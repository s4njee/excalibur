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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};

use crate::backend::{ConnectionSpec, MountInfo, MountStatus, ShareBackend};
use crate::error::{EngineError, Result};

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

/// Every mount the app holds.
pub struct SessionManager {
    connector: Arc<dyn Connector>,
    policy: HealthPolicy,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    active: Arc<RwLock<Option<String>>>,
    events: broadcast::Sender<SessionEvent>,
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
            active: Arc::new(RwLock::new(None)),
            // Bounded: a subscriber that stops reading loses old events rather
            // than growing the queue without limit. Lagging is survivable
            // because every event carries the full state, not a delta.
            events: broadcast::channel(64).0,
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
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

    /// Connect and mount a share, making it the active one.
    ///
    /// # Errors
    ///
    /// Returns the connector's failure. The session is kept in `Error` so the
    /// UI can show it and offer a reconnect rather than losing the row.
    pub async fn mount(&self, spec: ConnectionSpec) -> Result<()> {
        let id = spec.id.clone();
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
        });

        match self.connector.connect(&spec).await {
            Ok(backend) => {
                let info = backend.mount_info();
                {
                    let mut sessions = self.sessions.write().await;
                    if let Some(session) = sessions.get_mut(&id) {
                        session.backend = Some(Arc::clone(&backend));
                        session.status = MountStatus::Ready;
                        session.mount = Some(info.clone());
                    }
                }
                *self.active.write().await = Some(id.clone());
                self.publish(SessionEvent {
                    id: id.clone(),
                    status: MountStatus::Ready,
                    mount: Some(info),
                    reason: None,
                });
                self.spawn_health_check(id).await;
                Ok(())
            }
            Err(error) => {
                {
                    let mut sessions = self.sessions.write().await;
                    if let Some(session) = sessions.get_mut(&id) {
                        session.status = MountStatus::Error;
                    }
                }
                self.publish(SessionEvent {
                    id,
                    status: MountStatus::Error,
                    mount: None,
                    reason: Some(error.to_string()),
                });
                Err(error)
            }
        }
    }

    /// Drop a mount and stop watching it.
    pub async fn unmount(&self, id: &str) {
        if let Some(task) = self.tasks.write().await.remove(id) {
            task.abort();
        }
        self.sessions.write().await.remove(id);
        let mut active = self.active.write().await;
        if active.as_deref() == Some(id) {
            *active = None;
        }
    }

    /// Make `id` the mount the UI is looking at.
    ///
    /// Switching does not disconnect anything: the other mounts keep their
    /// connections and their health loops, because a user switching back
    /// should not pay for a reconnect.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotFound`] when no such mount is held.
    pub async fn set_active(&self, id: &str) -> Result<()> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(EngineError::NotFound(format!("no mount {id}")));
        }
        *self.active.write().await = Some(id.to_owned());
        Ok(())
    }

    /// The mount the UI is looking at.
    pub async fn active(&self) -> Option<String> {
        self.active.read().await.clone()
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

    /// The backend for `id`, if it is connected.
    pub async fn backend(&self, id: &str) -> Option<Arc<dyn ShareBackend>> {
        self.sessions
            .read()
            .await
            .get(id)
            .and_then(|s| s.backend.clone())
    }

    /// The active mount's backend.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Disconnected`] when nothing is mounted, or the
    /// active mount is not currently connected.
    pub async fn active_backend(&self) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = self.active().await else {
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
                            });
                            backoff = policy.backoff_initial;
                            break;
                        }
                        Err(error) => {
                            let _ = events.send(SessionEvent {
                                id: id.clone(),
                                status: MountStatus::Reconnecting,
                                mount: None,
                                reason: Some(error.to_string()),
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
