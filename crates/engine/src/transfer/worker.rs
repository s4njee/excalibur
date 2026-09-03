//! The worker: what actually moves the bytes.
//!
//! Modelled on the sibling Kestrel engine's worker, with four of its known
//! sharp edges designed out rather than inherited:
//!
//! - **Transferred data is fsynced before the rename.** Kestrel flushes and
//!   renames, so a download survives a crash of the app but not a power loss —
//!   the rename can land before the content does, leaving a file of zeroes
//!   with the right name.
//! - **A resumed download truncates to the length this queue recorded.**
//!   Kestrel trusts whatever `<dest>.part` happens to be, and keeps that file
//!   when a transfer is *cancelled*, so a later transfer to the same
//!   destination silently appends to a stranger's bytes. Cancelling here
//!   removes the partial, and resuming discards anything past the byte count
//!   this queue is prepared to vouch for.
//! - **State changes go through the queue's closure-under-lock**, so a worker
//!   reporting progress cannot overwrite a status the user set while a chunk
//!   was in flight. Kestrel's check-then-act on a `Mutex<State>` loses that
//!   race.
//! - **The window can ask for the whole queue.** Progress rides a broadcast
//!   channel whose lag is silent, so there has to be a way back to the truth;
//!   Kestrel has none and its UI can desync permanently.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{RwLock, Semaphore};

use super::queue::TransferQueue;
use super::rate::{backoff, Progress};
use super::{Direction, Status, Transfer};
use crate::backend::{ShareBackend, WriteMode};
use crate::error::{EngineError, Result};

/// Bytes moved per round trip.
///
/// Large enough that a fast link is not dominated by round trips, small enough
/// that pause and cancel take effect promptly — they are checked between
/// chunks, so this is also the granularity of "stop now".
const CHUNK: u32 = 1024 * 1024;

/// Attempts before a transient failure is reported as a failure.
const MAX_ATTEMPTS: u32 = 5;

/// Where the worker gets a backend from.
///
/// A trait rather than the `SessionManager` itself so the worker can be tested
/// without a session, and so a transfer can be re-pointed at a reconnected
/// mount without the worker knowing that happened.
#[async_trait]
pub trait BackendSource: Send + Sync {
    /// The backend to move bytes through, or why there is none.
    async fn active(&self) -> Result<Arc<dyn ShareBackend>>;
}

/// Runs transfers from a queue.
pub struct Worker {
    queue: Arc<TransferQueue>,
    backends: Arc<dyn BackendSource>,
    /// Set when a transfer should stop; read between chunks.
    stopping: Arc<RwLock<HashMap<String, Arc<AtomicBool>>>>,
    limit: Arc<Semaphore>,
}

impl Worker {
    /// A worker running at most `concurrency` transfers at once.
    ///
    /// # Panics
    ///
    /// Panics if `concurrency` is zero, which would never run anything.
    #[must_use]
    pub fn new(
        queue: Arc<TransferQueue>,
        backends: Arc<dyn BackendSource>,
        concurrency: usize,
    ) -> Self {
        assert!(concurrency > 0, "a worker must be able to run something");
        Self {
            queue,
            backends,
            stopping: Arc::new(RwLock::new(HashMap::new())),
            limit: Arc::new(Semaphore::new(concurrency)),
        }
    }

    /// Ask a running transfer to stop, and say whether it should keep its
    /// partial file.
    ///
    /// A pause keeps it — that is the resume point. A cancel removes it, so a
    /// later transfer to the same destination cannot append to bytes nobody
    /// asked for. That distinction is the whole reason this takes a status
    /// rather than a boolean.
    pub async fn stop(&self, id: &str, becomes: Status) {
        if let Some(flag) = self.stopping.read().await.get(id) {
            flag.store(true, Ordering::SeqCst);
        }
        self.queue
            .update(id, |transfer| {
                if transfer.status.active() {
                    transfer.status = becomes;
                    transfer.rate = None;
                    transfer.eta_seconds = None;
                }
            })
            .await;
    }

    /// Run queued transfers until there are none left.
    ///
    /// Returns when the queue has nothing `Queued`; the caller re-invokes it
    /// when something is added. Each transfer holds a permit for its whole
    /// life, so `concurrency` bounds what is moving rather than what is
    /// started.
    pub async fn drain(self: &Arc<Self>) {
        let mut running = Vec::new();
        while let Some(transfer) = self.queue.next_queued().await {
            let Ok(permit) = Arc::clone(&self.limit).acquire_owned().await else {
                return;
            };
            // Claimed before the task starts, so a second pass of this loop
            // cannot hand the same transfer to a second worker.
            let claimed = self
                .queue
                .update(&transfer.id, |item| {
                    if item.status == Status::Queued {
                        item.status = Status::Running;
                        item.error = None;
                    }
                })
                .await;
            if claimed.is_none_or(|item| item.status != Status::Running) {
                continue;
            }

            let worker = Arc::clone(self);
            running.push(tokio::spawn(async move {
                worker.run_one(&transfer.id).await;
                drop(permit);
            }));
        }
        for task in running {
            let _ = task.await;
        }
    }

    /// Move one transfer, retrying transient failures.
    async fn run_one(&self, id: &str) {
        let flag = Arc::new(AtomicBool::new(false));
        self.stopping
            .write()
            .await
            .insert(id.to_owned(), Arc::clone(&flag));

        let mut attempt = 0;
        let outcome = loop {
            let Some(transfer) = self.queue.update(id, |_| ()).await else {
                break Ok(());
            };
            if transfer.status != Status::Running {
                // Paused or cancelled between attempts.
                break Ok(());
            }
            match self.attempt(&transfer, &flag).await {
                Ok(()) => break Ok(()),
                Err(error) => {
                    attempt += 1;
                    // A transient failure is worth waiting out; anything else
                    // is the answer, and retrying it four more times only
                    // delays telling the user.
                    if !error.transient() || attempt >= MAX_ATTEMPTS {
                        break Err(error);
                    }
                    tokio::time::sleep(backoff(attempt)).await;
                }
            }
        };

        self.stopping.write().await.remove(id);
        self.queue
            .update(id, |transfer| {
                // Only settle a transfer that is still running: a pause or a
                // cancel that landed while the last chunk was in flight is the
                // user's decision and outranks this one.
                if transfer.status != Status::Running {
                    return;
                }
                transfer.rate = None;
                transfer.eta_seconds = None;
                match &outcome {
                    Ok(()) => {
                        transfer.status = Status::Done;
                        if transfer.bytes_total == 0 {
                            transfer.bytes_total = transfer.bytes_done;
                        }
                    }
                    Err(error) => {
                        transfer.status = Status::Failed;
                        transfer.error = Some(error.to_string());
                    }
                }
            })
            .await;
    }

    async fn attempt(&self, transfer: &Transfer, stop: &AtomicBool) -> Result<()> {
        let backend = self.backends.active().await?;
        match transfer.direction {
            Direction::Down => self.download(transfer, &backend, stop).await,
            Direction::Up => self.upload(transfer, &backend, stop).await,
        }
    }

    /// Share to disk: `<dest>.part`, fsync, rename.
    async fn download(
        &self,
        transfer: &Transfer,
        backend: &Arc<dyn ShareBackend>,
        stop: &AtomicBool,
    ) -> Result<()> {
        let destination = PathBuf::from(&transfer.dest_path);
        let partial = partial_path(&destination);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                EngineError::Io(format!("create {}: {error}", parent.display()))
            })?;
        }

        // Resume only as far as this queue is prepared to vouch for. Anything
        // beyond `bytes_done` in that file was written by something whose
        // record we do not have, and appending to it would corrupt the result.
        let mut offset = resume_offset(&partial, transfer.bytes_done)?;
        let total = transfer.bytes_total;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&partial)
            .map_err(|error| EngineError::Io(format!("open {}: {error}", partial.display())))?;
        file.set_len(offset)
            .map_err(|error| EngineError::Io(format!("truncate {}: {error}", partial.display())))?;
        seek_to(&mut file, offset, &partial)?;

        let mut progress = Progress::new();
        progress.sample(offset, Instant::now());
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let bytes = backend
                .read_range(&transfer.source_path, offset, CHUNK)
                .await?;
            if bytes.is_empty() {
                break;
            }
            file.write_all(&bytes).map_err(|error| {
                EngineError::Io(format!("write {}: {error}", partial.display()))
            })?;
            offset += bytes.len() as u64;
            self.publish(transfer, &mut progress, offset, total).await;
            if total > 0 && offset >= total {
                break;
            }
        }

        // Before the rename, not after: a rename that lands before the content
        // leaves a correctly named file full of zeroes.
        file.sync_all()
            .map_err(|error| EngineError::Io(format!("sync {}: {error}", partial.display())))?;
        drop(file);
        std::fs::rename(&partial, &destination).map_err(|error| {
            EngineError::Io(format!("publish {}: {error}", destination.display()))
        })?;
        self.settle(transfer, offset).await;
        Ok(())
    }

    /// Disk to share.
    async fn upload(
        &self,
        transfer: &Transfer,
        backend: &Arc<dyn ShareBackend>,
        stop: &AtomicBool,
    ) -> Result<()> {
        let source = PathBuf::from(&transfer.source_path);
        let contents = std::fs::read(&source)
            .map_err(|error| EngineError::Io(format!("read {}: {error}", source.display())))?;
        let total = contents.len() as u64;

        // Resume where the backend supports it; otherwise start over rather
        // than assume the remote holds what we last sent.
        let resumable = backend.caps().resume_write;
        let mut offset = if resumable {
            transfer.bytes_done.min(total)
        } else {
            0
        };

        let mut progress = Progress::new();
        progress.sample(offset, Instant::now());
        let mut first = true;
        while offset < total || first {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let end = total.min(offset + u64::from(CHUNK));
            let slice = &contents[usize::try_from(offset).unwrap_or(0)
                ..usize::try_from(end).unwrap_or(contents.len())];
            // The first write truncates; every later one continues. An upload
            // that resumed with `Create` would discard what it was resuming.
            let mode = if first && offset == 0 {
                WriteMode::Create
            } else {
                WriteMode::Resume { offset }
            };
            backend
                .write_range(&transfer.dest_path, mode, slice)
                .await?;
            offset = end;
            first = false;
            self.publish(transfer, &mut progress, offset, total).await;
        }
        self.settle(transfer, offset).await;
        Ok(())
    }

    /// Record progress, at most ten times a second per transfer.
    async fn publish(&self, transfer: &Transfer, progress: &mut Progress, done: u64, total: u64) {
        if !progress.sample(done, Instant::now()) {
            return;
        }
        let rate = progress.rate();
        let eta = progress.eta_seconds(done, total);
        self.queue
            .update(&transfer.id, |item| {
                if item.status != Status::Running {
                    return;
                }
                item.bytes_done = done;
                item.rate = rate;
                item.eta_seconds = eta;
            })
            .await;
    }

    /// Record the final byte count, whatever the throttle last published.
    async fn settle(&self, transfer: &Transfer, done: u64) {
        self.queue
            .update(&transfer.id, |item| {
                if item.status == Status::Running {
                    item.bytes_done = done;
                }
            })
            .await;
    }
}

/// The partial a download accumulates in.
fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    destination.with_file_name(name)
}

/// How far a download may resume, given what the queue recorded.
///
/// Never past `recorded`. A `.part` longer than the queue's own record was
/// written by something whose bytes we cannot account for — a cancelled
/// transfer to the same destination, or a different file entirely — and
/// appending to it produces a file that passes every length check and is
/// wrong. Kestrel trusts the file's length here; that is its bug 3.
fn resume_offset(partial: &Path, recorded: u64) -> Result<u64> {
    match std::fs::metadata(partial) {
        Ok(metadata) => Ok(metadata.len().min(recorded)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(EngineError::Io(format!(
            "inspect {}: {error}",
            partial.display()
        ))),
    }
}

fn seek_to(file: &mut std::fs::File, offset: u64, path: &Path) -> Result<()> {
    use std::io::Seek as _;
    file.seek(std::io::SeekFrom::Start(offset))
        .map(|_| ())
        .map_err(|error| EngineError::Io(format!("seek {}: {error}", path.display())))
}
