//! Mount lifecycle: connecting, holding, watching and reconnecting.
//!
//! One backend instance per mount, several mounts at once, and a health loop
//! per mount so a share that goes away takes only itself down.
//!
//! The reason this is not just a map: the design gates every write affordance
//! on `MountInfo::writable`, and a mount that reconnects may come back
//! *different* — an export flipped to read-only while the app was running. So
//! reconnecting is not merely restoring a connection, it is re-reading the
//! facts and republishing them. That is the part worth testing, and the part a
//! simpler design would get wrong.
//!
//! **"Active" is per window, not per app (E23-S1).** The shell can show several
//! browser windows over one engine, and each of them is looking at its own
//! mount. So the single `active` this held became a map keyed by an opaque
//! window label — a `String` the caller chooses, which keeps Tauri out of the
//! engine — and every caller that used to mean "the active mount" says whose.
//! Two windows on one mount share one session and one connection; the session
//! goes when the last window holding it does, which is
//! [`SessionManager::close_window`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};

use crate::backend::logged::LoggedBackend;
use crate::backend::{Access, ConnectionSpec, MountInfo, MountStatus, ShareBackend};
use crate::error::{EngineError, Result};
use crate::log::{LogAction, LogCategory, LogLine, MessageLog};

/// Builds a backend for a connection.
///
/// A trait rather than a `match` on the protocol so a test can supply a
/// connector that fails when told to. The reconnect path is the one that most
/// needs testing and the one a real server is worst at reproducing on demand.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Connect and mount, or say why not.
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>>;
}

/// What the shell is told when a mount changes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    /// Which mount.
    pub id: String,
    /// Where it is in its lifecycle.
    pub status: MountStatus,
    /// The mount facts, when there are any to report.
    ///
    /// Present on every `Ready`, because that is the moment a UI must
    /// re-read `writable` rather than trusting what it had.
    pub mount: Option<MountInfo>,
    /// Why, when the status is `Error`.
    pub reason: Option<String>,
    /// Which windows were looking at this mount when it changed (E23-S1).
    ///
    /// Routing, not payload — hence `serde(skip)`: what the window renders is
    /// the four fields above, and this only says who should be told. It is
    /// computed here rather than by the shell at delivery time because the
    /// answer must be the one that was true *when the event happened*: a mount
    /// that failed and gave the window its old share back would otherwise have
    /// its failure delivered to whoever happens to be on it a moment later.
    ///
    /// Empty means nobody is looking, which is normal — the CLI has no
    /// windows, and a health loop can outlive the window that opened it by the
    /// length of one event.
    #[serde(skip)]
    pub audience: Vec<String>,
}

/// How often to check, and how long to wait before trying again.
#[derive(Debug, Clone, Copy)]
pub struct HealthPolicy {
    /// Interval between pings on a healthy mount.
    pub interval: Duration,
    /// Wait before the first reconnect attempt.
    pub backoff_initial: Duration,
    /// Longest wait between attempts.
    pub backoff_max: Duration,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            // Often enough that a dropped share is noticed while the user is
            // still looking at it, rarely enough that an idle app is not
            // chattering at every mounted server.
            interval: Duration::from_secs(15),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

struct Session {
    spec: ConnectionSpec,
    backend: Option<Arc<dyn ShareBackend>>,
    status: MountStatus,
    mount: Option<MountInfo>,
}

/// Which windows are looking at `id`, in a stable order.
///
/// A free function rather than a method because the health loop holds the map
/// and not the manager, and both of them have to answer the same question the
/// same way.
fn audience_of(active: &HashMap<String, String>, id: &str) -> Vec<String> {
    let mut windows: Vec<String> = active
        .iter()
        .filter(|(_, mount)| mount.as_str() == id)
        .map(|(window, _)| window.clone())
        .collect();
    // Sorted so a test can assert on it and a log reads the same twice; a
    // `HashMap` would otherwise hand back a different order every run.
    windows.sort();
    windows
}

/// Every mount the app holds.
pub struct SessionManager {
    connector: Arc<dyn Connector>,
    policy: HealthPolicy,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    /// Which mount each window is looking at, keyed by window label (E23-S1).
    ///
    /// The label is whatever the caller uses to name a window; the engine never
    /// interprets it. Several windows may name the same mount, which is the
    /// point: they share the one session underneath.
    active: Arc<RwLock<HashMap<String, String>>>,
    /// The mount some window most recently made active.
    ///
    /// The app-wide fallback for the things that are not a window's: the
    /// transfer worker, and the stream handler when a URL names no window. It
    /// is not "the active mount" — with two windows there is no such thing —
    /// and it is deliberately not used by any command that has a window to ask.
    recent: Arc<RwLock<Option<String>>>,
    events: broadcast::Sender<SessionEvent>,
    /// The app's message log (`twopane.md` XTP-E5-S1).
    ///
    /// Held here because this is where a mount's life happens — connecting,
    /// coming back read-only, going away — and those are the lines the design
    /// calls protocol truth. Every backend handed out is wrapped so its verbs
    /// join the same stream.
    log: Arc<MessageLog>,
    /// Health tasks, aborted when their mount goes or the manager drops.
    tasks: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl SessionManager {
    /// A manager that builds backends with `connector`.
    #[must_use]
    pub fn new(connector: Arc<dyn Connector>) -> Self {
        Self::with_policy(connector, HealthPolicy::default())
    }

    /// As [`Self::new`], with an explicit health policy.
    #[must_use]
    pub fn with_policy(connector: Arc<dyn Connector>, policy: HealthPolicy) -> Self {
        Self {
            connector,
            policy,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            active: Arc::new(RwLock::new(HashMap::new())),
            recent: Arc::new(RwLock::new(None)),
            // Bounded: a subscriber that stops reading loses old events rather
            // than growing the queue without limit. Lagging is survivable
            // because every event carries the full state, not a delta.
            events: broadcast::channel(64).0,
            log: Arc::new(MessageLog::new()),
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// What a mount came back as, and what can be done about it.
    ///
    /// The handoff's `2b` draws exactly this pair: a `Status:` line naming the
    /// export flags, and a `Hint:` under it offering `Remount as RW`. The hint
    /// carries the verb rather than a label, so the shell is not matching on
    /// prose to decide which button to draw.
    fn announce_mount(&self, id: &str, info: &MountInfo) {
        let access = if info.writable {
            "read-write"
        } else {
            "read-only"
        };
        let options = if info.options.is_empty() {
            String::new()
        } else {
            format!(" — {}", info.options)
        };
        self.log.record(LogLine::on(
            id,
            LogCategory::Status,
            format!("mounted {access}{options}"),
        ));
        if !info.writable {
            self.log.record(
                LogLine::on(
                    id,
                    LogCategory::Hint,
                    if info.reason.is_empty() {
                        "the export is read-only".to_owned()
                    } else {
                        info.reason.clone()
                    },
                )
                .with_action(LogAction::RemountRw),
            );
        }
    }

    /// The message log every mount and verb writes to.
    #[must_use]
    pub fn log(&self) -> Arc<MessageLog> {
        Arc::clone(&self.log)
    }

    /// Subscribe to mount changes.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    fn publish(&self, event: SessionEvent) {
        // No subscribers is normal, not an error: the CLI has none.
        let _ = self.events.send(event);
    }

    /// Who should hear about `id` changing, plus the window that asked.
    ///
    /// `asking` is included even though it is not on the mount yet: a window
    /// that is connecting is the one window that must see `Mounting` and, if it
    /// fails, `Error` — and it is not in the map until the connection succeeds,
    /// because a failed mount leaves the window on the share it already had.
    async fn audience(&self, id: &str, asking: Option<&str>) -> Vec<String> {
        let mut windows = audience_of(&*self.active.read().await, id);
        if let Some(window) = asking {
            if !windows.iter().any(|held| held == window) {
                windows.push(window.to_owned());
                windows.sort();
            }
        }
        windows
    }

    /// Connect and mount a share, making it `window`'s active one.
    ///
    /// Mounting a share a second window already holds does **not** connect
    /// twice: the session is replaced by a fresh one, and both windows then
    /// look at it. That is a deliberate simplification over refusing — the
    /// caller asked for a connection, and the alternative is a mount that
    /// silently ignores what was typed in the dialog.
    ///
    /// # Errors
    ///
    /// Returns the connector's failure. The session is kept in `Error` so the
    /// UI can show it and offer a reconnect rather than losing the row.
    pub async fn mount(&self, window: &str, spec: ConnectionSpec) -> Result<()> {
        self.connect(Some(window), spec).await
    }

    /// Connect a share with no window on it, and leave it that way.
    ///
    /// For the shares the app should already be holding before anyone asks: the
    /// sidebar's local favourites, which are folders on this machine that a
    /// window will almost certainly want and that cost nothing to open. The
    /// point is the transfer queue rather than the browsing — a queue restored
    /// from the last run names shares nobody has mounted yet, and this is what
    /// there is for it to wait for.
    ///
    /// Two differences from [`Self::mount`], and both of them are the reason
    /// this is a separate method rather than an `Option`:
    ///
    /// - **Nothing already held is disturbed.** Mounting again would replace a
    ///   live session — which is right when a user asks for it and wrong when
    ///   the app is being helpful behind them, and with several windows this
    ///   runs once per window.
    /// - **It does not become `recent`.** That is the fallback for rows which
    ///   name no mount, and a share the user never opened must not become the
    ///   answer to "the share this app is working with".
    ///
    /// # Errors
    ///
    /// Returns the connector's failure, for a caller that wants to know. A
    /// favourite whose folder has gone away fails here and is meant to be
    /// ignored: the event carries no audience, so no window is told either.
    pub async fn preconnect(&self, spec: ConnectionSpec) -> Result<()> {
        if self.sessions.read().await.contains_key(&spec.id) {
            return Ok(());
        }
        self.connect(None, spec).await
    }

    async fn connect(&self, window: Option<&str>, spec: ConnectionSpec) -> Result<()> {
        let id = spec.id.clone();
        self.log.record(LogLine::on(
            &id,
            LogCategory::Command,
            format!("mount {}", describe(&spec)),
        ));
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(
                id.clone(),
                Session {
                    spec: spec.clone(),
                    backend: None,
                    status: MountStatus::Mounting,
                    mount: None,
                },
            );
        }
        self.publish(SessionEvent {
            id: id.clone(),
            status: MountStatus::Mounting,
            mount: None,
            reason: None,
            audience: self.audience(&id, window).await,
        });

        match self.connector.connect(&spec).await {
            Ok(backend) => {
                let info = backend.mount_info();
                self.announce_mount(&id, &info);
                // Wrapped from here on, so every verb the shell asks of this
                // share joins the same stream the mount did.
                let backend: Arc<dyn ShareBackend> = Arc::new(LoggedBackend::new(
                    backend,
                    Arc::clone(&self.log),
                    id.clone(),
                ));
                {
                    let mut sessions = self.sessions.write().await;
                    if let Some(session) = sessions.get_mut(&id) {
                        session.backend = Some(Arc::clone(&backend));
                        session.status = MountStatus::Ready;
                        session.mount = Some(info.clone());
                    }
                }
                // A preconnect has no window to make active and must not
                // become the app-wide fallback either: see [`Self::preconnect`].
                if let Some(window) = window {
                    self.active
                        .write()
                        .await
                        .insert(window.to_owned(), id.clone());
                    *self.recent.write().await = Some(id.clone());
                }
                self.publish(SessionEvent {
                    id: id.clone(),
                    status: MountStatus::Ready,
                    mount: Some(info),
                    reason: None,
                    audience: self.audience(&id, None).await,
                });
                self.spawn_health_check(id).await;
                Ok(())
            }
            Err(error) => {
                self.log.record(LogLine::on(
                    &id,
                    LogCategory::Error,
                    format!("mount failed: {error}"),
                ));
                {
                    let mut sessions = self.sessions.write().await;
                    if let Some(session) = sessions.get_mut(&id) {
                        session.status = MountStatus::Error;
                    }
                }
                // The window that asked keeps whatever share it had: a
                // connection that never came up must not take a working one
                // off the screen of the window that tried.
                let audience = self.audience(&id, window).await;
                self.publish(SessionEvent {
                    id,
                    status: MountStatus::Error,
                    mount: None,
                    reason: Some(error.to_string()),
                    audience,
                });
                Err(error)
            }
        }
    }

    /// Reconnect a mount asking for read-write, and report what came back.
    ///
    /// The recovery path behind "Remount as RW". It re-runs `connect` with
    /// `mount_preference = Rw` and re-reads `writable` from the result.
    ///
    /// **It does not retry, and it does not report failure when the server says
    /// no.** A share that comes back read-only again returns `Ok` with
    /// `writable == false` and the server's reason, because that is not an
    /// error — it is the answer. Treating it as one is how a recovery button
    /// becomes a loop: the caller sees a failure, offers the same button, and
    /// the user presses it forever. The design is explicit that the error text
    /// updates to the server's reason instead.
    ///
    /// The stored spec is updated on success, so a later reconnect keeps asking
    /// for read-write rather than quietly reverting to what was first typed.
    ///
    /// # Errors
    ///
    /// Only when the connection itself fails. A refused *upgrade* is `Ok`.
    /// A share that comes back read-only is `Ok(info)` with `writable == false`.
    pub async fn remount_read_write(&self, id: &str) -> Result<MountInfo> {
        self.reconnect_with(id, Some(Access::Rw)).await
    }

    /// Reconnect a mount as it was mounted, and report what came back.
    ///
    /// The toolbar's `Reconnect` and the connection banner's. Deliberately not
    /// [`Self::remount_read_write`]: that one asks for read-write whatever the
    /// share was mounted as, which is right for a button labelled "Remount as
    /// RW" and wrong for one labelled "Reconnect" — a share somebody chose to
    /// mount read-only would come back writable without being asked, which is
    /// a protection silently removed.
    ///
    /// # Errors
    ///
    /// When there is no such mount, or the connection fails.
    pub async fn reconnect(&self, id: &str) -> Result<MountInfo> {
        self.reconnect_with(id, None).await
    }

    /// Reconnect, optionally changing what access is asked for.
    ///
    /// A server that refuses the access asked for is not an error: the mount
    /// facts that came back are returned as they are, and the caller re-reads
    /// `writable` from them. Only the connection failing is an `Err`.
    ///
    /// The stored spec is replaced on success, so the next reconnect asks for
    /// whatever this one settled on.
    async fn reconnect_with(&self, id: &str, access: Option<Access>) -> Result<MountInfo> {
        let mut spec = {
            let sessions = self.sessions.read().await;
            sessions
                .get(id)
                .ok_or_else(|| EngineError::NotFound(format!("no mount {id}")))?
                .spec
                .clone()
        };
        if let Some(access) = access {
            spec.mount_preference = access;
        }

        self.log.record(LogLine::on(
            id,
            LogCategory::Command,
            format!("reconnect {}", describe(&spec)),
        ));
        let backend = self.connector.connect(&spec).await.inspect_err(|error| {
            self.log.record(LogLine::on(
                id,
                LogCategory::Error,
                format!("reconnect failed: {error}"),
            ));
        })?;
        let info = backend.mount_info();
        self.announce_mount(id, &info);
        let backend: Arc<dyn ShareBackend> =
            Arc::new(LoggedBackend::new(backend, Arc::clone(&self.log), id));
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(id) {
                session.spec = spec;
                session.backend = Some(Arc::clone(&backend));
                session.status = MountStatus::Ready;
                session.mount = Some(info.clone());
            }
        }
        self.publish(SessionEvent {
            id: id.to_owned(),
            status: MountStatus::Ready,
            mount: Some(info.clone()),
            reason: None,
            // Every window on this mount, not just whoever pressed the button:
            // the mount facts are new for all of them, and a window still
            // showing an `RO` badge over a mount that came back writable is the
            // disagreement this event exists to prevent.
            audience: self.audience(id, None).await,
        });
        Ok(info)
    }

    /// Connect, read the mount facts, and disconnect again.
    ///
    /// The dialog's `Test` button. It connects for real rather than probing the
    /// port, because the questions a user is actually asking — are my
    /// credentials right, does this export exist, will I be able to write — are
    /// only answerable by mounting it. Reporting "reachable" from a successful
    /// TCP connect would be answering an easier question than the one asked.
    ///
    /// # Errors
    ///
    /// Returns the connector's failure, which is the thing worth showing.
    pub async fn test(&self, spec: &ConnectionSpec) -> Result<MountInfo> {
        let backend = self.connector.connect(spec).await?;
        Ok(backend.mount_info())
    }

    /// Drop a mount and stop watching it, whoever is looking at it.
    ///
    /// The explicit disconnect. Every window on it is left with nothing, which
    /// is the honest outcome: the share is gone, and leaving windows pointing
    /// at a session that is not there would be a listing that fails without
    /// saying why.
    pub async fn unmount(&self, id: &str) {
        if let Some(task) = self.tasks.write().await.remove(id) {
            task.abort();
        }
        self.sessions.write().await.remove(id);
        self.active.write().await.retain(|_, mount| mount != id);
        let mut recent = self.recent.write().await;
        if recent.as_deref() == Some(id) {
            *recent = None;
        }
    }

    /// Forget a window, and disconnect its mount if it was the last one on it.
    ///
    /// Returns the mount that was closed, if one was. This is the other half of
    /// "a mount used by two windows is one session": closing one of them must
    /// not take the share away from the other, and closing the last one must
    /// not leave a connection nobody can see.
    ///
    /// Note what it is *not*: a window switching mounts does not close the one
    /// it left. Those sessions are kept deliberately, so switching back costs
    /// nothing — a window's history of shares is not the same thing as the set
    /// of windows open on them.
    pub async fn close_window(&self, window: &str) -> Option<String> {
        let left = self.active.write().await.remove(window)?;
        if !audience_of(&*self.active.read().await, &left).is_empty() {
            return None;
        }
        self.unmount(&left).await;
        Some(left)
    }

    /// Make `id` the mount `window` is looking at.
    ///
    /// Switching does not disconnect anything: the other mounts keep their
    /// connections and their health loops, because a user switching back
    /// should not pay for a reconnect.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotFound`] when no such mount is held.
    pub async fn set_active(&self, window: &str, id: &str) -> Result<()> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(EngineError::NotFound(format!("no mount {id}")));
        }
        self.active
            .write()
            .await
            .insert(window.to_owned(), id.to_owned());
        *self.recent.write().await = Some(id.to_owned());
        Ok(())
    }

    /// The mount one window is looking at.
    pub async fn active(&self, window: &str) -> Option<String> {
        self.active.read().await.get(window).cloned()
    }

    /// Every window looking at `id`, in a stable order.
    pub async fn windows_on(&self, id: &str) -> Vec<String> {
        audience_of(&*self.active.read().await, id)
    }

    /// The mount some window most recently made active, for the app-wide
    /// callers that have no window to ask: the transfer worker, and a stream
    /// URL that names no window.
    pub async fn recent(&self) -> Option<String> {
        self.recent.read().await.clone()
    }

    /// How many mounts are held, whatever their state.
    pub async fn len(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Whether no mounts are held.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Where a mount is in its lifecycle.
    pub async fn status(&self, id: &str) -> Option<MountStatus> {
        self.sessions.read().await.get(id).map(|s| s.status)
    }

    /// The last mount facts published for `id`.
    pub async fn mount_info(&self, id: &str) -> Option<MountInfo> {
        self.sessions
            .read()
            .await
            .get(id)
            .and_then(|s| s.mount.clone())
    }

    /// What `id` was mounted from.
    ///
    /// A transfer through xsync's engine needs the host, port, user and export
    /// root again, because it spawns its own `ssh` (E22-S1): one `xs --server`
    /// serves one session, and a browse session and a sync session are
    /// different protocols. The live backend cannot answer that — it is the
    /// far end of a socket, not a recipe for opening another one.
    pub async fn spec(&self, id: &str) -> Option<ConnectionSpec> {
        self.sessions.read().await.get(id).map(|s| s.spec.clone())
    }

    /// The backend for `id`, if it is connected.
    pub async fn backend(&self, id: &str) -> Option<Arc<dyn ShareBackend>> {
        self.sessions
            .read()
            .await
            .get(id)
            .and_then(|s| s.backend.clone())
    }

    /// The backend one window is looking at.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Disconnected`] when that window has nothing
    /// mounted, or its mount is not currently connected.
    pub async fn active_backend(&self, window: &str) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = self.active(window).await else {
            return Err(EngineError::Disconnected("no share is mounted".to_owned()));
        };
        self.backend(&id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))
    }

    /// The backend behind [`Self::recent`], for callers with no window.
    ///
    /// # Errors
    ///
    /// As [`Self::active_backend`], for the app-wide mount instead of a
    /// window's.
    pub async fn recent_backend(&self) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = self.recent().await else {
            return Err(EngineError::Disconnected("no share is mounted".to_owned()));
        };
        self.backend(&id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))
    }

    /// Watch one mount, reconnecting it when it stops answering.
    async fn spawn_health_check(&self, id: String) {
        let sessions = Arc::clone(&self.sessions);
        let connector = Arc::clone(&self.connector);
        let events = self.events.clone();
        let policy = self.policy;
        // The loop routes its own events, so a share that drops reaches every
        // window on it and no others — including windows that joined the mount
        // after the loop started.
        let active = Arc::clone(&self.active);

        let watched = id.clone();
        let task = tokio::spawn(async move {
            let id = watched;
            let mut backoff = policy.backoff_initial;
            loop {
                tokio::time::sleep(policy.interval).await;

                let Some(backend) = sessions
                    .read()
                    .await
                    .get(&id)
                    .and_then(|s| s.backend.clone())
                else {
                    // Unmounted, or never connected: nothing left to watch.
                    return;
                };
                if matches!(backend.ping().await, Ok(crate::backend::Health::Healthy)) {
                    backoff = policy.backoff_initial;
                    continue;
                }

                // Report the drop before trying to fix it, so the UI dims and
                // shows its banner while the reconnect is in flight rather
                // than after it resolves.
                let spec = {
                    let mut guard = sessions.write().await;
                    let Some(session) = guard.get_mut(&id) else {
                        return;
                    };
                    session.status = MountStatus::Reconnecting;
                    session.backend = None;
                    session.spec.clone()
                };
                let _ = events.send(SessionEvent {
                    id: id.clone(),
                    status: MountStatus::Reconnecting,
                    mount: None,
                    reason: None,
                    audience: audience_of(&*active.read().await, &id),
                });

                loop {
                    tokio::time::sleep(backoff).await;
                    match connector.connect(&spec).await {
                        Ok(backend) => {
                            // The mount may have come back *different* — an
                            // export flipped to read-only while the app was
                            // running. Republishing the facts is the point of
                            // reconnecting, not a side effect of it.
                            let info = backend.mount_info();
                            {
                                let mut guard = sessions.write().await;
                                let Some(session) = guard.get_mut(&id) else {
                                    return;
                                };
                                session.backend = Some(backend);
                                session.status = MountStatus::Ready;
                                session.mount = Some(info.clone());
                            }
                            let _ = events.send(SessionEvent {
                                id: id.clone(),
                                status: MountStatus::Ready,
                                mount: Some(info),
                                reason: None,
                                audience: audience_of(&*active.read().await, &id),
                            });
                            backoff = policy.backoff_initial;
                            break;
                        }
                        // A failure that retrying cannot fix stops the loop
                        // (E15-S2). Credentials are the case that matters: a
                        // client retrying a rejected password every few
                        // seconds locks the account, and it does it while the
                        // user is looking at a banner that says "reconnecting"
                        // — so the damage is invisible until they cannot log
                        // in anywhere. A changed host key is the same shape.
                        Err(error) if !error.transient() => {
                            {
                                let mut guard = sessions.write().await;
                                let Some(session) = guard.get_mut(&id) else {
                                    return;
                                };
                                session.status = MountStatus::Error;
                            }
                            let _ = events.send(SessionEvent {
                                id: id.clone(),
                                status: MountStatus::Error,
                                mount: None,
                                reason: Some(error.to_string()),
                                audience: audience_of(&*active.read().await, &id),
                            });
                            return;
                        }
                        Err(error) => {
                            let _ = events.send(SessionEvent {
                                id: id.clone(),
                                status: MountStatus::Reconnecting,
                                mount: None,
                                reason: Some(error.to_string()),
                                audience: audience_of(&*active.read().await, &id),
                            });
                            // Backing off rather than hammering: a server that
                            // is down stays down for longer than one second,
                            // and a hundred clients retrying in a tight loop
                            // is how a recovering server stays down.
                            backoff = (backoff * 2).min(policy.backoff_max);
                        }
                    }
                    if sessions.read().await.get(&id).is_none() {
                        return;
                    }
                }
            }
        });
        self.tasks.write().await.insert(id, task);
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // The health tasks hold `Arc`s to shared state; without this they would
        // outlive the manager and keep reconnecting to shares nobody holds.
        if let Ok(tasks) = self.tasks.try_write() {
            for task in tasks.values() {
                task.abort();
            }
        }
    }
}

/// Whether a host is answering on a port, for the sidebar's dimmed rows.
///
/// A TCP connect, not a mount: this runs for every saved bookmark on a timer,
/// and mounting each one would ask a server for credentials nobody typed. It
/// answers a deliberately weaker question — "is something listening" — and the
/// design uses it for exactly that, dimming a row rather than claiming the
/// share is usable.
pub async fn reachable(host: &str, port: u16, timeout: Duration) -> bool {
    // Every address at once, first answer wins. `TcpStream::connect` on a
    // name tries the addresses one after another, and a `.local` host on
    // this network resolves to an IPv6 link-local address *before* its IPv4
    // one; the link-local attempt can sit in the timeout for the whole budget
    // and the probe reports a reachable machine as down. Found 2026-09-05
    // with orion.local, which `nc` reached in 39 ms while the sidebar dimmed it.
    let probe = async {
        let Ok(addresses) = tokio::net::lookup_host((host, port)).await else {
            return false;
        };
        let mut attempts = tokio::task::JoinSet::new();
        for address in addresses {
            attempts.spawn(async move { tokio::net::TcpStream::connect(address).await.is_ok() });
        }
        while let Some(result) = attempts.join_next().await {
            if matches!(result, Ok(true)) {
                return true;
            }
        }
        false
    };
    matches!(tokio::time::timeout(timeout, probe).await, Ok(true))
}

/// A connection as the log names it: `xsync://freya.local/mnt/data (rw)`.
///
/// The spec's own fields rather than the bookmark's display name: the log is
/// where protocol truth lives, and "data" does not say which host it is on.
fn describe(spec: &ConnectionSpec) -> String {
    let scheme = format!("{:?}", spec.protocol).to_lowercase();
    let port = spec.port.map(|port| format!(":{port}")).unwrap_or_default();
    let share = spec.share.trim_start_matches('/');
    let access = match spec.mount_preference {
        Access::Rw => "rw",
        Access::Ro => "ro",
    };
    format!("{scheme}://{}{port}/{share} ({access})", spec.host)
}
