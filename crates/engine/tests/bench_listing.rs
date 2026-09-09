//! One-round-trip listing against the paging one (`twopane.md` XTP-E9-S3).
//!
//! Not a `#[test]`: it measures, and a measurement that fails a suite when a
//! laptop is busy is a flaky test rather than a benchmark. Run it deliberately:
//!
//! ```text
//! EXCALIBUR_XS=../xsync/target/release/xs \
//!   cargo test -p excalibur-engine --test bench_listing -- --ignored --nocapture
//! ```
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use excalibur_engine::backend::xsync::{self, XsyncBackend};
use excalibur_engine::{MessageLog, ShareBackend};

fn xs_binary() -> Option<String> {
    let path = std::env::var("EXCALIBUR_XS").ok()?;
    PathBuf::from(&path).is_file().then_some(path)
}

#[tokio::test]
#[ignore = "a measurement, not an assertion: run it deliberately"]
async fn listing_round_trips() {
    let Some(xs) = xs_binary() else {
        eprintln!("set EXCALIBUR_XS to an xs binary");
        return;
    };
    let root = tempfile::tempdir().expect("temp dir");
    for index in 0..200 {
        std::fs::write(root.path().join(format!("file-{index:04}.txt")), b"x").unwrap();
    }
    // The frame tap is the measurement that matters: a round trip over a local
    // pipe costs microseconds, so the *time* here is dominated by syscalls and
    // serialisation and the two paths are indistinguishable. Over SSH to
    // another machine each round trip costs the link's latency, and the frame
    // count is what says how many of those a listing needs.
    let log = Arc::new(MessageLog::new());
    let backend = XsyncBackend::connect_local(
        &xs,
        root.path().to_str().unwrap(),
        false,
        Some(xsync::frame_tap(Arc::clone(&log), "bench".to_owned())),
    )
    .await
    .expect("mount");

    // Warm: the first listing pays for the mount's own bookkeeping.
    let _ = backend.list_dir("").await.expect("list");

    // The frame count is taken from *one* listing: the log is a bounded ring
    // and two hundred listings overflow it, which would report a count capped
    // by the ring rather than the exchange.
    {
        let mark = log.lines().len();
        let _ = backend.list_dir("").await.expect("list");
        let one: Vec<String> = log.lines().into_iter().skip(mark).map(|l| l.text).collect();
        println!("one listing is {} frames: {one:?}", one.len());
    }
    let rounds = 200;
    let started = Instant::now();
    for _ in 0..rounds {
        let _ = backend.list_dir("").await.expect("list");
    }
    let elapsed = started.elapsed();
    println!(
        "{rounds} listings of 200 entries: {:?} total, {:?} each",
        elapsed,
        elapsed / rounds,
    );
}
