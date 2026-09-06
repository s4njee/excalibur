//! What a folder copy actually costs, measured rather than argued.
//!
//! Every case here is `#[ignore]`d: they move hundreds of megabytes and time
//! themselves, which is a measurement rather than an assertion. Run one with
//!
//! ```text
//! cargo test -p excalibur-engine --release --test throughput -- --ignored --nocapture
//! ```
//!
//! The rows exist so that three different costs can be told apart.
//! `std::fs recursive copy` is the floor — what this machine does with no
//! planner at all. The two `endpoint` rows are one `sync_endpoints` session
//! through `transfer::endpoint`, which is the adapter's own cost. The two `app`
//! rows are what the Tauri command does with a selection, and the gap between
//! them and `endpoint: down` is the app's own overhead rather than the
//! adapter's.
//!
//! `app: down, one row per file` is E13-S4's shape, kept after E22-S3 replaced
//! it: it is the measurement that justified the replacement and it is the one
//! that would catch a regression back to it. `app: down, one row per folder` is
//! what `enqueue_downloads` does now.
//!
//! `queue: N rows, M progress ticks` is a different question with the same
//! answer — a queue backed by a real file, so the cost of writing it down is in
//! the number. The app's own `transfers.json` reached 1.2 MB over 2253 rows,
//! and until E22-S3 every progress tick rewrote all of it.

// A stopwatch, not a program: every cast here is a duration or a byte count on
// its way to a printed table, and `try_from` on each of them would bury the
// measurement in error handling for a number that cannot overflow this side of
// five centuries.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::{
    Attrs, Capacity, Caps, Health, Listing, MountInfo, ReadHandle, ShareBackend, WriteMode,
};
use excalibur_engine::transfer::plan::plan_downloads;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::{Direction, Status, Transfer};
use excalibur_engine::Result;

/// What `src-tauri` uses for `Worker::new`'s third argument.
const APP_CONCURRENCY: usize = 3;

/// One shape of tree: how many files of what size.
#[derive(Clone, Copy)]
struct Shape {
    label: &'static str,
    small_files: usize,
    small_bytes: usize,
    large_files: usize,
    large_bytes: usize,
}

impl Shape {
    fn total_bytes(self) -> u64 {
        (self.small_files * self.small_bytes + self.large_files * self.large_bytes) as u64
    }

    fn file_count(self) -> usize {
        self.small_files + self.large_files
    }
}

/// Many small files: the shape where per-file overhead is the whole story.
const SMALL: Shape = Shape {
    label: "2000 x 4 KiB",
    small_files: 2000,
    small_bytes: 4096,
    large_files: 0,
    large_bytes: 0,
};

/// A few large files: the shape where per-chunk overhead is the whole story.
const LARGE: Shape = Shape {
    label: "4 x 64 MiB",
    small_files: 0,
    small_bytes: 0,
    large_files: 4,
    large_bytes: 64 * 1024 * 1024,
};

/// What a user actually drags.
const MIXED: Shape = Shape {
    label: "1000 x 4 KiB + 2 x 32 MiB",
    small_files: 1000,
    small_bytes: 4096,
    large_files: 2,
    large_bytes: 32 * 1024 * 1024,
};

/// Build the tree, spread over subdirectories so the walk has work to do.
fn build(root: &std::path::Path, shape: Shape) {
    std::fs::create_dir_all(root).unwrap();
    let small = vec![7_u8; shape.small_bytes];
    for index in 0..shape.small_files {
        let directory = root.join(format!("d{:03}", index / 50));
        if index % 50 == 0 {
            std::fs::create_dir_all(&directory).unwrap();
        }
        std::fs::write(directory.join(format!("f{index:05}.bin")), &small).unwrap();
    }
    if shape.large_files > 0 {
        let large = vec![3_u8; shape.large_bytes];
        std::fs::create_dir_all(root.join("big")).unwrap();
        for index in 0..shape.large_files {
            std::fs::write(root.join(format!("big/g{index:02}.bin")), &large).unwrap();
        }
    }
}

/// Recursive `std::fs::copy`, as the floor every other row is read against.
fn plain_copy(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            plain_copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// One measured run, in the shape the table prints.
struct Row {
    label: String,
    elapsed: Duration,
    bytes: u64,
    /// When the first byte reached the destination, from the start of the run.
    first_byte: Option<Duration>,
}

impl Row {
    fn print(&self) {
        #[allow(clippy::cast_precision_loss)]
        let mb = self.bytes as f64 / (1024.0 * 1024.0);
        let seconds = self.elapsed.as_secs_f64();
        let first = self.first_byte.map_or_else(
            || "     -".to_owned(),
            |at| format!("{:6.2}", at.as_secs_f64()),
        );
        println!(
            "| {:<28} | {:>8.2} | {:>9.1} | {first} |",
            self.label,
            seconds,
            mb / seconds
        );
    }
}

fn header(title: &str) {
    println!("\n### {title}");
    println!("| case                         | seconds  |     MB/s  | 1st B |");
    println!("| ---------------------------- | -------- | --------- | ----- |");
}

/// Every call a transfer made through the seam, and what it cost.
#[derive(Default)]
struct Counters {
    list_dir: AtomicUsize,
    stat: AtomicUsize,
    read_range: AtomicUsize,
    read_range_bytes: AtomicU64,
    open_read: AtomicUsize,
    handle_reads: AtomicUsize,
    handle_read_bytes: AtomicU64,
    write_range: AtomicUsize,
    write_range_bytes: AtomicU64,
    mkdir: AtomicUsize,
    rename: AtomicUsize,
    remove: AtomicUsize,
    /// Nanoseconds spent inside each family of call, summed across threads.
    walk_nanos: AtomicU64,
    read_nanos: AtomicU64,
    write_nanos: AtomicU64,
}

impl Counters {
    fn print(&self) {
        let get = |slot: &AtomicUsize| slot.load(Ordering::Relaxed);
        let bytes = |slot: &AtomicU64| slot.load(Ordering::Relaxed);
        #[allow(clippy::cast_precision_loss)]
        let secs = |slot: &AtomicU64| slot.load(Ordering::Relaxed) as f64 / 1e9;
        let per_call = |count: usize, total: u64| {
            if count == 0 {
                0
            } else {
                total / count as u64
            }
        };
        println!(
            "    seam: list_dir={} stat={} mkdir={} rename={} remove={}  ({:.2}s in walk calls)",
            get(&self.list_dir),
            get(&self.stat),
            get(&self.mkdir),
            get(&self.rename),
            get(&self.remove),
            secs(&self.walk_nanos),
        );
        println!(
            "    read: open_read={} handle_read_at={} ({} B avg) read_range={} ({} B avg)  ({:.2}s in read calls)",
            get(&self.open_read),
            get(&self.handle_reads),
            per_call(get(&self.handle_reads), bytes(&self.handle_read_bytes)),
            get(&self.read_range),
            per_call(get(&self.read_range), bytes(&self.read_range_bytes)),
            secs(&self.read_nanos),
        );
        println!(
            "    write: write_range={} ({} B avg, {} MiB total)  ({:.2}s in write calls)",
            get(&self.write_range),
            per_call(get(&self.write_range), bytes(&self.write_range_bytes)),
            bytes(&self.write_range_bytes) / (1024 * 1024),
            secs(&self.write_nanos),
        );
    }
}

/// A backend that counts and times every call the seam makes through it.
struct Counting {
    inner: Arc<dyn ShareBackend>,
    counters: Arc<Counters>,
}

struct CountingHandle {
    inner: Arc<dyn ReadHandle>,
    counters: Arc<Counters>,
}

#[async_trait]
impl ReadHandle for CountingHandle {
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        let at = Instant::now();
        let out = self.inner.read_at(offset, length).await;
        self.counters
            .read_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.handle_reads.fetch_add(1, Ordering::Relaxed);
        if let Ok(bytes) = &out {
            self.counters
                .handle_read_bytes
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        out
    }

    fn max_read(&self) -> u32 {
        self.inner.max_read()
    }

    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[async_trait]
impl ShareBackend for Counting {
    fn mount_info(&self) -> MountInfo {
        self.inner.mount_info()
    }

    fn caps(&self) -> Caps {
        self.inner.caps()
    }

    async fn capacity(&self) -> Result<Capacity> {
        self.inner.capacity().await
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let at = Instant::now();
        let out = self.inner.list_dir(path).await;
        self.counters
            .walk_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.list_dir.fetch_add(1, Ordering::Relaxed);
        out
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let at = Instant::now();
        let out = self.inner.stat(path).await;
        self.counters
            .walk_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.stat.fetch_add(1, Ordering::Relaxed);
        out
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let at = Instant::now();
        let out = self.inner.read_range(path, offset, length).await;
        self.counters
            .read_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.read_range.fetch_add(1, Ordering::Relaxed);
        if let Ok(bytes) = &out {
            self.counters
                .read_range_bytes
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        out
    }

    async fn open_read(&self, path: &str) -> Result<Option<Arc<dyn ReadHandle>>> {
        self.counters.open_read.fetch_add(1, Ordering::Relaxed);
        Ok(self.inner.open_read(path).await?.map(|handle| {
            Arc::new(CountingHandle {
                inner: handle,
                counters: Arc::clone(&self.counters),
            }) as Arc<dyn ReadHandle>
        }))
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        let at = Instant::now();
        let out = self.inner.write_range(path, mode, data).await;
        self.counters
            .write_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.write_range.fetch_add(1, Ordering::Relaxed);
        self.counters
            .write_range_bytes
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        out
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let at = Instant::now();
        let out = self.inner.mkdir(path).await;
        self.counters
            .walk_nanos
            .fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counters.mkdir.fetch_add(1, Ordering::Relaxed);
        out
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.counters.rename.fetch_add(1, Ordering::Relaxed);
        self.inner.rename(from, to, replace).await
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.counters.remove.fetch_add(1, Ordering::Relaxed);
        self.inner.remove(path, recursive).await
    }

    async fn ping(&self) -> Result<Health> {
        self.inner.ping().await
    }
}

/// Drive one `endpoint::run` and time it, with the seam counted.
async fn measure(
    label: &str,
    share: Arc<dyn ShareBackend>,
    counters: &Arc<Counters>,
    transfer: Transfer,
    bytes: u64,
) -> Row {
    let start = Instant::now();
    let first = Arc::new(std::sync::Mutex::new(None));
    let seen = Arc::clone(&first);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    excalibur_engine::transfer::endpoint::run(&share, &transfer, 4, stop, move |moved| {
        if moved.bytes > 0 {
            let mut slot = seen.lock().unwrap();
            if slot.is_none() {
                *slot = Some(start.elapsed());
            }
        }
    })
    .await
    .expect("the transfer finished");
    let elapsed = start.elapsed();
    let row = Row {
        label: label.to_owned(),
        elapsed,
        bytes,
        first_byte: *first.lock().unwrap(),
    };
    row.print();
    counters.print();
    row
}

/// A `BackendSource` that hands out one fixed share, as the app's does.
struct Fixed(Arc<dyn ShareBackend>);

#[async_trait]
impl BackendSource for Fixed {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.0))
    }
}

/// A share holding one `Show` tree, and somewhere to download it to.
struct Downloadable {
    workspace: tempfile::TempDir,
    share: Arc<dyn ShareBackend>,
    destination: std::path::PathBuf,
    counters: Arc<Counters>,
}

impl Downloadable {
    async fn build(tree: Shape) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let exported = workspace.path().join("export");
        build(&exported.join("Show"), tree);
        let destination = workspace.path().join("downloads");
        std::fs::create_dir_all(&destination).unwrap();
        let counters = Arc::new(Counters::default());
        let share: Arc<dyn ShareBackend> = Arc::new(Counting {
            inner: Arc::new(LocalBackend::open(&exported, false).await.unwrap()),
            counters: Arc::clone(&counters),
        });
        Self {
            workspace,
            share,
            destination,
            counters,
        }
    }
}

/// Drain a queue of rows through a worker and time the whole thing.
///
/// `walked` is whatever the app did before the first row existed, which is the
/// number `first_byte` shows: for the old shape that is the pre-walk, and for
/// the new one there is nothing to wait for.
async fn drain_rows(
    label: &str,
    share: &Arc<dyn ShareBackend>,
    counters: &Arc<Counters>,
    rows: Vec<Transfer>,
    bytes: u64,
    start: Instant,
    walked: Option<Duration>,
) -> Row {
    let queue = Arc::new(TransferQueue::in_memory());
    let count = rows.len();
    for row in rows {
        queue.add(row).await;
    }
    Arc::new(Worker::new(
        Arc::clone(&queue),
        Arc::new(Fixed(Arc::clone(share))),
        APP_CONCURRENCY,
    ))
    .drain()
    .await;
    let elapsed = start.elapsed();
    let unfinished: Vec<Transfer> = queue
        .all()
        .await
        .into_iter()
        .filter(|row| row.status != Status::Done)
        .collect();
    assert!(
        unfinished.is_empty(),
        "{} of {count} rows did not finish: {:?}",
        unfinished.len(),
        unfinished.first().map(|row| row.error.clone()),
    );

    let row = Row {
        label: label.to_owned(),
        elapsed,
        bytes,
        first_byte: walked,
    };
    row.print();
    println!("    {count} queue rows");
    counters.print();
    row
}

/// The download as `enqueue_downloads` performed it before E22-S3.
///
/// `plan_downloads` walks the whole share subtree before a row exists, then
/// every file beneath it becomes its own `Transfer` — and so its own
/// `sync_endpoints` session, with its own scoped thread pool, cancel watcher
/// and journal. Kept as the control: it is what the new row is read against.
async fn app_download_per_file(shape: Shape) -> Row {
    let fixture = Downloadable::build(shape).await;
    let start = Instant::now();
    let plan = plan_downloads(
        fixture.share.as_ref(),
        &["Show".to_owned()],
        &fixture.destination,
    )
    .await;
    let walked = start.elapsed();
    for directory in &plan.directories {
        std::fs::create_dir_all(directory).unwrap();
    }
    let rows = plan
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            Transfer::new(
                format!("row{index}"),
                Direction::Down,
                file.source.clone(),
                file.destination.to_string_lossy().into_owned(),
                file.size,
            )
        })
        .collect();
    let row = drain_rows(
        "app: down, one row per file",
        &fixture.share,
        &fixture.counters,
        rows,
        shape.total_bytes(),
        start,
        Some(walked),
    )
    .await;
    println!("    the walk before the first row took {walked:.2?}");
    drop(fixture.workspace);
    row
}

/// The download exactly as `enqueue_downloads` performs it since E22-S3.
///
/// One `stat` of the selected path, one row, one session. The planner does the
/// walk, which is the walk the transfer was going to do anyway.
async fn app_download_per_folder(shape: Shape) -> Row {
    let fixture = Downloadable::build(shape).await;
    let start = Instant::now();
    let rows = vec![Transfer::new(
        "row0",
        Direction::Down,
        "Show",
        fixture.destination.join("Show").to_str().unwrap(),
        0,
    )];
    let row = drain_rows(
        "app: down, one row per folder",
        &fixture.share,
        &fixture.counters,
        rows,
        shape.total_bytes(),
        start,
        None,
    )
    .await;
    drop(fixture.workspace);
    row
}

/// The whole table, for one shape of tree.
async fn table(shape: Shape) {
    header(&format!(
        "{} ({} files, {} MiB)",
        shape.label,
        shape.file_count(),
        shape.total_bytes() / (1024 * 1024)
    ));

    // `cp`: the floor.
    {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("src/Show");
        build(&source, shape);
        let start = Instant::now();
        plain_copy(&source, &workspace.path().join("dst/Show"));
        Row {
            label: "std::fs recursive copy".to_owned(),
            elapsed: start.elapsed(),
            bytes: shape.total_bytes(),
            first_byte: None,
        }
        .print();
    }

    // Down: a folder copied *out* of a share onto this machine. The source is
    // `ShareSource` over the seam; the sink is xsync's own local one.
    {
        let fixture = Downloadable::build(shape).await;
        measure(
            "endpoint: down (out of share)",
            Arc::clone(&fixture.share),
            &fixture.counters,
            Transfer::new(
                "down",
                Direction::Down,
                "Show",
                fixture.destination.join("Show").to_str().unwrap(),
                0,
            ),
            shape.total_bytes(),
        )
        .await;
    }

    app_download_per_file(shape).await;
    app_download_per_folder(shape).await;

    // Up: a folder copied *into* a share. The sink is `ShareSink` over the
    // seam; the source is xsync's own local one.
    {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("Show");
        build(&source, shape);
        let into = workspace.path().join("share");
        std::fs::create_dir_all(&into).unwrap();
        let counters = Arc::new(Counters::default());
        let backend: Arc<dyn ShareBackend> = Arc::new(Counting {
            inner: Arc::new(LocalBackend::open(&into, false).await.unwrap()),
            counters: Arc::clone(&counters),
        });
        measure(
            "endpoint: up (into share)",
            backend,
            &counters,
            Transfer::new("up", Direction::Up, source.to_str().unwrap(), "Copied", 0),
            shape.total_bytes(),
        )
        .await;
    }
}

/// What a queue of `rows` costs when `ticks` progress updates go through it.
///
/// Backed by a real file, which is the whole point: `TransferQueue::in_memory`
/// is what every other case here uses and it hides this cost completely. The
/// rows are given the sizes a real queue has — the app's own `transfers.json`
/// was 1.2 MB over 2253 rows — and the ticks are what one running transfer
/// produces in a few seconds.
async fn queue_cost(rows: usize, ticks: usize) {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    for index in 0..rows {
        queue
            .add(Transfer::new(
                format!("row{index:05}"),
                Direction::Down,
                format!("Show/d{:03}/f{index:05}.bin", index / 50),
                workspace
                    .path()
                    .join(format!("downloads/Show/d{:03}/f{index:05}.bin", index / 50))
                    .to_string_lossy()
                    .into_owned(),
                4096,
            ))
            .await;
    }
    let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
    queue
        .update("row00000", |row| row.status = Status::Running)
        .await;

    let start = Instant::now();
    for tick in 0..ticks {
        queue
            .update("row00000", |row| {
                row.bytes_done = tick as u64 * 64;
                row.rate = Some(1_000_000);
            })
            .await;
    }
    let progress = start.elapsed();

    let start = Instant::now();
    queue
        .update("row00000", |row| row.status = Status::Done)
        .await;
    let one_write = start.elapsed();

    println!(
        "| {:<28} | {:>8.2} | {:>9} | {:>5} |",
        format!("queue: {rows} rows, {ticks} ticks"),
        progress.as_secs_f64(),
        "-",
        "-"
    );
    // The projection is the point of the case: before E22-S3 a progress tick
    // *was* a status change as far as the file was concerned, so this is what
    // the old rule cost for one running transfer's worth of ticks.
    println!(
        "    file is {} KiB; {ticks} progress updates took {progress:.2?}, one status change {one_write:.2?}",
        size / 1024,
    );
    println!(
        "    persisting every tick would be {:.2?} and {} MiB written; it is now no write at all",
        one_write * ticks as u32,
        size * ticks as u64 / (1024 * 1024),
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not an assertion"]
async fn many_small_files() {
    table(SMALL).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not an assertion"]
async fn a_few_large_files() {
    table(LARGE).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not an assertion"]
async fn a_mixed_tree() {
    table(MIXED).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not an assertion"]
async fn a_queue_on_disk() {
    header("a queue backed by a file");
    queue_cost(2253, 1000).await;
    queue_cost(200, 1000).await;
}
