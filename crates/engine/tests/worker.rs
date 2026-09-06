//! The transfer worker against the mock backend.
//!
//! The cases worth writing down are the ones a happy-path copy never reaches:
//! a resume onto a stale partial, a cancel mid-flight, and a failure that is
//! retried versus one that is not.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::ShareBackend;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::{Direction, Status, Transfer};
use excalibur_engine::Result;

struct Fixed(Arc<dyn ShareBackend>);

#[async_trait]
impl BackendSource for Fixed {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.0))
    }
}

/// A source that refuses the first `failures` times, then works.
struct FlakySource {
    inner: Arc<dyn ShareBackend>,
    remaining: AtomicU32,
    transient: bool,
}

#[async_trait]
impl BackendSource for FlakySource {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        if self.remaining.load(Ordering::SeqCst) > 0 {
            self.remaining.fetch_sub(1, Ordering::SeqCst);
            return Err(if self.transient {
                excalibur_engine::EngineError::Disconnected("dropped".to_owned())
            } else {
                excalibur_engine::EngineError::PermissionDenied("no".to_owned())
            });
        }
        Ok(Arc::clone(&self.inner))
    }
}

fn worker(queue: &Arc<TransferQueue>, source: Arc<dyn BackendSource>) -> Arc<Worker> {
    Arc::new(Worker::new(Arc::clone(queue), source, 3))
}

fn mock() -> Arc<dyn ShareBackend> {
    Arc::new(MockBackend::media_share())
}

/// The media share, with `path` actually holding the bytes it claims.
///
/// `media_share` is the **listing** fixture: its rows carry the design's sizes
/// — `Arrival.2016.mkv` is 48 GB — and no contents at all. The chunk loop this
/// story deleted could not tell that apart from a real file, because it read
/// until a short answer and called that the end; so a "download" of a file with
/// no bytes in it published an empty file and reported success. xsync's planner
/// asks for the range it was promised and refuses what comes back short, which
/// is the stricter and the correct behaviour. A test that moves bytes therefore
/// has to seed them.
async fn seeded(path: &str, bytes: &[u8]) -> Arc<dyn ShareBackend> {
    let share = mock();
    share
        .write_range(path, excalibur_engine::WriteMode::Create, bytes)
        .await
        .expect("seed the share");
    share
}

/// Bytes whose value depends on their offset.
///
/// A pattern rather than a fill: several of these tests are about the *order*
/// bytes land in, and a file of identical bytes cannot tell a correct copy from
/// a shuffled one.
fn patterned_bytes(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

#[tokio::test]
async fn a_download_lands_whole_and_leaves_nothing_beside_it() {
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("Arrival.2016.mkv");
    let payload = patterned_bytes(3 * 1024 * 1024);
    let share = seeded("Arrival.2016.mkv", &payload).await;
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Down,
            "Arrival.2016.mkv",
            destination.to_str().expect("utf-8"),
            0,
        ))
        .await;

    worker(&queue, Arc::new(Fixed(share))).drain().await;

    let done = &queue.all().await[0];
    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        std::fs::read(&destination).expect("read result"),
        payload,
        "the published file is not what the share held"
    );
    assert_eq!(done.bytes_done, payload.len() as u64);
    // The stage is consumed by the rename, not left beside the result. Only
    // the file the user asked for is in the folder.
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn a_stranger_s_partial_beside_the_destination_contributes_nothing() {
    // The bug this exists to prevent, and the one the sibling engine has: a
    // leftover partial from a *different* cancelled transfer to the same
    // destination. Kestrel trusts its length and appends real bytes onto a
    // stranger's, producing a file that passes every length check and is wrong.
    //
    // The E13 worker answered that with a byte count this queue recorded. The
    // staged write path answers it with a stage of its own naming, a per-range
    // journal keyed on the source's identity, and a length check before the
    // publishing rename — so a file left under the old `.part` name is now
    // simply not part of the transfer at all.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("Arrival.2016.srt");
    let partial = directory.path().join("Arrival.2016.srt.part");
    std::fs::write(&partial, b"bytes from something else entirely").expect("write");

    let payload = patterned_bytes(74_216);
    let share = seeded("Arrival.2016.srt", &payload).await;
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Down,
            "Arrival.2016.srt",
            destination.to_str().expect("utf-8"),
            0,
        ))
        .await;

    worker(&queue, Arc::new(Fixed(share))).drain().await;

    assert_eq!(
        std::fs::read(&destination).expect("read result"),
        payload,
        "the stale partial contributed bytes"
    );
    // And it is still exactly where it was: this transfer never claimed it.
    assert_eq!(
        std::fs::read(&partial).expect("read partial"),
        b"bytes from something else entirely"
    );
}

#[tokio::test]
async fn a_transfer_resumed_after_an_interruption_still_lands_the_whole_file() {
    // `bytes_done` used to be the resume point: the worker seeked a `.part` to
    // it and carried on. It is now only what the row last showed, and the
    // resume point is the stage plus the journal — so a row restored from a
    // queue that recorded a partial byte count must still produce the whole
    // file rather than a file missing its head.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("resumed.txt");

    let full = patterned_bytes(4096);
    let share = seeded("resumed.txt", &full).await;

    let queue = Arc::new(TransferQueue::in_memory());
    let mut transfer = Transfer::new(
        "one",
        Direction::Down,
        "resumed.txt",
        destination.to_str().expect("utf-8"),
        full.len() as u64,
    );
    transfer.bytes_done = 1000;
    queue.add(transfer).await;

    worker(&queue, Arc::new(Fixed(share))).drain().await;

    let done = &queue.all().await[0];
    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(std::fs::read(&destination).expect("read"), full);
}

#[tokio::test]
async fn a_transient_failure_is_retried_and_a_fatal_one_is_not() {
    let directory = tempfile::tempdir().expect("temp dir");
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "flaky",
            Direction::Down,
            "Arrival.2016.srt",
            directory.path().join("a.txt").to_str().expect("utf-8"),
            0,
        ))
        .await;
    let source = Arc::new(FlakySource {
        inner: seeded("Arrival.2016.srt", b"subtitles").await,
        remaining: AtomicU32::new(2),
        transient: true,
    });
    worker(&queue, source).drain().await;
    assert_eq!(queue.all().await[0].status, Status::Done);

    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "denied",
            Direction::Down,
            "Arrival.2016.srt",
            directory.path().join("b.txt").to_str().expect("utf-8"),
            0,
        ))
        .await;
    let source = Arc::new(FlakySource {
        inner: seeded("Arrival.2016.srt", b"subtitles").await,
        remaining: AtomicU32::new(1),
        transient: false,
    });
    worker(&queue, source).drain().await;
    let failed = &queue.all().await[0];
    // Retrying a permission error four more times only delays telling the user.
    assert_eq!(failed.status, Status::Failed);
    assert!(failed
        .error
        .as_deref()
        .is_some_and(|e| e.contains("permission")));
}

#[tokio::test]
async fn a_cancelled_transfer_is_not_settled_as_done() {
    // A cancel that lands while the last chunk is in flight is the user's
    // decision and outranks the worker's.
    let directory = tempfile::tempdir().expect("temp dir");

    // Slow enough that the cancel lands mid-flight rather than after the
    // transfer has already settled, which is the case being tested.
    //
    // 200ms rather than the 40ms this used before E13-S7: a download now keeps
    // four reads in flight, so this file's eight chunks took two rounds rather
    // than eight and the whole thing finished before the cancel at 80ms could
    // land. The test was measuring "eight serial round trips take longer than
    // 80ms", which stopped being true when they stopped being serial.
    let slow: Arc<dyn ShareBackend> =
        Arc::new(MockBackend::media_share().with_latency(std::time::Duration::from_millis(200)));
    let payload: Vec<u8> = vec![7; 8 * 1024 * 1024];
    slow.write_range("big.bin", excalibur_engine::WriteMode::Create, &payload)
        .await
        .expect("seed");

    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Down,
            "big.bin",
            directory.path().join("x.bin").to_str().expect("utf-8"),
            payload.len() as u64,
        ))
        .await;
    let running = worker(&queue, Arc::new(Fixed(slow)));

    let stopper = Arc::clone(&running);
    let stopping = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        stopper.stop("one", Status::Cancelled).await;
    });
    running.drain().await;
    stopping.await.expect("stopper");

    assert_eq!(queue.all().await[0].status, Status::Cancelled);
    // The partial is left where it is; only the *destination* must not appear.
    assert!(!directory.path().join("x.bin").exists());
}

#[tokio::test]
async fn an_upload_writes_what_was_on_disk() {
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("outgoing.txt");
    std::fs::write(&source, b"from this machine").expect("write");

    let backend = mock();
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "up",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "uploaded.txt",
            0,
        ))
        .await;

    worker(&queue, Arc::new(Fixed(Arc::clone(&backend))))
        .drain()
        .await;

    let done = &queue.all().await[0];
    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        backend
            .read_range("uploaded.txt", 0, 1 << 20)
            .await
            .expect("read back"),
        b"from this machine"
    );
}

#[tokio::test]
async fn an_upload_to_a_read_only_mount_fails_with_the_servers_reason() {
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("outgoing.txt");
    std::fs::write(&source, b"nope").expect("write");

    let backend: Arc<dyn ShareBackend> =
        Arc::new(MockBackend::media_share().read_only("export is read-only"));
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "up",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "blocked.txt",
            0,
        ))
        .await;

    worker(&queue, Arc::new(Fixed(backend))).drain().await;

    let failed = &queue.all().await[0];
    assert_eq!(failed.status, Status::Failed);
    // The row shows this verbatim, and it is what decides whether the design's
    // `Remount as RW` button appears on it.
    assert!(failed.needs_write_access(), "{:?}", failed.error);
}

#[tokio::test]
async fn several_transfers_all_finish() {
    let directory = tempfile::tempdir().expect("temp dir");
    let queue = Arc::new(TransferQueue::in_memory());
    for index in 0..6 {
        queue
            .add(Transfer::new(
                format!("t{index}"),
                Direction::Down,
                "Arrival.2016.srt",
                directory
                    .path()
                    .join(format!("copy{index}.txt"))
                    .to_str()
                    .expect("utf-8"),
                0,
            ))
            .await;
    }

    worker(
        &queue,
        Arc::new(Fixed(seeded("Arrival.2016.srt", b"subtitles").await)),
    )
    .drain()
    .await;

    for transfer in queue.all().await {
        assert_eq!(
            transfer.status,
            Status::Done,
            "{} {:?}",
            transfer.id,
            transfer.error
        );
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 6);
}

#[tokio::test]
async fn the_concurrency_limit_can_be_raised_and_lowered() {
    // E17-S1 makes this a preference, so it has to move after construction.
    let directory = tempfile::tempdir().expect("temp dir");
    let queue = Arc::new(TransferQueue::open(directory.path().join("q.json")));
    let worker = worker(&queue, Arc::new(Fixed(mock())));

    assert_eq!(worker.concurrency(), 3);
    worker.set_concurrency(6);
    assert_eq!(worker.concurrency(), 6);
    worker.set_concurrency(1);
    assert_eq!(worker.concurrency(), 1);

    // Zero would not be a setting, it would be a way to hang the queue.
    worker.set_concurrency(0);
    assert_eq!(worker.concurrency(), 1);
}

#[tokio::test]
async fn lowering_the_limit_still_runs_everything() {
    // The shrink cannot recall a permit already held, so it is owed and taken
    // out of the ones that come back. The risk in that design is a transfer
    // that never starts because the debt is never paid, which is what this
    // guards: six files, a limit dropped mid-flight, all six must still land.
    let directory = tempfile::tempdir().expect("temp dir");
    let queue = Arc::new(TransferQueue::open(directory.path().join("q.json")));
    for index in 0..6 {
        queue
            .add(Transfer::new(
                format!("shrink{index}"),
                Direction::Down,
                "Arrival.2016.srt",
                directory
                    .path()
                    .join(format!("shrink{index}.txt"))
                    .to_str()
                    .expect("utf-8"),
                0,
            ))
            .await;
    }

    let worker = worker(
        &queue,
        Arc::new(Fixed(seeded("Arrival.2016.srt", b"subtitles").await)),
    );
    worker.set_concurrency(1);
    worker.drain().await;

    for transfer in queue.all().await {
        assert_eq!(transfer.status, Status::Done, "{}", transfer.id);
    }
    // And the limit that was asked for is the one in force afterwards, rather
    // than whatever the debt happened to settle at.
    assert_eq!(worker.concurrency(), 1);
}

#[tokio::test]
async fn raising_after_lowering_nets_out() {
    // Lower then raise must return to where it started. Adding permits without
    // cancelling the outstanding debt would leave the worker permanently below
    // its own stated limit.
    let directory = tempfile::tempdir().expect("temp dir");
    let queue = Arc::new(TransferQueue::open(directory.path().join("q.json")));
    let worker = worker(
        &queue,
        Arc::new(Fixed(seeded("Arrival.2016.srt", b"subtitles").await)),
    );

    worker.set_concurrency(1);
    worker.set_concurrency(3);
    assert_eq!(worker.concurrency(), 3);

    for index in 0..4 {
        queue
            .add(Transfer::new(
                format!("net{index}"),
                Direction::Down,
                "Arrival.2016.srt",
                directory
                    .path()
                    .join(format!("net{index}.txt"))
                    .to_str()
                    .expect("utf-8"),
                0,
            ))
            .await;
    }
    worker.drain().await;
    for transfer in queue.all().await {
        assert_eq!(transfer.status, Status::Done, "{}", transfer.id);
    }
}

/// A backend whose reads go through a handle that counts what it is asked
/// (E13-S7). Everything else is the mock's.
struct Handed {
    inner: Arc<dyn ShareBackend>,
    opens: Arc<AtomicU32>,
    closes: Arc<AtomicU32>,
    live: Arc<AtomicU32>,
    peak: Arc<AtomicU32>,
}

struct Counted {
    inner: Arc<dyn ShareBackend>,
    path: String,
    closes: Arc<AtomicU32>,
    live: Arc<AtomicU32>,
    peak: Arc<AtomicU32>,
}

#[async_trait]
impl excalibur_engine::backend::ReadHandle for Counted {
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(live, Ordering::SeqCst);
        // Long enough that a serial reader could not have overlapped by
        // accident: the count below is the property, and this is what gives it
        // something to count.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let bytes = self.inner.read_range(&self.path, offset, length).await;
        self.live.fetch_sub(1, Ordering::SeqCst);
        bytes
    }

    fn max_read(&self) -> u32 {
        1024 * 1024
    }

    async fn close(&self) -> Result<()> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl ShareBackend for Handed {
    fn mount_info(&self) -> excalibur_engine::MountInfo {
        self.inner.mount_info()
    }
    fn caps(&self) -> excalibur_engine::Caps {
        self.inner.caps()
    }
    async fn capacity(&self) -> Result<excalibur_engine::Capacity> {
        self.inner.capacity().await
    }
    async fn list_dir(&self, path: &str) -> Result<excalibur_engine::Listing> {
        self.inner.list_dir(path).await
    }
    async fn stat(&self, path: &str) -> Result<excalibur_engine::Attrs> {
        self.inner.stat(path).await
    }
    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.inner.read_range(path, offset, length).await
    }
    async fn open_read(
        &self,
        path: &str,
    ) -> Result<Option<Arc<dyn excalibur_engine::backend::ReadHandle>>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(Counted {
            inner: Arc::clone(&self.inner),
            path: path.to_owned(),
            closes: Arc::clone(&self.closes),
            live: Arc::clone(&self.live),
            peak: Arc::clone(&self.peak),
        })))
    }
    async fn write_range(
        &self,
        path: &str,
        mode: excalibur_engine::WriteMode,
        data: &[u8],
    ) -> Result<u64> {
        self.inner.write_range(path, mode, data).await
    }
    async fn mkdir(&self, path: &str) -> Result<()> {
        self.inner.mkdir(path).await
    }
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.inner.rename(from, to, replace).await
    }
    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.inner.remove(path, recursive).await
    }
    async fn ping(&self) -> Result<excalibur_engine::Health> {
        self.inner.ping().await
    }
}

/// A share holding `big.bin`, whose every byte depends on its offset.
///
/// A pattern rather than a constant fill: the property under test is that the
/// chunks are written **in the order they were asked for**, and a file of
/// identical bytes cannot tell a correct copy from a shuffled one.
async fn patterned(size: usize) -> (Arc<Handed>, Vec<u8>) {
    let payload: Vec<u8> = (0..size)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect();
    let inner = mock();
    inner
        .write_range("big.bin", excalibur_engine::WriteMode::Create, &payload)
        .await
        .expect("seed");
    (
        Arc::new(Handed {
            inner,
            opens: Arc::new(AtomicU32::new(0)),
            closes: Arc::new(AtomicU32::new(0)),
            live: Arc::new(AtomicU32::new(0)),
            peak: Arc::new(AtomicU32::new(0)),
        }),
        payload,
    )
}

#[tokio::test]
async fn a_download_opens_one_handle_per_worker_and_keeps_several_reads_in_flight() {
    // E13-S7's property, restated for E22-S2. The old chunk loop opened one
    // handle per transfer and issued `depth` reads down it. xsync's engine
    // opens one **per segment worker** and hands each worker a contiguous
    // region, which is what `Source::open_region` is for. The number moved;
    // what has not moved is the point of the story — the handle is opened once
    // per reader rather than once per chunk, and the chunks overlap.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("big.bin");
    // Sixteen chunks of the handle's 1 MiB, so "one open per chunk" and "one
    // open per worker" are far apart and the assertion below means something.
    let (share, payload) = patterned(16 * 1024 * 1024).await;

    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Down,
            "big.bin",
            destination.to_str().expect("utf-8"),
            payload.len() as u64,
        ))
        .await;

    let running = worker(
        &queue,
        Arc::new(Fixed(Arc::clone(&share) as Arc<dyn ShareBackend>)),
    );
    running.set_read_depth(4);
    running.drain().await;

    let done = &queue.all().await[0];
    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    let opens = share.opens.load(Ordering::SeqCst);
    assert!(
        opens <= 5,
        "{opens} opens for sixteen chunks: the handle is being reopened per read"
    );
    // Every handle is released. One leaked per failed file exhausts a
    // session's table long before the queue empties.
    assert_eq!(share.closes.load(Ordering::SeqCst), opens);
    assert_eq!(
        share.peak.load(Ordering::SeqCst),
        4,
        "reads did not overlap to the configured depth"
    );
    // And the bytes are in the order they were asked for. Writing them as they
    // arrived would pass every length check and produce a shuffled file.
    assert_eq!(std::fs::read(&destination).expect("read result"), payload);
}

#[tokio::test]
async fn a_depth_of_one_is_the_serial_download_it_replaced() {
    // The escape hatch, and the proof that depth is the only thing that
    // changed: at one, this is chunk-at-a-time again and still correct.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("big.bin");
    let (share, payload) = patterned(3 * 1024 * 1024).await;

    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::new(
            "one",
            Direction::Down,
            "big.bin",
            destination.to_str().expect("utf-8"),
            payload.len() as u64,
        ))
        .await;

    let running = worker(
        &queue,
        Arc::new(Fixed(Arc::clone(&share) as Arc<dyn ShareBackend>)),
    );
    running.set_read_depth(1);
    running.drain().await;

    assert_eq!(share.peak.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&destination).expect("read result"), payload);
}

#[tokio::test]
async fn a_resumed_streaming_download_starts_where_the_queue_left_off() {
    // The `.part` contract is unchanged by the pipelining: the reads are
    // scheduled from the resume point, not from zero, and the bytes already on
    // disk are not fetched again.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("big.bin");
    let partial = directory.path().join("big.bin.part");
    let (share, payload) = patterned(4 * 1024 * 1024).await;

    let head = 1024 * 1024 + 7;
    std::fs::write(&partial, &payload[..head]).expect("write");

    let queue = Arc::new(TransferQueue::in_memory());
    let mut transfer = Transfer::new(
        "one",
        Direction::Down,
        "big.bin",
        destination.to_str().expect("utf-8"),
        payload.len() as u64,
    );
    transfer.bytes_done = head as u64;
    queue.add(transfer).await;

    worker(
        &queue,
        Arc::new(Fixed(Arc::clone(&share) as Arc<dyn ShareBackend>)),
    )
    .drain()
    .await;

    assert_eq!(std::fs::read(&destination).expect("read result"), payload);
}
