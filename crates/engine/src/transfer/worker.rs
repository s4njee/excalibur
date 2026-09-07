//! The worker: what schedules transfers and what settles their rows.
//!
//! It no longer moves any bytes. Until E22-S2 this file held a chunk loop —
//! `download`, `stream` and `upload` — modelled on the sibling Kestrel engine's
//! worker. Every transfer now runs xsync's planner instead, through one of two
//! adapters ([`super::xsync_engine`] and [`super::endpoint`]), and what is left
//! here is the part that was always this file's own: the queue, the permit, the
//! retry, and the rule about who is allowed to write a row's status.
//!
//! Two of Kestrel's sharp edges were designed out here and are worth keeping
//! written down, because they are now *xsync's* to keep rather than this file's
//! and both are kept more strongly than they were:
//!
//! - **Nothing appears under its final name until every byte is there, and the
//!   bytes are checked.** Kestrel flushes and renames, so a download survives a
//!   crash of the app but not a power loss. The staged write path verifies each
//!   range against the source's own BLAKE3 before it lands and syncs the stage
//!   before it publishes.
//! - **A resumed transfer never appends to bytes it cannot account for.**
//!   Kestrel trusts whatever `<dest>.part` happens to be, so a later transfer to
//!   the same destination silently appends to a stranger's. This queue used to
//!   answer that with a recorded byte count; the resume journal answers it with
//!   a per-range record keyed on the source's identity, and refuses to reuse
//!   anything it cannot prove.
//!
//! The two that are still this file's:
//!
//! - **State changes go through the queue's closure-under-lock**, so a worker
//!   reporting progress cannot overwrite a status the user set while a chunk
//!   was in flight. Kestrel's check-then-act on a `Mutex<State>` loses that
//!   race.
//! - **The window can ask for the whole queue.** Progress rides a broadcast
//!   channel whose lag is silent, so there has to be a way back to the truth;
//!   Kestrel has none and its UI can desync permanently.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::{RwLock, Semaphore};

use super::queue::TransferQueue;
use super::rate::{backoff, Progress};
use super::xsync_engine::{Moved, Settled, XsyncRoute};
use super::{Direction, Status, Transfer};
use crate::backend::ShareBackend;
use crate::error::Result;

/// How many ranged reads a transfer keeps in flight against one file (E13-S7).
///
/// One chunk at a time makes a transfer round-trip bound: the link is idle
/// from the moment a chunk arrives until the next request reaches the server,
/// and no chunk size fixes that. Depth does, and it is the only number here
/// chosen from measurement rather than argument.
///
/// Four, because that is where the curve flattens: on a gigabit link to a
/// Raspberry Pi 5, 1 GiB came down at 27.6 MB/s at depth 1, 43.5 at 2, 66.1 at
/// 4 — and then 61.5, 62.7 and 59.9 at 6, 8 and 16. Past the plateau, depth
/// buys nothing and costs something: a megabyte of buffer per read in flight
/// per running transfer, and a longer wait for a cancel, which finishes the
/// window it has already asked for. See `docs/benchmarks.md`.
const DEFAULT_READ_DEPTH: usize = 4;

/// Attempts before a transient failure is reported as a failure.
const MAX_ATTEMPTS: u32 = 5;

/// What a queued row says while the share it names is not connected.
///
/// A note rather than an error, because nothing has gone wrong: mounts are made
/// when a window connects, so every row restored from the last run names a
/// share that nobody has mounted *yet*. Reported as a failure — which is what
/// this used to do — one launch turned a queue of 261 rows into 260 identical
/// `connection lost: … is not connected` failures before a window existed.
pub const WAITING_FOR_SHARE: &str = "waiting for its share";

/// Where the worker gets a backend from.
///
/// A trait rather than the `SessionManager` itself so the worker can be tested
/// without a session, and so a transfer can be re-pointed at a reconnected
/// mount without the worker knowing that happened.
#[async_trait]
pub trait BackendSource: Send + Sync {
    /// The backend to move bytes through, or why there is none.
    async fn active(&self) -> Result<Arc<dyn ShareBackend>>;

    /// The backend for one named mount, falling back to [`Self::active`].
    ///
    /// A row carries its mount since E24-S2, because a drag between two windows
    /// enqueues a transfer on a share that is *not* the one the app most
    /// recently touched. The default keeps every caller that has only one mount
    /// — the CLI, the worker's own tests — saying nothing about it.
    async fn backend_for(&self, mount: Option<&str>) -> Result<Arc<dyn ShareBackend>> {
        let _ = mount;
        self.active().await
    }

    /// Whether the share a row names can be reached at all right now.
    ///
    /// Asked *before* a transfer is claimed, and a `false` leaves it queued
    /// rather than failing it: a share is mounted when a window connects, so at
    /// launch there are none, and a restored queue that started itself against
    /// nothing produced one identical failure per row.
    ///
    /// `true` by default, and the default is the interesting part: a source
    /// with one backend and no notion of mounts — the CLI, this file's own
    /// tests — has nothing to be waiting for, and must not have its
    /// [`Self::active`] called an extra time to say so.
    async fn is_connected(&self, mount: Option<&str>) -> bool {
        let _ = mount;
        true
    }

    /// How to reach `mount` as an xsync export, when it is one (E22-S1).
    ///
    /// `Some` is what routes a transfer through xsync's own engine instead of
    /// the endpoint adapter. It is a separate question from
    /// [`Self::backend_for`] because the answers are different objects: the
    /// backend is a live v3 browse session, and a transfer needs a *second*
    /// process speaking the v1/v2 sync grammar — one `xs --server` serves one
    /// session and the role is fixed at the handshake.
    ///
    /// The default is `None`, so a caller with no session manager behind it —
    /// the worker's own tests, a mock — takes the endpoint adapter without
    /// having to say anything about xsync.
    async fn xsync_route(&self, mount: Option<&str>) -> Option<XsyncRoute> {
        let _ = mount;
        None
    }
}

/// Runs transfers from a queue.
pub struct Worker {
    queue: Arc<TransferQueue>,
    backends: Arc<dyn BackendSource>,
    /// Set when a transfer should stop; turned into the engine's cancel token.
    stopping: Arc<RwLock<HashMap<String, Arc<AtomicBool>>>>,
    limit: Arc<Semaphore>,
    /// What the limit is *meant* to be, which the semaphore may not have
    /// reached yet — see [`Worker::set_concurrency`].
    nominal: Arc<AtomicUsize>,
    /// Permits still to be taken away once running transfers give them back.
    ///
    /// Lowering the limit cannot cancel work already in flight, so a shrink is
    /// applied to whatever is free now and the rest is owed. Paying it down at
    /// acquisition time is what makes the new limit real without interrupting
    /// a transfer the user did not ask to stop.
    debt: Arc<AtomicUsize>,
    /// Ranged reads in flight per transfer (E13-S7).
    ///
    /// Per transfer rather than per worker: the point is to keep one file's
    /// link full, and a queue running three transfers already has three of
    /// these going at once. It reaches the engine as the source's
    /// `read_concurrency`, which is where the depth is now spent.
    read_depth: Arc<AtomicUsize>,
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
            nominal: Arc::new(AtomicUsize::new(concurrency)),
            debt: Arc::new(AtomicUsize::new(0)),
            read_depth: Arc::new(AtomicUsize::new(DEFAULT_READ_DEPTH)),
        }
    }

    /// Change how many ranged reads a transfer keeps in flight.
    ///
    /// Applies to transfers that start after it, not to one already running:
    /// the depth is read once when a session begins, so a change mid-file
    /// cannot leave the in-flight window inconsistent with the plan that built
    /// it. Zero would mean scheduling nothing, so it is clamped to one — which
    /// is also how a caller asks for the old serial behaviour.
    pub fn set_read_depth(&self, depth: usize) {
        self.read_depth.store(depth.max(1), Ordering::SeqCst);
    }

    /// Ranged reads in flight per transfer, as it stands.
    #[must_use]
    pub fn read_depth(&self) -> usize {
        self.read_depth.load(Ordering::SeqCst).max(1)
    }

    /// Change how many transfers may run at once (E17-S1).
    ///
    /// Raising takes effect immediately. Lowering takes effect as running
    /// transfers finish: a permit already held cannot be recalled without
    /// stopping work the user did not ask to stop, so the shortfall is
    /// remembered and taken out of the permits that come back.
    ///
    /// A limit of zero would mean nothing ever runs, which is a way to hang
    /// the queue rather than a setting, so it is clamped to one.
    pub fn set_concurrency(&self, limit: usize) {
        let limit = limit.max(1);
        let previous = self.nominal.swap(limit, Ordering::SeqCst);
        if limit > previous {
            // Cancel any outstanding shrink first, so lowering and raising
            // again nets out rather than compounding.
            let owed = self.debt.swap(0, Ordering::SeqCst);
            let granted = limit - previous;
            if granted > owed {
                self.limit.add_permits(granted - owed);
            } else {
                self.debt.store(owed - granted, Ordering::SeqCst);
            }
        } else if limit < previous {
            let wanted = previous - limit;
            let forgotten = self.limit.forget_permits(wanted);
            self.debt.fetch_add(wanted - forgotten, Ordering::SeqCst);
        }
    }

    /// The limit as it stands, for tests and for the window to read back.
    #[must_use]
    pub fn concurrency(&self) -> usize {
        self.nominal.load(Ordering::SeqCst)
    }

    /// Take back permits owed by an earlier shrink, as far as possible now.
    fn settle_debt(&self) {
        let owed = self.debt.swap(0, Ordering::SeqCst);
        if owed == 0 {
            return;
        }
        let forgotten = self.limit.forget_permits(owed);
        self.debt.fetch_add(owed - forgotten, Ordering::SeqCst);
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

    /// Whether a session for this row is still open.
    ///
    /// The registration [`Self::stop`] signals through, read as a fact: it is
    /// inserted before the first attempt and removed after the last one, so it
    /// answers "has the engine let go of this transfer's files yet", which is
    /// not the same question as the row's status. A cancel writes `Cancelled`
    /// on the row immediately and the session then drains what is in flight;
    /// anything that has to sweep the destination — a promised drag's staging
    /// directory (E24-S1) — has to wait for *this* rather than for the status.
    pub async fn is_busy(&self, id: &str) -> bool {
        self.stopping.read().await.contains_key(id)
    }

    /// Run queued transfers until there are none left.
    ///
    /// Returns when the queue has nothing `Queued` it can act on; the caller
    /// re-invokes it when something is added, and when a mount arrives. Each
    /// transfer holds a permit for its whole life, so `concurrency` bounds what
    /// is moving rather than what is started.
    ///
    /// A row whose share is not connected is passed over rather than started:
    /// it stays `Queued` and is picked up by the drain that a mount reaching
    /// `Ready` triggers. Passing over a row that stays queued is how a loop
    /// reading "the next queued transfer" spins for ever, which is why the ones
    /// left behind are remembered here and excluded from the question.
    pub async fn drain(self: &Arc<Self>) {
        let mut running = Vec::new();
        let mut waiting: HashSet<String> = HashSet::new();
        while let Some(transfer) = self.queue.next_queued_except(&waiting).await {
            // HTTP downloads have no share to wait for. Checking the backend
            // here would leave them queued forever when the app has no mount.
            // Any row whose source is HTTP takes the HTTP route, including a
            // malformed one: it must attempt and fail loudly, not wait for an
            // unrelated share that will never arrive.
            let is_http = matches!(transfer.from_endpoint(), Some(super::Endpoint::Http { .. }));
            if !is_http && !self.backends.is_connected(transfer.mount.as_deref()).await {
                waiting.insert(transfer.id);
                continue;
            }
            // Before taking a permit, give back any the limit no longer
            // allows. Done here rather than on release because a permit is
            // returned by dropping it, which this type does not observe.
            self.settle_debt();
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
                        // Both describe the attempt that is over — the reason
                        // it stopped, or the reason it never started — and
                        // neither is true of the one beginning now.
                        item.note = None;
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
        // Once, at the end, rather than as each one is passed over: a queue
        // restored against a share nothing has mounted is every row waiting,
        // and `note_all` writes the file once for all of them. It also says
        // nothing at all on the second drain over the same still-waiting
        // queue, because the note is already what it would be set to.
        if !waiting.is_empty() {
            let ids: Vec<String> = waiting.into_iter().collect();
            self.queue.note_all(&ids, WAITING_FOR_SHARE).await;
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

    /// Move one transfer once, by whichever route its pair calls for.
    ///
    /// **Every transfer runs xsync's planner now (E22-S2).** What the routing
    /// rule decides is only *which* of the two adapters carries the bytes:
    ///
    /// - a share that is an xsync export, paired with a folder on this machine,
    ///   goes through [`super::xsync_engine`] — xsync's own wire protocol to a
    ///   second `xs --server`, with its compression and its multi-stream push;
    /// - everything else — NFS, SMB, SFTP, a local folder browsed as a mount,
    ///   and an xsync export whose engine could not be reached — goes through
    ///   [`super::endpoint`], which is `ShareBackend` implementing xsync's
    ///   `Source` and `Sink`.
    ///
    /// There is no fallback *between* the two once a route is chosen: a session
    /// that fails reports its failure rather than quietly re-running the
    /// transfer a slower way, because a transfer that succeeded by a route the
    /// user was not told about is a transfer whose progress, resume and
    /// verification claims were all about something else.
    ///
    /// E22-S1's carve-out for pulling a single file is gone, and this is where
    /// it went. `xs --server` cannot serve a *file* as the source of a pull —
    /// the scan advertises the entry and the read resolves it against the root
    /// again — so that pull used to fall back to a chunk loop that hashed
    /// nothing. It now takes the endpoint route instead, which is the same
    /// planner, the same verified write path and the same events as every other
    /// pair.
    async fn attempt(&self, transfer: &Transfer, stop: &AtomicBool) -> Result<()> {
        let flag = self.flag_for(transfer, stop).await;
        // Route HTTP before looking up a share. URL and endpoint validation is
        // repeated by the route at execution time so restored/deserialised
        // rows cannot bypass it.
        if matches!(transfer.from_endpoint(), Some(super::Endpoint::Http { .. })) {
            let (url, destination) = super::http::endpoints(transfer)?;
            let queue = Arc::clone(&self.queue);
            let id = transfer.id.clone();
            return self
                .settle_session(
                    transfer,
                    super::http::run(
                        url,
                        destination,
                        Arc::clone(&flag),
                        move |resolved| {
                            let name = resolved
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("download")
                                .to_owned();
                            tokio::spawn(async move {
                                queue
                                    .update(&id, |item| {
                                        item.name = name;
                                        item.dest_path = resolved.to_string_lossy().into_owned();
                                    })
                                    .await;
                            });
                        },
                        self.pump(transfer),
                    ),
                )
                .await;
        }
        if let Some(route) = self.backends.xsync_route(transfer.mount.as_deref()).await {
            if let Some(remote_is_dir) = self.routable(transfer).await {
                return self
                    .settle_session(
                        transfer,
                        super::xsync_engine::run(
                            &route,
                            transfer,
                            remote_is_dir,
                            Arc::clone(&flag),
                            self.pump(transfer),
                        ),
                    )
                    .await;
            }
        }
        let backend = self.backends.backend_for(transfer.mount.as_deref()).await?;
        let depth = self.read_depth();
        self.settle_session(
            transfer,
            super::endpoint::run(
                &backend,
                transfer,
                depth,
                Arc::clone(&flag),
                self.pump(transfer),
            ),
        )
        .await
    }

    /// The stop flag this transfer is already being watched through.
    ///
    /// `Worker::stop` sets it and nothing here needs a second channel; the
    /// fallback covers a caller that never registered one, which is only the
    /// case in a test that drives `attempt` directly.
    async fn flag_for(&self, transfer: &Transfer, stop: &AtomicBool) -> Arc<AtomicBool> {
        self.stopping.read().await.get(&transfer.id).map_or_else(
            || Arc::new(AtomicBool::new(stop.load(Ordering::SeqCst))),
            Arc::clone,
        )
    }

    /// Whether the xsync route can take this transfer, and what its far end is.
    ///
    /// `Some(remote_is_dir)` routes it through xsync's own engine; `None` sends
    /// it to the endpoint adapter. The one `None` is the single-file pull
    /// described on [`Self::attempt`].
    ///
    /// A `stat` that fails is treated as "not a directory", which sends a pull
    /// to the adapter — the path that will produce a real error for whatever is
    /// actually wrong with that file.
    async fn routable(&self, transfer: &Transfer) -> Option<bool> {
        if transfer.direction == Some(Direction::Up) {
            // The far end is the destination and the session's root is its
            // parent directory either way, so nothing has to be asked.
            return Some(false);
        }
        let backend = self
            .backends
            .backend_for(transfer.mount.as_deref())
            .await
            .ok()?;
        let is_dir = backend
            .stat(&transfer.source_path)
            .await
            .is_ok_and(|attrs| attrs.kind == crate::backend::EntryKind::Dir);
        is_dir.then_some(true)
    }

    /// A progress sink for one session, draining onto the row.
    ///
    /// Both routes report bytes on their own schedule and are already throttled
    /// by the session; this only writes it down, on a task so that the engine's
    /// thread never waits for the queue's lock.
    fn pump(&self, transfer: &Transfer) -> impl FnMut(Moved) + Send + 'static {
        let queue = Arc::clone(&self.queue);
        let id = transfer.id.clone();
        let (updates, mut written) = tokio::sync::mpsc::unbounded_channel::<Moved>();
        tokio::spawn(async move {
            let mut rate = Progress::new();
            while let Some(moved) = written.recv().await {
                let now = Instant::now();
                rate.sample(moved.bytes, now);
                let speed = rate.rate();
                let eta = moved
                    .planned
                    .and_then(|planned| rate.eta_seconds(moved.bytes, planned.bytes));
                queue
                    .update(&id, |item| {
                        if item.status != Status::Running {
                            return;
                        }
                        item.bytes_done = moved.bytes;
                        item.files_done = moved.files;
                        item.rate = speed;
                        item.eta_seconds = eta;
                        // `Planned` is where a folder row learns what it is:
                        // until the planner has scanned, nothing on this side
                        // knows how big the job is or how many files are in it,
                        // and a bar at zero for a whole transfer looks broken.
                        let Some(planned) = moved.planned else {
                            return;
                        };
                        if planned.bytes > 0 {
                            item.bytes_total = planned.bytes;
                        }
                        item.files_total = planned.files;
                    })
                    .await;
            }
        });
        move |moved| {
            let _ = updates.send(moved);
        }
    }

    /// Await one session and write what it settled onto the row.
    async fn settle_session(
        &self,
        transfer: &Transfer,
        session: impl std::future::Future<Output = Result<Settled>>,
    ) -> Result<()> {
        let settled = session.await?;
        if settled.stopped {
            // Paused or cancelled. The row's status is the user's and this
            // must not overwrite it.
            return Ok(());
        }
        self.queue
            .update(&transfer.id, |item| {
                if item.status != Status::Running {
                    return;
                }
                item.bytes_done = settled.bytes;
                item.verified = settled.verified;
                item.note.clone_from(&settled.note);
                // The last word on the count, for the same reason as the bytes:
                // the throttle may have dropped the tick that carried the final
                // file, and a folder row that finished on "1001 of 1002" would
                // be reporting a file it actually moved as missing.
                item.files_done = settled.files;
                if item.files_total < settled.files {
                    item.files_total = settled.files;
                }
            })
            .await;
        Ok(())
    }
}
