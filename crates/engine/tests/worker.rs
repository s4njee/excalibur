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

#[tokio::test]
async fn a_download_lands_whole_and_leaves_no_partial() {
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("Arrival.2016.mkv");
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

    worker(&queue, Arc::new(Fixed(mock()))).drain().await;

    let done = &queue.all().await[0];
    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert!(destination.exists(), "the file was not published");
    // The partial is consumed by the rename, not left beside the result.
    assert!(!directory.path().join("Arrival.2016.mkv.part").exists());
    assert_eq!(
        done.bytes_done,
        std::fs::metadata(&destination).unwrap().len()
    );
}

#[tokio::test]
async fn a_resumed_download_discards_bytes_the_queue_cannot_vouch_for() {
    // The bug this exists to prevent, and the one the sibling engine has: a
    // `.part` left by a *different* cancelled transfer to the same
    // destination. Trusting its length appends real bytes onto a stranger's
    // and produces a file that passes every length check and is wrong.
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("Arrival.2016.srt");
    let partial = directory.path().join("Arrival.2016.srt.part");
    std::fs::write(&partial, b"bytes from something else entirely").expect("write");

    let queue = Arc::new(TransferQueue::in_memory());
    let mut transfer = Transfer::new(
        "one",
        Direction::Down,
        "Arrival.2016.srt",
        destination.to_str().expect("utf-8"),
        0,
    );
    // The queue never recorded any progress for this transfer.
    transfer.bytes_done = 0;
    queue.add(transfer).await;

    worker(&queue, Arc::new(Fixed(mock()))).drain().await;

    let expected = mock()
        .read_range("Arrival.2016.srt", 0, 1 << 20)
        .await
        .expect("read");
    assert_eq!(
        std::fs::read(&destination).expect("read result"),
        expected,
        "the stale partial contributed bytes"
    );
}

#[tokio::test]
async fn a_resume_continues_from_what_the_queue_recorded() {
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("resumed.txt");
    let partial = directory.path().join("resumed.txt.part");

    // Put a known payload on the share rather than relying on fixture
    // contents, so the assertion is about resume and nothing else.
    let backend = mock();
    let full: Vec<u8> = (0..4096_u32)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    backend
        .write_range("resumed.txt", excalibur_engine::WriteMode::Create, &full)
        .await
        .expect("seed");

    let head = &full[..1000];
    std::fs::write(&partial, head).expect("write");

    let queue = Arc::new(TransferQueue::in_memory());
    let mut transfer = Transfer::new(
        "one",
        Direction::Down,
        "resumed.txt",
        destination.to_str().expect("utf-8"),
        full.len() as u64,
    );
    // The queue vouches for exactly those bytes, so they are kept.
    transfer.bytes_done = head.len() as u64;
    queue.add(transfer).await;

    worker(&queue, Arc::new(Fixed(backend))).drain().await;

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
        inner: mock(),
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
        inner: mock(),
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
    let slow: Arc<dyn ShareBackend> =
        Arc::new(MockBackend::media_share().with_latency(std::time::Duration::from_millis(40)));
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

    worker(&queue, Arc::new(Fixed(mock()))).drain().await;

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
