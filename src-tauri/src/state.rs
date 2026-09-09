//! What the shell holds between commands, and how the worker sees it.

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::{Protocol, ShareBackend};
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::xsync_engine::{SshEndpoint, XsyncRemote, XsyncRoute};
use excalibur_engine::{
    hostkeys, EditStore, EngineError, KnownHosts, Result, SecretStore, SessionManager,
};

/// The worker's window onto the session manager.
///
/// A thin adapter rather than the worker taking a `SessionManager` directly:
/// the worker should not know how a mount is chosen, only that there is one,
/// which is what lets a transfer survive a reconnect that swapped the backend
/// underneath it.
///
/// It asks for the *app's* mount rather than a window's (E23-S1). The queue is
/// app-wide and a transfer outlives the window that queued it — closing that
/// window must not strand the bytes — so there is no window here to ask. The
/// honest fix is a transfer that names its own endpoint, which is E22-S0's
/// work; until then this is what "the active mount" always meant, and with one
/// window it is exactly what it meant before.
pub(crate) struct AppMount {
    pub(crate) sessions: Arc<SessionManager>,
    /// The app's accepted SSH host keys.
    ///
    /// Held here, not just on the connector, because a transfer through
    /// xsync's engine spawns its *own* `ssh` (E22-S1) and it must enforce the
    /// same trust decision the browse session did. A second `ssh` that fell
    /// back to OpenSSH's own store would be a second, different policy.
    pub(crate) known_hosts: KnownHosts,
}

#[async_trait]
impl BackendSource for AppMount {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        self.sessions.recent_backend().await
    }

    /// The share a row names, rather than the one the app last touched.
    ///
    /// With two windows open, "the active mount" is not a thing the worker can
    /// ask for (E23-S1): a drag from one window into another (E24-S2) enqueues a
    /// transfer on a share that is by definition *not* the one most recently
    /// activated, and resolving it late would read from whichever window the
    /// user happened to click in last. Rows queued before this field existed
    /// still fall back, so a restored queue keeps working.
    async fn backend_for(&self, mount: Option<&str>) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = mount else {
            return self.sessions.recent_backend().await;
        };
        self.sessions
            .backend(id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))
    }

    /// Whether the share a row names is held right now.
    ///
    /// The whole of this app's answer to "not connected *yet*". Mounts are made
    /// when a window connects to something, so between launch and the first
    /// connection there are none — and the queue restored from the last run is
    /// full of rows naming shares from it. Asking here, before the row is
    /// claimed, is what makes that a wait instead of 260 identical failures.
    ///
    /// A map lookup and nothing else: it must not connect anything, because it
    /// is asked once per queued row per drain.
    ///
    /// The xsync route is included because it is a second way to reach a share
    /// — it spawns its own `ssh` and does not use the browse session at all —
    /// so a mount that is reconnecting can still move bytes, exactly as
    /// [`Self::xsync_route`] says.
    async fn is_connected(&self, mount: Option<&str>) -> bool {
        self.backend_for(mount).await.is_ok() || self.xsync_route(mount).await.is_some()
    }

    /// The recipe for a second `ssh` to an xsync export (E22-S1).
    ///
    /// `None` for every other protocol, and that `None` is the routing rule:
    /// the worker asks this first and falls back to its own chunk loop when
    /// there is no answer. So NFS, SMB, SFTP and a local folder are unchanged
    /// by this story without a single mention of them here.
    ///
    /// A mount that is not `Ready` is deliberately still answered. The
    /// transfer opens its own process and does not use the browse session at
    /// all, so a mount that is reconnecting can still move bytes — and the
    /// failure, if the host really is gone, is `ssh`'s own and reads better
    /// than "not connected".
    async fn xsync_route(&self, mount: Option<&str>) -> Option<XsyncRoute> {
        let id = match mount {
            Some(id) => id.to_owned(),
            None => self.sessions.recent().await?,
        };
        let spec = self.sessions.spec(&id).await?;
        if spec.protocol != Protocol::Xsync {
            return None;
        }
        // The mount's own answer, not what the user asked for at connect time:
        // `writable` is the one derived value every write is gated on, and on
        // this route it is the *only* gate — `xs --server --read-only` does not
        // stop a v1/v2 push.
        let mount_info = self.sessions.mount_info(&id).await;
        Some(XsyncRoute {
            remote: XsyncRemote::Ssh(SshEndpoint {
                host: spec.host.clone(),
                port: spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT),
                username: spec.username.clone(),
                // The app's own trust store, so the transfer's `ssh` enforces
                // exactly the decision the user made in the trust prompt — the
                // same file, the same `StrictHostKeyChecking=yes`, the same
                // `BatchMode`. `hostkeys::verify` already ran when the mount
                // was made; a key that changed since then is refused by `ssh`
                // here rather than silently accepted.
                known_hosts: self.known_hosts.path().to_path_buf(),
                rsh: None,
            }),
            export_root: spec.share.clone(),
            writable: mount_info.as_ref().is_some_and(|info| info.writable),
            reason: mount_info.map_or_else(
                || "the mount has not reported its access yet".to_owned(),
                |info| info.reason,
            ),
        })
    }
}

/// What the shell holds: the mounts, and everything that acts on them.
pub(crate) struct AppState {
    pub(crate) sessions: Arc<SessionManager>,
    /// The OS credential store. Written to from the window, never read back to
    /// it: see the commands below.
    pub(crate) secrets: Arc<dyn SecretStore>,
    /// Accepted SSH host keys. Shared with the connector, which is what
    /// enforces them.
    pub(crate) known_hosts: KnownHosts,
    /// Files checked out for editing.
    pub(crate) edits: Arc<EditStore>,
    /// Everything moving, or waiting to.
    pub(crate) transfers: Arc<TransferQueue>,
    /// Moves it. Held so a new enqueue can wake it.
    pub(crate) worker: Arc<Worker>,
}

impl AppState {
    /// The backend a command should use: the **calling window's** mount, or
    /// nothing.
    ///
    /// The window is not a detail of the call, it is the subject of it
    /// (E23-S1): with two browser windows open there is no such thing as "the
    /// active mount", and a command that guessed would list one window's share
    /// into the other's list. Tauri injects the caller's `Window`, so no
    /// command has to be trusted to pass the right label.
    ///
    /// There is deliberately no fallback (E1-S6). A mock share stood here so
    /// the window had something to draw on first run, and what it drew was a
    /// share that does not exist — with verbs that acted on it, so Download
    /// queued a row for a file nobody has. "Nothing is mounted" is a fact the
    /// window can render as an empty state; a fake listing is not something it
    /// can tell apart from a real one.
    pub(crate) async fn backend(&self, window: &tauri::Window) -> Result<Arc<dyn ShareBackend>> {
        self.sessions.active_backend(window.label()).await
    }

    /// A named mount and its backend, defaulting to the calling window's.
    ///
    /// The id comes back with the backend because every queue row now records
    /// which share it moves bytes through, and resolving the id here — once —
    /// is what stops a caller stamping a row with one mount and planning it
    /// against another.
    ///
    /// `mount` is `None` for every caller that means "this window": Download,
    /// Upload, the conflict check behind both. It is `Some` only for a drop
    /// carrying another window's selection (E24-S2), where the source share is
    /// named in the payload rather than implied by who is asking.
    pub(crate) async fn mount_and_backend(
        &self,
        window: &tauri::Window,
        mount: Option<&str>,
    ) -> Result<(String, Arc<dyn ShareBackend>)> {
        let Some(id) = mount
            .map(str::to_owned)
            .or(self.sessions.active(window.label()).await)
        else {
            return Err(EngineError::Disconnected("no share is mounted".to_owned()));
        };
        let backend = self
            .sessions
            .backend(&id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))?;
        Ok((id, backend))
    }
}
