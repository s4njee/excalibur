//! The queue: what is waiting, what is moving, and what happened.
//!
//! Owned by the engine rather than the window, because the worker acts on it
//! and a queue the worker had to ask the window about would stall whenever the
//! window was busy. The window observes it through events.
//!
//! Persisted here too, for the same reason: the thing that changes the queue
//! is the thing that should be writing it down, and a shell that saved on
//! every event would race the worker that produced them.
//!
//! **Every update is broadcast; only a durable one is written down.** The two
//! used to be the same call, and the cost of that was measured rather than
//! argued: a real `transfers.json` on this machine is 1.2 MB across 2253 rows,
//! and [`TransferQueue::persist`] rewrites the whole file, so every progress
//! tick of every running transfer rewrote 1.2 MB. Progress is the one thing in
//! a row that a restart does not need — see [`restore`], which throws away the
//! rate and the ETA and re-queues anything that was moving — so the file is
//! written when the row's *durable* shape changes and not when its byte count
//! moves. There is no timer and no coalescing window: the rule is a comparison
//! of two values, which is a thing a test can state.

use std::collections::{HashMap, HashSet};
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
        // A row appearing is durable by definition: it is the whole of what
        // the file exists to remember.
        self.announce(transfer, true).await;
    }

    /// Apply a change to one transfer, if it is still there.
    ///
    /// Takes a closure rather than a whole `Transfer` so a worker reporting
    /// progress cannot overwrite a `Cancelled` the user set while the chunk
    /// was in flight — the read and the write happen under one lock.
    pub async fn update(&self, id: &str, change: impl FnOnce(&mut Transfer)) -> Option<Transfer> {
        let (updated, durable_changed) = {
            let mut items = self.items.write().await;
            let transfer = items.get_mut(id)?;
            let before = durable(transfer);
            change(transfer);
            let after = durable(transfer);
            (transfer.clone(), before != after)
        };
        self.announce(updated.clone(), durable_changed).await;
        Some(updated)
    }

    /// Forget one transfer, whatever state it is in.
    ///
    /// For a row whose destination stops existing when the thing that made it
    /// finishes — a promised drag's staging directory (E24-S1). Leaving such a
    /// row in the file is not a harmless record: [`restore`] re-queues it on
    /// the next launch, and it would copy a file into a directory that was
    /// deleted when the drag ended.
    pub async fn remove(&self, id: &str) -> Option<Transfer> {
        let gone = self.items.write().await.remove(id);
        if gone.is_some() {
            self.persist().await;
        }
        gone
    }

    /// Take many transfers out at once, writing the file once.
    ///
    /// The startup sweep removes every row that named an abandoned staging
    /// directory, and a drag of a large folder makes thousands of them. One
    /// [`Self::remove`] each would rewrite the whole queue thousands of times
    /// before the first window could paint — the file starts at over a
    /// megabyte, so the cost is quadratic in the bytes and was measured in
    /// seconds of boot.
    pub async fn remove_all(&self, ids: &[String]) -> usize {
        let gone = {
            let mut items = self.items.write().await;
            ids.iter().filter(|id| items.remove(*id).is_some()).count()
        };
        if gone > 0 {
            self.persist().await;
        }
        gone
    }

    /// The next transfer a free worker should start, if any.
    ///
    /// Oldest first, and only `Queued`: a paused transfer stays paused however
    /// idle the pool is, which is the whole difference between the two states.
    pub async fn next_queued(&self) -> Option<Transfer> {
        self.next_queued_except(&HashSet::new()).await
    }

    /// As [`Self::next_queued`], ignoring rows the caller has already passed
    /// over.
    ///
    /// The worker leaves a transfer `Queued` when the share it names is not
    /// connected yet, because that is a wait rather than a failure — and a row
    /// that stays `Queued` is a row [`Self::next_queued`] hands back for ever.
    /// The skip set is what turns "come back to this one later" into a loop
    /// that ends.
    pub async fn next_queued_except(&self, skip: &HashSet<String>) -> Option<Transfer> {
        let items = self.items.read().await;
        items
            .values()
            .filter(|transfer| transfer.status == Status::Queued)
            .filter(|transfer| !skip.contains(&transfer.id))
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

    /// Drop every failed row.
    ///
    /// Deliberately not folded into [`Self::clear_finished`] by making `Failed`
    /// finished. A failed row carries a `Retry` button, so clearing it with the
    /// done ones would throw away the thing the user is about to act on; this
    /// is the other verb, asked for by name, and the two stay apart because
    /// only one of them is destructive of a choice.
    pub async fn clear_failed(&self) -> Vec<String> {
        let removed: Vec<String> = {
            let mut items = self.items.write().await;
            let doomed: Vec<String> = items
                .values()
                .filter(|transfer| transfer.status == Status::Failed)
                .map(|transfer| transfer.id.clone())
                .collect();
            for id in &doomed {
                items.remove(id);
            }
            doomed
        };
        if !removed.is_empty() {
            self.persist().await;
        }
        removed
    }

    /// Put every failed row back in the queue.
    ///
    /// One write rather than one per row: the queue this exists for is the one
    /// with hundreds of failures in it, and a retry that rewrote a megabyte per
    /// row would cost more than the transfers.
    ///
    /// The error and the note go with the status, because both describe the
    /// attempt that failed and neither is true of the row that is about to be
    /// tried again.
    pub async fn requeue_failed(&self) -> Vec<Transfer> {
        let changed = {
            let mut items = self.items.write().await;
            let mut changed = Vec::new();
            for transfer in items.values_mut() {
                if transfer.status != Status::Failed {
                    continue;
                }
                transfer.status = Status::Queued;
                transfer.error = None;
                transfer.note = None;
                changed.push(transfer.clone());
            }
            changed
        };
        for transfer in &changed {
            let _ = self.events.send(QueueEvent {
                transfer: transfer.clone(),
            });
        }
        if !changed.is_empty() {
            self.persist().await;
        }
        changed
    }

    /// Say why several queued rows are not moving, writing the file once.
    ///
    /// The worker finds these one at a time as it walks the queue, and a
    /// restored queue against a share nothing has connected yet is every row
    /// at once. One [`Self::update`] each would rewrite the whole file per row
    /// on the way up, which is the boot cost `2b16451` measured and removed.
    ///
    /// Only `Queued` rows are touched, and only when the note is not already
    /// what it would be set to — so the second pass over a queue that is still
    /// waiting writes nothing and tells nobody.
    pub async fn note_all(&self, ids: &[String], note: &str) -> usize {
        let changed = {
            let mut items = self.items.write().await;
            let mut changed = Vec::new();
            for id in ids {
                let Some(transfer) = items.get_mut(id) else {
                    continue;
                };
                if transfer.status != Status::Queued || transfer.note.as_deref() == Some(note) {
                    continue;
                }
                transfer.note = Some(note.to_owned());
                changed.push(transfer.clone());
            }
            changed
        };
        for transfer in &changed {
            let _ = self.events.send(QueueEvent {
                transfer: transfer.clone(),
            });
        }
        if !changed.is_empty() {
            self.persist().await;
        }
        changed.len()
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

    /// Tell the window, and write the file only when the window is not the
    /// only one that needed telling.
    async fn announce(&self, transfer: Transfer, durable: bool) {
        let _ = self.events.send(QueueEvent { transfer });
        if durable {
            self.persist().await;
        }
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

/// One row with everything a restart does not need blanked out.
///
/// Compared either side of an update to decide whether the file has to be
/// rewritten. The four fields blanked here are exactly the ones [`restore`]
/// either discards or cannot use: `rate` and `eta_seconds` describe a process
/// that will not exist, and `bytes_done` and `files_done` describe progress a
/// restored row re-derives — it comes back as `Queued`, and what it does not
/// have to move again is the resume journal's answer, not this file's.
///
/// A whole clone rather than a tuple of the fields that *do* matter, so that a
/// field added to [`Transfer`] later is durable until somebody says otherwise.
/// That is the safe direction to be wrong in: the cost of a needless write is a
/// write, and the cost of a missed one is a row that does not come back.
fn durable(transfer: &Transfer) -> Transfer {
    Transfer {
        bytes_done: 0,
        files_done: 0,
        rate: None,
        eta_seconds: None,
        ..transfer.clone()
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
