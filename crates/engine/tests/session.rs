//! Mount lifecycle, and what happens when a share goes away.
//!
//! The connector is scripted rather than real: the interesting states are a
//! connection that fails, a mount that stops answering, and a share that comes
//! back *different*, and a real server is bad at producing any of those on
//! demand.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::{
    Access, Attrs, Capacity, Caps, ConnectionSpec, Health, Listing, MountInfo, MountStatus,
    Protocol, ShareBackend, WriteMode,
};
use excalibur_engine::{Connector, HealthPolicy, Result, SessionManager};
use tokio::sync::RwLock;

/// A backend whose health the test can flip *after* it has been handed out.
///
/// A connector flag is not enough: the session already holds a backend, and the
/// case being tested is precisely a mount that stops answering while held.
struct ScriptedBackend {
    inner: MockBackend,
    healthy: Arc<AtomicBool>,
}

#[async_trait]
impl ShareBackend for ScriptedBackend {
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
        self.inner.list_dir(path).await
    }
    async fn stat(&self, path: &str) -> Result<Attrs> {
        self.inner.stat(path).await
    }
    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.inner.read_range(path, offset, length).await
    }
    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
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
    async fn ping(&self) -> Result<Health> {
        if self.healthy.load(Ordering::SeqCst) {
            Ok(Health::Healthy)
        } else {
            Ok(Health::Failed)
        }
    }
}

fn spec(id: &str) -> ConnectionSpec {
    ConnectionSpec {
        id: id.to_owned(),
        name: id.to_owned(),
        protocol: Protocol::Xsync,
        host: "example".to_owned(),
        port: None,
        share: "/srv/share".to_owned(),
        username: None,
        mount_preference: Access::Rw,
    }
}

/// A connector under the test's control.
struct ScriptedConnector {
    attempts: AtomicUsize,
    /// Refuse to connect at all.
    refuse: AtomicBool,
    /// Hand back a read-only mount, as an export flipped while running would.
    read_only: AtomicBool,
    /// Shared with every backend handed out, so a live mount can be knocked
    /// over without touching the session that holds it.
    healthy: Arc<AtomicBool>,
    /// What the last `connect` asked for, so a test can check that a remount
    /// actually changed the request rather than reconnecting identically.
    last_preference: Arc<RwLock<Option<Access>>>,
}

impl Default for ScriptedConnector {
    fn default() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
            refuse: AtomicBool::new(false),
            read_only: AtomicBool::new(false),
            healthy: Arc::new(AtomicBool::new(true)),
            last_preference: Arc::new(RwLock::new(None)),
        }
    }
}

impl ScriptedConnector {
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    /// Knock every mount this connector has handed out off the air.
    fn knock_over(&self) {
        self.healthy.store(false, Ordering::SeqCst);
    }

    fn bring_back(&self) {
        self.healthy.store(true, Ordering::SeqCst);
    }

    async fn last_preference(&self) -> Option<Access> {
        *self.last_preference.read().await
    }
}

#[async_trait]
impl Connector for ScriptedConnector {
    async fn connect(&self, _spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        *self.last_preference.write().await = Some(_spec.mount_preference);
        if self.refuse.load(Ordering::SeqCst) {
            return Err(excalibur_engine::EngineError::Disconnected(
                "host is unreachable".to_owned(),
            ));
        }
        let inner = MockBackend::media_share();
        let inner = if self.read_only.load(Ordering::SeqCst) {
            inner.read_only("export mapped `ro,root_squash`")
        } else {
            inner
        };
        Ok(Arc::new(ScriptedBackend {
            inner,
            healthy: Arc::clone(&self.healthy),
        }))
    }
}

fn manager(connector: &Arc<ScriptedConnector>) -> SessionManager {
    SessionManager::with_policy(
        Arc::clone(connector) as Arc<dyn Connector>,
        HealthPolicy {
            interval: Duration::from_millis(50),
            backoff_initial: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
        },
    )
}

#[tokio::test]
async fn a_mount_becomes_ready_and_publishes_its_facts() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = manager(&connector);
    let mut events = sessions.events();

    sessions.mount(spec("media")).await.expect("mount");

    assert_eq!(sessions.status("media").await, Some(MountStatus::Ready));
    assert_eq!(sessions.active().await.as_deref(), Some("media"));
    assert!(sessions.mount_info("media").await.expect("facts").writable);

    // Mounting is two events, not one: a UI shows the row as connecting before
    // it can show it as ready.
    let first = events.recv().await.expect("mounting");
    assert_eq!(first.status, MountStatus::Mounting);
    let second = events.recv().await.expect("ready");
    assert_eq!(second.status, MountStatus::Ready);
    // Every Ready carries the facts, because that is the moment a UI must
    // re-read `writable` rather than trusting what it had.
    assert!(second.mount.expect("facts on ready").writable);
}

#[tokio::test]
async fn a_refused_connection_keeps_the_row_and_says_why() {
    let connector = Arc::new(ScriptedConnector::default());
    connector.refuse.store(true, Ordering::SeqCst);
    let sessions = manager(&connector);

    let error = sessions.mount(spec("media")).await.expect_err("must fail");
    assert!(error.to_string().contains("unreachable"));
    // The row survives, so the UI can show the failure and offer a retry
    // rather than the bookmark vanishing.
    assert_eq!(sessions.status("media").await, Some(MountStatus::Error));
    assert_eq!(sessions.len().await, 1);
    assert!(sessions.backend("media").await.is_none());
}

#[tokio::test]
async fn switching_the_active_mount_does_not_disconnect_the_others() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = manager(&connector);

    sessions.mount(spec("media")).await.expect("first");
    sessions.mount(spec("backups")).await.expect("second");
    assert_eq!(sessions.active().await.as_deref(), Some("backups"));

    sessions.set_active("media").await.expect("switch back");
    assert_eq!(sessions.active().await.as_deref(), Some("media"));

    // Both are still connected: a user switching back should not pay for a
    // reconnect, and two connections were made, not three.
    assert!(sessions.backend("media").await.is_some());
    assert!(sessions.backend("backups").await.is_some());
    assert_eq!(connector.attempts(), 2);

    assert!(sessions.set_active("nope").await.is_err());
}

#[tokio::test]
async fn unmounting_drops_only_that_mount() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = manager(&connector);
    sessions.mount(spec("media")).await.expect("first");
    sessions.mount(spec("backups")).await.expect("second");

    sessions.unmount("backups").await;
    assert_eq!(sessions.len().await, 1);
    assert!(sessions.backend("media").await.is_some());
    // It was the active one, so nothing is active now rather than something
    // arbitrary being promoted.
    assert_eq!(sessions.active().await, None);
    assert!(sessions.active_backend().await.is_err());
}

#[tokio::test]
async fn a_mount_that_stops_answering_reconnects_and_republishes_its_facts() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = manager(&connector);
    let mut events = sessions.events();
    sessions.mount(spec("media")).await.expect("mount");
    assert!(sessions.mount_info("media").await.expect("facts").writable);

    // The share goes away, and comes back read-only — an export flipped while
    // the app was running. This is the case the design cares about: the badge
    // and every write verb must follow, and a manager that only restored the
    // connection would leave them wrong.
    connector.knock_over();

    // Wait until the manager has actually noticed, rather than guessing how
    // long that takes: bringing the share back before the health check saw it
    // fail would test nothing.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    macro_rules! next_event {
        () => {
            tokio::time::timeout(deadline - tokio::time::Instant::now(), events.recv())
                .await
                .expect("the mount never settled")
                .expect("events")
        };
    }
    loop {
        if next_event!().status == MountStatus::Reconnecting {
            break;
        }
    }

    // Now it comes back, read-only.
    connector.read_only.store(true, Ordering::SeqCst);
    connector.bring_back();

    loop {
        let event = next_event!();
        if event.status == MountStatus::Ready {
            let facts = event.mount.expect("ready carries the facts");
            assert!(!facts.writable, "the reconnect must re-read writability");
            assert_eq!(facts.reason, "export mapped `ro,root_squash`");
            break;
        }
    }

    assert_eq!(sessions.status("media").await, Some(MountStatus::Ready));
    let facts = sessions.mount_info("media").await.expect("facts");
    assert!(
        !facts.writable,
        "the stored facts must match what was published"
    );
}

#[tokio::test]
async fn a_share_that_stays_down_keeps_retrying_without_hammering() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = manager(&connector);
    sessions.mount(spec("media")).await.expect("mount");

    // Down, and staying down.
    connector.knock_over();
    connector.refuse.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(
        sessions.status("media").await,
        Some(MountStatus::Reconnecting)
    );
    // Backoff caps at 40ms here, so ~400ms of downtime is on the order of ten
    // attempts. The bound that matters is the upper one: without backoff this
    // would be a tight loop, and the exact count depends on scheduling.
    let attempts = connector.attempts();
    assert!(
        (2..=40).contains(&attempts),
        "expected a backed-off retry loop, saw {attempts} attempts"
    );
    // Nothing can be done with a mount that is down, and asking says so rather
    // than handing back a stale backend.
    assert!(sessions.active_backend().await.is_err());
}

#[tokio::test]
async fn testing_a_connection_reports_the_facts_without_mounting() {
    let connector = Arc::new(ScriptedConnector::default());
    connector.read_only.store(true, Ordering::SeqCst);
    let sessions = manager(&connector);

    // The dialog's Test button asks the questions a user actually has, which
    // includes "will I be able to write" — so it reports the mount facts, not
    // just reachability.
    let facts = sessions.test(&spec("media")).await.expect("test");
    assert!(!facts.writable);
    assert_eq!(facts.reason, "export mapped `ro,root_squash`");

    // Testing is not mounting: nothing is held afterwards.
    assert!(sessions.is_empty().await);
    assert_eq!(sessions.active().await, None);

    connector.refuse.store(true, Ordering::SeqCst);
    let error = sessions.test(&spec("media")).await.expect_err("must fail");
    assert!(error.to_string().contains("unreachable"));
    assert!(sessions.is_empty().await);
}

#[tokio::test]
async fn dropping_the_manager_stops_its_health_tasks() {
    let connector = Arc::new(ScriptedConnector::default());
    {
        let sessions = manager(&connector);
        sessions.mount(spec("media")).await.expect("mount");
        connector.knock_over();
        connector.refuse.store(true, Ordering::SeqCst);
    }
    let after_drop = connector.attempts();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A task that outlived its manager would keep reconnecting to a share
    // nobody holds.
    assert_eq!(connector.attempts(), after_drop);
}

#[tokio::test]
async fn reachability_answers_for_a_listener_and_for_nothing() {
    // A port that is definitely listening, because this test opened it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    assert!(excalibur_engine::session::reachable("127.0.0.1", port, Duration::from_secs(2)).await);

    // And one that is not: the listener is gone.
    drop(listener);
    assert!(
        !excalibur_engine::session::reachable("127.0.0.1", port, Duration::from_millis(300)).await
    );
}

#[tokio::test]
async fn reachability_gives_up_rather_than_hanging() {
    // A host that will not answer. The point of the timeout is that a sidebar
    // full of unreachable bookmarks does not take minutes to render.
    let started = tokio::time::Instant::now();
    assert!(
        !excalibur_engine::session::reachable("192.0.2.1", 445, Duration::from_millis(200)).await
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn remounting_read_write_upgrades_a_read_only_share() {
    let connector = Arc::new(ScriptedConnector::default());
    connector.read_only.store(true, Ordering::SeqCst);
    let sessions = SessionManager::new(Arc::clone(&connector) as Arc<dyn Connector>);
    sessions.mount(spec("media")).await.expect("mount");
    assert!(!sessions.mount_info("media").await.expect("info").writable);

    // The export is remounted read-write by the server this time.
    connector.read_only.store(false, Ordering::SeqCst);
    let info = sessions.remount_read_write("media").await.expect("remount");

    assert!(info.writable);
    // And the manager's own view agrees, so the status bar is not reading a
    // stale MountInfo from before the upgrade.
    assert!(sessions.mount_info("media").await.expect("info").writable);
    assert_eq!(connector.last_preference().await, Some(Access::Rw));
}

#[tokio::test]
async fn a_refused_upgrade_reports_the_reason_rather_than_failing() {
    // The case the design calls out: the button must not become a loop. A
    // share that comes back read-only again is an answer, not an error — a
    // caller that saw `Err` would offer the same button and the user would
    // press it forever.
    let connector = Arc::new(ScriptedConnector::default());
    connector.read_only.store(true, Ordering::SeqCst);
    let sessions = SessionManager::new(Arc::clone(&connector) as Arc<dyn Connector>);
    sessions.mount(spec("media")).await.expect("mount");
    let before = connector.attempts();

    let info = sessions
        .remount_read_write("media")
        .await
        .expect("a refused upgrade is Ok, not Err");

    assert!(!info.writable);
    assert!(info.reason.contains("root_squash"), "{}", info.reason);
    // Exactly one further connection: it asked once and reported the answer.
    assert_eq!(connector.attempts(), before + 1);
}

#[tokio::test]
async fn a_remounted_share_keeps_asking_for_read_write_when_it_reconnects() {
    // Otherwise a drop silently reverts the mount to whatever was first typed
    // in the dialog, and the user's recovery is undone by a network blip.
    let connector = Arc::new(ScriptedConnector::default());
    connector.read_only.store(true, Ordering::SeqCst);
    let policy = HealthPolicy {
        interval: Duration::from_millis(20),
        backoff_initial: Duration::from_millis(10),
        backoff_max: Duration::from_millis(20),
    };
    let sessions =
        SessionManager::with_policy(Arc::clone(&connector) as Arc<dyn Connector>, policy);

    let mut ro = spec("media");
    ro.mount_preference = Access::Ro;
    sessions.mount(ro).await.expect("mount");
    assert_eq!(connector.last_preference().await, Some(Access::Ro));

    connector.read_only.store(false, Ordering::SeqCst);
    sessions.remount_read_write("media").await.expect("remount");

    // A drop and a reconnect must not go back to asking for read-only.
    connector.knock_over();
    tokio::time::sleep(Duration::from_millis(120)).await;
    connector.bring_back();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(connector.last_preference().await, Some(Access::Rw));
}

#[tokio::test]
async fn remounting_a_mount_that_is_not_there_is_an_error() {
    let connector = Arc::new(ScriptedConnector::default());
    let sessions = SessionManager::new(connector as Arc<dyn Connector>);
    assert!(sessions.remount_read_write("nothing").await.is_err());
}
