//! The queue: what is waiting, what is moving, and what happened.
//!
//! Owned by the engine rather than the window, because the worker acts on it
//! and a queue the worker had to ask the window about would stall whenever the
//! window was busy. The window observes it through events.
//!
//! Persisted here too, for the same reason: the thing that changes the queue
//! is the thing that should be writing it down, and a shell that saved on
//! every event would race the worker that produced them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use super::{Status, Transfer};
use crate::error::{EngineError, Result};

/// A change worth telling the window about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEvent {
    /// The transfer that changed.
    pub transfer: Transfer,
}

/// Every transfer the app knows about.
pub struct TransferQueue {
    items: Arc<RwLock<HashMap<String, Transfer>>>,
    events: broadcast::Sender<QueueEvent>,
    /// Where the queue is written. `None` keeps it in memory, for tests.
    path: Option<PathBuf>,
}

impl TransferQueue {
    /// A queue backed by a file, restoring whatever was left in it.
    ///
    /// # Errors
    ///
    /// Never fails on a missing or unreadable file: an unreadable queue is an
    /// empty one. Losing a record of a transfer is recoverable; refusing to
    /// start the app because of it is not.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let restored = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Transfer>>(&text).ok())
            .unwrap_or_default();
        Self::from_parts(restore(restored), Some(path))
    }

    /// An in-memory queue, for tests.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::from_parts(HashMap::new(), None)
    }

    fn from_parts(items: HashMap<String, Transfer>, path: Option<PathBuf>) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            items: Arc::new(RwLock::new(items)),
            events,
            path,
        }
    }

    /// Subscribe to queue changes.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<QueueEvent> {
        self.events.subscribe()
    }

    /// Every transfer, oldest first.
    pub async fn all(&self) -> Vec<Transfer> {
        let mut all: Vec<Transfer> = self.items.read().await.values().cloned().collect();
        all.sort_by_key(|transfer| (transfer.queued_at_ms, transfer.id.clone()));
        all
    }

    /// Add a transfer and announce it.
    pub async fn add(&self, transfer: Transfer) {
        self.items
            .write()
            .await
            .insert(transfer.id.clone(), transfer.clone());
        self.announce(transfer).await;
    }

    /// Apply a change to one transfer, if it is still there.
    ///
    /// Takes a closure rather than a whole `Transfer` so a worker reporting
    /// progress cannot overwrite a `Cancelled` the user set while the chunk
    /// was in flight — the read and the write happen under one lock.
    pub async fn update(&self, id: &str, change: impl FnOnce(&mut Transfer)) -> Option<Transfer> {
        let updated = {
            let mut items = self.items.write().await;
            let transfer = items.get_mut(id)?;
            change(transfer);
            transfer.clone()
        };
        self.announce(updated.clone()).await;
        Some(updated)
    }

    /// The next transfer a free worker should start, if any.
    ///
    /// Oldest first, and only `Queued`: a paused transfer stays paused however
    /// idle the pool is, which is the whole difference between the two states.
    pub async fn next_queued(&self) -> Option<Transfer> {
        let items = self.items.read().await;
        items
            .values()
            .filter(|transfer| transfer.status == Status::Queued)
            .min_by_key(|transfer| (transfer.queued_at_ms, transfer.id.clone()))
            .cloned()
    }

    /// Drop everything finished or cancelled.
    pub async fn clear_finished(&self) -> Vec<String> {
        let removed: Vec<String> = {
            let mut items = self.items.write().await;
            let doomed: Vec<String> = items
                .values()
                .filter(|transfer| transfer.status.finished())
                .map(|transfer| transfer.id.clone())
                .collect();
            for id in &doomed {
                items.remove(id);
            }
            doomed
        };
        self.persist().await;
        removed
    }

    /// Pause everything that has not finished.
    pub async fn pause_all(&self) -> Vec<Transfer> {
        let changed = {
            let mut items = self.items.write().await;
            let mut changed = Vec::new();
            for transfer in items.values_mut() {
                if matches!(transfer.status, Status::Queued | Status::Running) {
                    transfer.status = Status::Paused;
                    changed.push(transfer.clone());
                }
            }
            changed
        };
        for transfer in &changed {
            let _ = self.events.send(QueueEvent {
                transfer: transfer.clone(),
            });
        }
        self.persist().await;
        changed
    }

    async fn announce(&self, transfer: Transfer) {
        let _ = self.events.send(QueueEvent { transfer });
        self.persist().await;
    }

    /// Write the queue out. A failure is logged by being ignored: losing the
    /// record is survivable, and failing a transfer because its bookkeeping
    /// could not be saved is not a trade worth making.
    async fn persist(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let all = self.all().await;
        let Ok(text) = serde_json::to_string_pretty(&all) else {
            return;
        };
        let _ = write_atomically(path, &text);
    }
}

/// Write through a temporary file, so a crash cannot truncate the queue.
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| EngineError::Io(format!("create {}: {error}", parent.display())))?;
    }
    let temporary = path.with_extension("writing");
    std::fs::write(&temporary, text)
        .map_err(|error| EngineError::Io(format!("write {}: {error}", temporary.display())))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| EngineError::Io(format!("replace {}: {error}", path.display())))
}

/// Bring a restored queue back to a state the worker can act on.
///
/// Anything that was mid-flight becomes `Queued` again, because the process
/// that was moving those bytes is gone. Its `bytes_done` is kept: a download
/// resumes from its `.part` file, and starting from zero would re-transfer
/// what is already on disk.
fn restore(transfers: Vec<Transfer>) -> HashMap<String, Transfer> {
    transfers
        .into_iter()
        .map(|mut transfer| {
            if matches!(transfer.status, Status::Running | Status::Queued) {
                transfer.status = Status::Queued;
                // The rate belonged to a process that no longer exists.
                transfer.rate = None;
                transfer.eta_seconds = None;
            }
            (transfer.id.clone(), transfer)
        })
        .collect()
}
