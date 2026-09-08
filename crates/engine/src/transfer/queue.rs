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

use tokio::sync::{broadcast, Notify, RwLock};

use super::{NamedQueue, Status, Transfer, DEFAULT_QUEUE_ID};
use crate::error::{EngineError, Result};

/// A change worth telling the window about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEvent {
    /// The transfer that changed.
    pub transfer: Transfer,
}

/// A durable change to the named queue definitions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDefinitionsEvent {
    pub queues: Vec<NamedQueue>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedQueue {
    #[serde(default)]
    transfers: Vec<Transfer>,
    #[serde(default)]
    queues: Vec<NamedQueue>,
}

/// Every transfer the app knows about.
pub struct TransferQueue {
    items: Arc<RwLock<HashMap<String, Transfer>>>,
    queues: Arc<RwLock<HashMap<String, NamedQueue>>>,
    running: Arc<RwLock<HashMap<String, usize>>>,
    queue_notify: Arc<Notify>,
    events: broadcast::Sender<QueueEvent>,
    queue_events: broadcast::Sender<QueueDefinitionsEvent>,
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
        let (restored, queues) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| {
                serde_json::from_str::<PersistedQueue>(&text)
                    .map(|state| (state.transfers, state.queues))
                    .or_else(|_| {
                        serde_json::from_str::<Vec<Transfer>>(&text).map(|rows| (rows, Vec::new()))
                    })
                    .ok()
            })
            .unwrap_or_default();
        Self::from_parts(restore(restored), queues, Some(path))
    }

    /// An in-memory queue, for tests.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::from_parts(HashMap::new(), Vec::new(), None)
    }

    fn from_parts(
        mut items: HashMap<String, Transfer>,
        queues: Vec<NamedQueue>,
        path: Option<PathBuf>,
    ) -> Self {
        let queues = normalize_queues(queues);
        normalize_positions(&mut items, &queues);
        let (events, _) = broadcast::channel(256);
        let (queue_events, _) = broadcast::channel(64);
        Self {
            items: Arc::new(RwLock::new(items)),
            queues: Arc::new(RwLock::new(queues)),
            running: Arc::new(RwLock::new(HashMap::new())),
            queue_notify: Arc::new(Notify::new()),
            events,
            queue_events,
            path,
        }
    }

    /// Subscribe to queue changes.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<QueueEvent> {
        self.events.subscribe()
    }

    /// Subscribe to named queue changes.
    #[must_use]
    pub fn queue_events(&self) -> broadcast::Receiver<QueueDefinitionsEvent> {
        self.queue_events.subscribe()
    }

    /// Every named queue, in execution order.
    pub async fn queues(&self) -> Vec<NamedQueue> {
        let mut queues: Vec<NamedQueue> = self.queues.read().await.values().cloned().collect();
        queues.sort_by_key(|queue| (queue.order, queue.id.clone()));
        queues
    }

    /// Add a named queue at the end of the queue list.
    ///
    /// # Errors
    /// Returns an error when the name or colour is empty or exceeds its bound.
    pub async fn create_queue(
        &self,
        name: impl Into<String>,
        color: impl Into<String>,
        concurrency: usize,
    ) -> Result<NamedQueue> {
        let name = validate_queue_name(&name.into())?;
        let color = validate_queue_color(&color.into())?;
        let concurrency = concurrency.max(1);
        let mut queues = self.queues.write().await;
        let order = queues
            .values()
            .map(|queue| queue.order)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let queue = NamedQueue {
            id: next_queue_id(&queues),
            name,
            color,
            concurrency,
            order,
        };
        queues.insert(queue.id.clone(), queue.clone());
        drop(queues);
        self.persist().await;
        self.announce_queues().await;
        Ok(queue)
    }

    /// Change a queue's label, colour, or row limit.
    ///
    /// # Errors
    /// Returns an error when the name or colour is empty or exceeds its bound.
    pub async fn update_queue(
        &self,
        id: &str,
        name: impl Into<String>,
        color: impl Into<String>,
        concurrency: usize,
    ) -> Result<Option<NamedQueue>> {
        let name = validate_queue_name(&name.into())?;
        let color = validate_queue_color(&color.into())?;
        let mut queues = self.queues.write().await;
        let Some(queue) = queues.get_mut(id) else {
            return Ok(None);
        };
        queue.name = name;
        queue.color = color;
        queue.concurrency = concurrency.max(1);
        let queue = queue.clone();
        drop(queues);
        self.persist().await;
        self.announce_queues().await;
        self.queue_notify.notify_one();
        Ok(Some(queue))
    }

    /// Apply the visible order of the named queues.
    pub async fn reorder_queues(&self, ids: &[String]) -> bool {
        let mut queues = self.queues.write().await;
        let mut order = 0;
        for id in ids {
            let Some(queue) = queues.get_mut(id) else {
                continue;
            };
            queue.order = order;
            order = order.saturating_add(1);
        }
        drop(queues);
        self.persist().await;
        self.announce_queues().await;
        self.queue_notify.notify_one();
        true
    }

    /// Remove a named queue, moving its rows to the built-in queue.
    pub async fn delete_queue(&self, id: &str) -> Option<NamedQueue> {
        if id == DEFAULT_QUEUE_ID {
            return None;
        }
        let removed = self.queues.write().await.remove(id);
        removed.as_ref()?;
        let default = DEFAULT_QUEUE_ID.to_owned();
        let mut items = self.items.write().await;
        let next = items
            .values()
            .filter(|transfer| transfer.queue_id == default)
            .filter_map(|transfer| transfer.queue_position)
            .max()
            .unwrap_or(0);
        let mut position = next;
        for transfer in items
            .values_mut()
            .filter(|transfer| transfer.queue_id == id)
        {
            transfer.queue_id.clone_from(&default);
            position = position.saturating_add(1);
            transfer.queue_position = Some(position);
            let _ = self.events.send(QueueEvent {
                transfer: transfer.clone(),
            });
        }
        drop(items);
        self.persist().await;
        self.announce_queues().await;
        self.queue_notify.notify_one();
        removed
    }

    /// Move one row to a named queue, appending it at that queue's tail.
    pub async fn move_to_queue(&self, id: &str, queue_id: &str) -> Option<Transfer> {
        if !self.queues.read().await.contains_key(queue_id) {
            return None;
        }
        let mut items = self.items.write().await;
        let position = items
            .values()
            .filter(|transfer| transfer.queue_id == queue_id)
            .filter_map(|transfer| transfer.queue_position)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let transfer = items.get_mut(id)?;
        transfer.queue_id = queue_id.to_owned();
        transfer.queue_position = Some(position);
        let transfer = transfer.clone();
        drop(items);
        self.announce(transfer.clone(), true).await;
        self.queue_notify.notify_one();
        Some(transfer)
    }

    /// Apply the visible order of rows in one named queue.
    pub async fn reorder_queue(&self, queue_id: &str, ids: &[String]) -> bool {
        if !self.queues.read().await.contains_key(queue_id) {
            return false;
        }
        let mut items = self.items.write().await;
        let mut position = 0;
        for id in ids {
            let Some(transfer) = items.get_mut(id) else {
                continue;
            };
            if transfer.queue_id != queue_id {
                continue;
            }
            transfer.queue_position = Some(position);
            position = position.saturating_add(1);
            let _ = self.events.send(QueueEvent {
                transfer: transfer.clone(),
            });
        }
        drop(items);
        self.persist().await;
        true
    }

    /// Atomically reserve one running row in a named queue.
    pub async fn try_claim_queue(&self, queue_id: &str) -> bool {
        self.try_claim_queue_with_ceiling(queue_id, usize::MAX)
            .await
    }

    /// Reserve a row while treating the built-in queue's configured value as
    /// the worker-wide setting. That keeps the existing global preference in
    /// force for legacy rows while named queues retain their own limits.
    pub async fn try_claim_queue_with_ceiling(&self, queue_id: &str, ceiling: usize) -> bool {
        let limit = self.queues.read().await.get(queue_id).map_or(1, |queue| {
            if queue_id == DEFAULT_QUEUE_ID {
                ceiling.max(1)
            } else {
                queue.concurrency.max(1)
            }
        });
        let mut running = self.running.write().await;
        let active = running.entry(queue_id.to_owned()).or_default();
        if *active >= limit {
            return false;
        }
        *active += 1;
        true
    }

    /// Release a previously claimed row.
    pub async fn release_queue(&self, queue_id: &str) {
        let mut running = self.running.write().await;
        if let Some(active) = running.get_mut(queue_id) {
            *active = active.saturating_sub(1);
        }
        self.queue_notify.notify_one();
    }

    /// Wait until a running row releases a named queue slot or its policy
    /// changes. This keeps a queue capped without polling or recursive drains.
    pub async fn wait_for_queue_change(&self) {
        self.queue_notify.notified().await;
    }

    /// Every transfer, oldest first.
    pub async fn all(&self) -> Vec<Transfer> {
        let mut all: Vec<Transfer> = self.items.read().await.values().cloned().collect();
        let queues = self.queues.read().await;
        all.sort_by_key(|transfer| {
            (
                queues
                    .get(&transfer.queue_id)
                    .map_or(u64::MAX, |queue| queue.order),
                transfer.queue_position.unwrap_or(u64::MAX),
                transfer.queued_at_ms,
                transfer.id.clone(),
            )
        });
        all
    }

    /// Add a transfer and announce it.
    pub async fn add(&self, transfer: Transfer) {
        let mut transfer = transfer;
        if !self.queues.read().await.contains_key(&transfer.queue_id) {
            DEFAULT_QUEUE_ID.clone_into(&mut transfer.queue_id);
        }
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
        let queues = self.queues.read().await;
        items
            .values()
            .filter(|transfer| transfer.status == Status::Queued)
            .filter(|transfer| !skip.contains(&transfer.id))
            .min_by_key(|transfer| {
                (
                    queues
                        .get(&transfer.queue_id)
                        .map_or(u64::MAX, |queue| queue.order),
                    transfer.queue_position.unwrap_or(u64::MAX),
                    transfer.queued_at_ms,
                    transfer.id.clone(),
                )
            })
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
        let state = PersistedQueue {
            transfers: self.all().await,
            queues: self.queues().await,
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else {
            return;
        };
        let _ = write_atomically(path, &text);
    }

    async fn announce_queues(&self) {
        let _ = self.queue_events.send(QueueDefinitionsEvent {
            queues: self.queues().await,
        });
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
        progress: None,
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
            // Lane detail belongs to the process that was moving the row. A
            // restored transfer gets a fresh snapshot from its next attempt.
            transfer.progress = None;
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

fn normalize_queues(queues: Vec<NamedQueue>) -> HashMap<String, NamedQueue> {
    let mut normalized: HashMap<String, NamedQueue> = queues
        .into_iter()
        .filter(|queue| !queue.id.is_empty())
        .map(|queue| (queue.id.clone(), queue))
        .collect();
    normalized
        .entry(DEFAULT_QUEUE_ID.to_owned())
        .or_insert_with(NamedQueue::default_queue);
    normalized
}

fn normalize_positions(
    items: &mut HashMap<String, Transfer>,
    queues: &HashMap<String, NamedQueue>,
) {
    for transfer in items.values_mut() {
        if !queues.contains_key(&transfer.queue_id) {
            DEFAULT_QUEUE_ID.clone_into(&mut transfer.queue_id);
        }
    }
}

fn validate_queue_name(name: &str) -> Result<String> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(EngineError::Unsupported(
            "queue name must contain between 1 and 80 characters".to_owned(),
        ));
    }
    Ok(name)
}

fn validate_queue_color(color: &str) -> Result<String> {
    let color = color.trim().to_owned();
    if color.is_empty() || color.chars().count() > 32 {
        return Err(EngineError::Unsupported(
            "queue colour must contain between 1 and 32 characters".to_owned(),
        ));
    }
    Ok(color)
}

fn next_queue_id(queues: &HashMap<String, NamedQueue>) -> String {
    let mut index = queues.len();
    loop {
        let id = format!("queue-{index}");
        if !queues.contains_key(&id) {
            return id;
        }
        index = index.saturating_add(1);
    }
}
