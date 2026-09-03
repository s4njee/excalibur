//! The engine's contract, exercised through the mock.
//!
//! The gating tests matter most: the design stakes everything on one derived
//! value, and a backend that lets a write through on a read-only mount would
//! make the badge a lie.

use std::time::Duration;

use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::{EngineError, EntryKind, ShareBackend, WriteMode};

#[tokio::test]
async fn the_media_share_is_the_listing_the_design_shows() {
    let backend = MockBackend::media_share();
    let listing = backend.list_dir("").await.unwrap();

    // The status bar in the mock reads "13 items".
    assert_eq!(listing.entries.len(), 13);
    assert_eq!(
        listing
            .entries
            .iter()
            .filter(|e| e.attrs.kind == EntryKind::Dir)
            .count(),
        3
    );

    let feature = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Arrival.2016.mkv")
        .expect("the mock's first film");
    assert_eq!(feature.kind_label, "Matroska Video");
    // 48.2 GB, as the mock's selected-size figure shows.
    assert_eq!(feature.attrs.size, 48_234_496_000);

    // A dotfile is marked so the list can dim it and label it "Hidden".
    let hidden = listing
        .entries
        .iter()
        .find(|entry| entry.name == ".DS_Store")
        .expect("the mock's dotfile");
    assert!(hidden.hidden);
    assert_eq!(hidden.kind_label, "Hidden");
}

#[tokio::test]
async fn writability_travels_with_the_listing() {
    // The status bar and the rows it describes have to agree. Fetching them
    // separately would let two round trips disagree, and the badge is the
    // thing a user trusts.
    let writable = MockBackend::media_share();
    assert!(writable.list_dir("").await.unwrap().mount.writable);

    let refused = MockBackend::media_share().read_only("export mapped ro,root_squash");
    let listing = refused.list_dir("").await.unwrap();
    assert!(!listing.mount.writable);
    assert_eq!(listing.mount.reason, "export mapped ro,root_squash");
    // A reason is required whenever a write is refused: the status bar shows it
    // verbatim, so an empty one would leave the user with no explanation.
    assert!(!listing.mount.reason.is_empty());
}

#[tokio::test]
async fn every_mutation_is_refused_on_a_read_only_mount() {
    let backend = MockBackend::media_share().read_only("export is read-only");

    // Enumerated rather than spot-checked: the UI disables these from one
    // derived value, and a mutation that slipped past the list here is one the
    // UI would also forget to disable.
    let attempts: Vec<(&str, EngineError)> = vec![
        (
            "write",
            backend
                .write_range("new.txt", WriteMode::Create, b"x")
                .await
                .expect_err("write must be refused"),
        ),
        (
            "mkdir",
            backend
                .mkdir("new-folder")
                .await
                .expect_err("mkdir must be refused"),
        ),
        (
            "rename",
            backend
                .rename("Arrival.2016.mkv", "renamed.mkv", false)
                .await
                .expect_err("rename must be refused"),
        ),
        (
            "remove",
            backend
                .remove("Arrival.2016.mkv", false)
                .await
                .expect_err("remove must be refused"),
        ),
    ];
    for (verb, error) in attempts {
        assert!(
            matches!(error, EngineError::ReadOnly(_)),
            "{verb} was refused for the wrong reason: {error}"
        );
    }

    // Reading is untouched.
    assert!(backend.list_dir("").await.is_ok());
    assert!(backend.stat("Arrival.2016.mkv").await.is_ok());
}

#[tokio::test]
async fn a_writable_mount_round_trips_a_file() {
    let backend = MockBackend::empty();
    backend
        .write_range("notes.txt", WriteMode::Create, b"hello")
        .await
        .unwrap();
    assert_eq!(
        backend.read_range("notes.txt", 0, 32).await.unwrap(),
        b"hello"
    );
    // Resume writes at an offset rather than from the start.
    backend
        .write_range("notes.txt", WriteMode::Resume { offset: 5 }, b" world")
        .await
        .unwrap();
    assert_eq!(
        backend.read_range("notes.txt", 0, 32).await.unwrap(),
        b"hello world"
    );
    assert_eq!(backend.stat("notes.txt").await.unwrap().size, 11);
}

#[tokio::test]
async fn failures_are_classified_the_way_the_ui_routes_them() {
    let backend = MockBackend::media_share();
    let missing = backend.stat("nope").await.expect_err("missing path");
    assert!(matches!(missing, EngineError::NotFound(_)));
    // A per-item failure belongs in the transfer queue, not a banner.
    assert_eq!(
        missing.surface(),
        excalibur_engine::error::Surface::TransferRow
    );
    assert!(!missing.transient());

    // A dropped connection is the banner case, and is worth retrying.
    let dropped = MockBackend::media_share().failing_after(0);
    let error = dropped
        .list_dir("")
        .await
        .expect_err("mock was told to fail");
    assert!(matches!(error, EngineError::Disconnected(_)));
    assert_eq!(error.surface(), excalibur_engine::error::Surface::Banner);
    assert!(error.transient());
}

#[tokio::test]
async fn the_mock_can_be_made_slow_enough_to_show_a_spinner() {
    // The design says a listing under ~200ms must not flash a spinner, so the
    // frontend needs a backend that can reliably take longer than that.
    let backend = MockBackend::media_share().with_latency(Duration::from_millis(250));
    let started = std::time::Instant::now();
    backend.list_dir("").await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250));
}

#[tokio::test]
async fn a_failing_connection_reports_unhealthy_rather_than_erroring() {
    // The status-bar dot asks this on a timer; a health check that returned an
    // error would make every poll look like a crash.
    let backend = MockBackend::media_share().failing_after(0);
    assert_eq!(
        backend.ping().await.unwrap(),
        excalibur_engine::Health::Failed
    );
}
