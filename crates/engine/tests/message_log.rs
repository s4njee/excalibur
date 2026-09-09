//! What the message log records (`twopane.md` XTP-E5-S1).
//!
//! The handoff calls the log "where protocol truth lives", which makes two
//! things testable: that a mount's life reaches it — what was asked for, what
//! came back, and what can be done about a read-only export — and that every
//! verb crossing the `ShareBackend` seam does too, because a log assembled by
//! remembering to write a line beside each call is a log missing whichever
//! call was added last.

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::logged::LoggedBackend;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::{
    Access, Attrs, Capacity, Caps, ConnectionSpec, Health, Listing, MountInfo, Protocol,
    ShareBackend, WriteMode,
};
use excalibur_engine::log::{LogAction, LogCategory, LogLine, MessageLog, LOG_CAPACITY};
use excalibur_engine::{Connector, EngineError, Result, SessionManager};

fn spec(id: &str) -> ConnectionSpec {
    ConnectionSpec {
        id: id.to_owned(),
        name: id.to_owned(),
        protocol: Protocol::Xsync,
        host: "freya.local".to_owned(),
        port: None,
        share: "/mnt/data".to_owned(),
        username: None,
        domain: None,
        mount_preference: Access::Rw,
    }
}

/// A connector that hands out a mock, optionally read-only.
struct Scripted {
    read_only: bool,
}

#[async_trait]
impl Connector for Scripted {
    async fn connect(&self, _spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        let backend = if self.read_only {
            MockBackend::media_share().read_only("Read-only — export mapped ro,root_squash")
        } else {
            MockBackend::media_share()
        };
        Ok(Arc::new(backend))
    }
}

/// A backend whose every verb fails, for the failure lines.
struct Broken;

#[async_trait]
impl ShareBackend for Broken {
    fn mount_info(&self) -> MountInfo {
        MockBackend::media_share().mount_info()
    }
    fn caps(&self) -> Caps {
        MockBackend::media_share().caps()
    }
    async fn capacity(&self) -> Result<Capacity> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn list_dir(&self, _path: &str) -> Result<Listing> {
        Err(EngineError::PermissionDenied("Films".to_owned()))
    }
    async fn stat(&self, _path: &str) -> Result<Attrs> {
        Err(EngineError::NotFound("gone".to_owned()))
    }
    async fn read_range(&self, _path: &str, _offset: u64, _length: u32) -> Result<Vec<u8>> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn write_range(&self, _path: &str, _mode: WriteMode, _data: &[u8]) -> Result<u64> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn mkdir(&self, _path: &str) -> Result<()> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn rename(&self, _from: &str, _to: &str, _replace: bool) -> Result<()> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn remove(&self, _path: &str, _recursive: bool) -> Result<()> {
        Err(EngineError::PermissionDenied("no".to_owned()))
    }
    async fn ping(&self) -> Result<Health> {
        Ok(Health::Healthy)
    }
}

fn texts(log: &MessageLog, category: LogCategory) -> Vec<String> {
    log.lines()
        .into_iter()
        .filter(|line| line.category == category)
        .map(|line| line.text)
        .collect()
}

#[tokio::test]
async fn a_mount_says_what_was_asked_for_and_what_came_back() {
    let sessions =
        SessionManager::new(Arc::new(Scripted { read_only: false }) as Arc<dyn Connector>);
    let log = sessions.log();
    sessions.mount("main", spec("media")).await.expect("mount");

    // The spec's own fields, not the bookmark's name: "media" does not say
    // which host it is on, and the log is where protocol truth lives.
    assert_eq!(
        texts(&log, LogCategory::Command),
        vec!["mount xsync://freya.local/mnt/data (rw)"]
    );
    assert_eq!(texts(&log, LogCategory::Status), vec!["mounted read-write"]);
}

#[tokio::test]
async fn a_read_only_export_offers_its_own_recovery() {
    // `2b` draws exactly this pair: the status line naming the export, and a
    // hint under it offering `Remount as RW`.
    let sessions =
        SessionManager::new(Arc::new(Scripted { read_only: true }) as Arc<dyn Connector>);
    let log = sessions.log();
    sessions.mount("main", spec("media")).await.expect("mount");

    let hints: Vec<LogLine> = log
        .lines()
        .into_iter()
        .filter(|line| line.category == LogCategory::Hint)
        .collect();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].text, "Read-only — export mapped ro,root_squash");
    // The verb, not a label: the shell is not matching on prose to decide
    // which button to draw.
    assert_eq!(hints[0].action, Some(LogAction::RemountRw));
}

#[tokio::test]
async fn a_writable_mount_offers_nothing_to_recover_from() {
    let sessions =
        SessionManager::new(Arc::new(Scripted { read_only: false }) as Arc<dyn Connector>);
    let log = sessions.log();
    sessions.mount("main", spec("media")).await.expect("mount");
    assert!(texts(&log, LogCategory::Hint).is_empty());
}

#[tokio::test]
async fn every_verb_that_crosses_the_seam_is_recorded() {
    let log = Arc::new(MessageLog::new());
    let backend = LoggedBackend::new(
        Arc::new(MockBackend::media_share()),
        Arc::clone(&log),
        "media",
    );

    backend.list_dir("").await.expect("list");
    backend.mkdir("Films").await.expect("mkdir");

    let commands = texts(&log, LogCategory::Command);
    // The share root reads as `/`, which is what the path bar shows.
    assert_eq!(commands, vec!["list_dir /", "mkdir Films"]);
    // A listing says how many rows and how long, which is the design's
    // "listing timings" in one line.
    let listings = texts(&log, LogCategory::Listing);
    assert_eq!(listings.len(), 1);
    assert!(listings[0].contains("entries in"), "{}", listings[0]);
}

#[tokio::test]
async fn a_failure_is_recorded_in_the_words_the_backend_used() {
    // Verbatim: paraphrasing loses the detail that says whether it is the
    // export or the credentials.
    let log = Arc::new(MessageLog::new());
    let backend = LoggedBackend::new(Arc::new(Broken), Arc::clone(&log), "media");

    assert!(backend.list_dir("Films").await.is_err());

    let errors = texts(&log, LogCategory::Error);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("permission denied"), "{}", errors[0]);
    // And no listing line claiming a result that never arrived.
    assert!(texts(&log, LogCategory::Listing).is_empty());
}

#[tokio::test]
async fn moving_bytes_does_not_fill_the_log() {
    // A transfer makes thousands of range reads. A log they fill is one
    // nobody can read, so only the verbs a person performs get a line.
    let log = Arc::new(MessageLog::new());
    let backend = LoggedBackend::new(
        Arc::new(MockBackend::media_share()),
        Arc::clone(&log),
        "media",
    );

    for offset in 0..50 {
        let _ = backend.read_range("Films/a.mkv", offset, 1).await;
    }
    assert!(log.lines().is_empty());
}

#[test]
fn the_ring_drops_its_oldest_line_rather_than_growing() {
    // A log that grows with uptime is a leak with a nice name.
    let log = MessageLog::new();
    for index in 0..LOG_CAPACITY + 10 {
        log.record(LogLine::new(LogCategory::Status, format!("line {index}")));
    }
    let lines = log.lines();
    assert_eq!(lines.len(), LOG_CAPACITY);
    assert_eq!(lines[0].text, "line 10");
    assert_eq!(
        lines[lines.len() - 1].text,
        format!("line {}", LOG_CAPACITY + 9)
    );
}

#[tokio::test]
async fn a_subscriber_hears_lines_as_they_are_written() {
    let log = MessageLog::new();
    let mut lines = log.subscribe();
    log.record(LogLine::new(LogCategory::Status, "mounted"));
    let heard = lines.recv().await.expect("a line");
    assert_eq!(heard.text, "mounted");
}

#[test]
fn clearing_empties_the_ring() {
    let log = MessageLog::new();
    log.record(LogLine::new(LogCategory::Status, "mounted"));
    log.clear();
    assert!(log.lines().is_empty());
}
