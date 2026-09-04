//! The SMB backend against a real Samba server.
//!
//! Compiled only with the `smb` feature, which is the same feature that makes
//! the binary GPL-3.0-or-later — see `backend/smb.rs`.
//!
//! Skipped unless `EXCALIBUR_SMB` names the server as `host`. libsmbclient
//! offers no way to set the port, so the server must be on 445. Bring one up
//! with `tests/fixtures/smb/up.sh`.
#![cfg(feature = "smb")]

use std::sync::Arc;
use std::time::Duration;

use excalibur_engine::backend::smb::{SmbBackend, SmbTarget};
use excalibur_engine::backend::{
    Access, ConnectionSpec, EntryKind, MountStatus, Protocol, ShareBackend, WriteMode,
};
use excalibur_engine::error::Surface;
use excalibur_engine::{Connector, EngineError, HealthPolicy, SessionManager};

fn host() -> Option<String> {
    std::env::var("EXCALIBUR_SMB").ok()
}

async fn mount(share: &str, read_only: bool) -> Option<SmbBackend> {
    let host = host()?;
    let target = SmbTarget {
        host: &host,
        port: None,
        share,
        username: "excalibur",
        password: "excalibur1",
        workgroup: "WORKGROUP",
        read_only,
    };
    Some(
        SmbBackend::connect(&target)
            .await
            .unwrap_or_else(|error| panic!("mount {share}: {error}")),
    )
}

#[tokio::test]
async fn lists_reads_and_writes_a_real_share() {
    let Some(backend) = mount("public", false).await else {
        eprintln!("skipping: set EXCALIBUR_SMB to run this");
        return;
    };

    let listing = backend.list_dir("").await.expect("list the root");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"notes.txt"), "{names:?}");
    assert!(names.contains(&"Series"), "{names:?}");
    assert_eq!(listing.mount.protocol_label, "SMB");
    assert!(listing.mount.writable, "{}", listing.mount.reason);

    let folder = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Series")
        .expect("the folder");
    assert_eq!(folder.attrs.kind, EntryKind::Dir);
    assert_eq!(folder.kind_label, "Folder");

    let notes = listing
        .entries
        .iter()
        .find(|entry| entry.name == "notes.txt")
        .expect("the file");
    assert_eq!(notes.attrs.size, 17);
    assert!(notes.attrs.modified_ms.is_some_and(|ms| ms > 0));

    assert_eq!(
        backend
            .read_range("notes.txt", 0, 4096)
            .await
            .expect("read"),
        b"hello from samba\n"
    );
    // A ranged read must land at the offset, not at the start.
    assert_eq!(
        backend.read_range("notes.txt", 6, 4).await.expect("ranged"),
        b"from"
    );

    backend
        .write_range("fresh.txt", WriteMode::Create, b"written by excalibur")
        .await
        .expect("write");
    assert_eq!(
        backend
            .read_range("fresh.txt", 0, 4096)
            .await
            .expect("read back"),
        b"written by excalibur"
    );
    backend.remove("fresh.txt", false).await.expect("clean up");

    // The capacity units trap: against a server without UNIX extensions Samba
    // packs `frsize` with sectors-per-unit and `bsize` with bytes-per-sector,
    // so a naive `blocks * frsize` reports a multi-terabyte share as a few
    // gigabytes. A sane total is the cheapest check that it was not done.
    let capacity = backend.capacity().await.expect("statvfs");
    assert!(capacity.total > 1 << 30, "suspiciously small: {capacity:?}");
    assert!(capacity.free <= capacity.total);
}

#[tokio::test]
async fn a_read_only_share_refuses_writes_and_says_why() {
    let Some(backend) = mount("readonly", false).await else {
        return;
    };
    let info = backend.mount_info();
    assert!(!info.writable);
    // There is no share-level flag to read through libsmbclient, so this
    // verdict comes from actually trying to create a file.
    assert!(!info.reason.is_empty(), "{info:?}");

    let refused = backend
        .write_range("blocked.txt", WriteMode::Create, b"nope")
        .await
        .expect_err("a write must be refused");
    assert!(matches!(refused, EngineError::ReadOnly(_)), "{refused:?}");

    assert_eq!(
        backend.read_range("fixed.txt", 0, 5).await.expect("read"),
        b"you m"
    );
}

#[tokio::test]
async fn a_read_only_request_is_honoured_without_probing() {
    let Some(backend) = mount("public", true).await else {
        return;
    };
    let info = backend.mount_info();
    assert!(!info.writable);
    assert!(info.reason.contains("you asked"), "{}", info.reason);
}

#[tokio::test]
async fn a_large_read_and_write_round_trip() {
    let Some(backend) = mount("public", false).await else {
        return;
    };
    let payload: Vec<u8> = (0..900_000_u32).map(|index| (index % 253) as u8).collect();
    let written = backend
        .write_range("big-write.bin", WriteMode::Create, &payload)
        .await
        .expect("write");
    assert_eq!(written, payload.len() as u64);

    let read_back = backend
        .read_range("big-write.bin", 0, 900_000)
        .await
        .expect("read back");
    assert!(read_back == payload, "a large round trip came back wrong");

    // A ranged read inside a large file, which is where a seek that was
    // dropped or applied twice shows up.
    let middle = backend
        .read_range("big-write.bin", 400_000, 1000)
        .await
        .expect("ranged");
    assert_eq!(middle, payload[400_000..401_000]);

    backend
        .remove("big-write.bin", false)
        .await
        .expect("clean up");
}

#[tokio::test]
async fn a_resumed_write_lands_at_its_offset() {
    let Some(backend) = mount("public", false).await else {
        return;
    };
    backend
        .write_range("part.bin", WriteMode::Create, b"AAAA")
        .await
        .expect("first");
    backend
        .write_range("part.bin", WriteMode::Resume { offset: 2 }, b"BB")
        .await
        .expect("resume");
    assert_eq!(
        backend.read_range("part.bin", 0, 16).await.expect("read"),
        b"AABB"
    );
    backend.remove("part.bin", false).await.expect("clean up");
}

#[tokio::test]
async fn the_mutations_work_against_a_real_server() {
    let Some(backend) = mount("public", false).await else {
        return;
    };
    let _ = backend.remove("box", true).await;

    backend.mkdir("box").await.expect("mkdir");
    backend
        .write_range("box/one.txt", WriteMode::Create, b"one")
        .await
        .expect("write");
    backend
        .write_range("box/two.txt", WriteMode::Create, b"two")
        .await
        .expect("write");

    // Samba's rename clobbers and offers no no-clobber flag, so this refusal
    // is the backend's own check rather than the server's.
    let clash = backend
        .rename("box/one.txt", "box/two.txt", false)
        .await
        .expect_err("no-replace rename");
    assert!(matches!(clash, EngineError::AlreadyExists(_)), "{clash:?}");

    backend
        .rename("box/one.txt", "box/two.txt", true)
        .await
        .expect("replacing rename");
    assert_eq!(
        backend
            .read_range("box/two.txt", 0, 16)
            .await
            .expect("read"),
        b"one"
    );
    // And it is *not* claimed atomic: Samba unlinks the destination first, so
    // the transfer worker must use its safe path.
    assert!(!backend.caps().atomic_rename);

    assert!(
        backend.remove("box", false).await.is_err(),
        "rmdir non-empty"
    );
    backend.remove("box", true).await.expect("recursive remove");
    assert!(backend.stat("box").await.is_err());
}

// ---------------------------------------------------------------------------
// E4-S6 — reconnect
// ---------------------------------------------------------------------------

/// Builds real SMB backends, so the session manager reconnects for real.
struct SmbConnector {
    host: String,
}

#[async_trait::async_trait]
impl Connector for SmbConnector {
    async fn connect(
        &self,
        spec: &ConnectionSpec,
    ) -> excalibur_engine::Result<Arc<dyn ShareBackend>> {
        let backend = SmbBackend::connect(&SmbTarget {
            host: &self.host,
            port: None,
            share: &spec.share,
            username: "excalibur",
            password: "excalibur1",
            workgroup: "WORKGROUP",
            read_only: spec.mount_preference == Access::Ro,
        })
        .await?;
        Ok(Arc::new(backend))
    }
}

fn spec() -> ConnectionSpec {
    ConnectionSpec {
        id: "smb-reconnect".to_owned(),
        name: "public".to_owned(),
        protocol: Protocol::Smb,
        host: "127.0.0.1".to_owned(),
        port: None,
        share: "public".to_owned(),
        username: Some("excalibur".to_owned()),
        domain: Some("WORKGROUP".to_owned()),
        mount_preference: Access::Rw,
    }
}

fn docker(args: &[&str]) {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("docker {args:?}: {error}"));
}

/// These two tests kill the server, so they are opt-in.
///
/// Every other test in this file would fail while the server is down, and
/// `cargo test` runs them in parallel by default. Rather than make the whole
/// suite serial for the sake of two cases, they run only when asked:
///
/// ```sh
/// EXCALIBUR_SMB=127.0.0.1 EXCALIBUR_SMB_DISRUPTIVE=1 \
///   cargo test -p excalibur-engine --features smb --test smb -- --test-threads=1
/// ```
fn disruptive_allowed() -> bool {
    std::env::var("EXCALIBUR_SMB_DISRUPTIVE").is_ok()
}

/// Restarts the server when it goes out of scope, panic or not.
///
/// Without this an assertion failure leaves the container down, and every
/// later run of this file fails for a reason that has nothing to do with the
/// code under test.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        docker(&["start", "excalibur-samba"]);
        // Block until it answers. `Drop` cannot await, and returning early
        // would hand the next test a server that is not up yet.
        for _ in 0..120 {
            if std::net::TcpStream::connect("127.0.0.1:445").is_ok() {
                std::thread::sleep(Duration::from_secs(2));
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// Wait for a mount to reach a status, or give up.
async fn wait_for(
    sessions: &SessionManager,
    id: &str,
    want: MountStatus,
    within: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        if sessions.status(id).await == Some(want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn a_dropped_session_reconnects_and_re_reads_writable() {
    // The whole of E4-S6 in one test: the server goes away, the mount notices,
    // the manager reconnects — which for SMB means a fresh tree connect,
    // because there is no session to resume — and `writable` is established
    // again from the new connection rather than remembered from the old one.
    if host().is_none() || !disruptive_allowed() {
        eprintln!("skipping: needs EXCALIBUR_SMB and EXCALIBUR_SMB_DISRUPTIVE");
        return;
    }
    let _guard = ServerGuard;
    let sessions = SessionManager::with_policy(
        Arc::new(SmbConnector {
            host: "127.0.0.1".to_owned(),
        }) as Arc<dyn Connector>,
        HealthPolicy {
            interval: Duration::from_millis(200),
            backoff_initial: Duration::from_millis(200),
            // Samba takes a few seconds to come back, so the retry has to be
            // patient enough to still be trying when it does.
            backoff_max: Duration::from_secs(2),
        },
    );

    sessions.mount(spec()).await.expect("mount");
    assert_eq!(
        sessions.status("smb-reconnect").await,
        Some(MountStatus::Ready)
    );
    assert!(
        sessions
            .mount_info("smb-reconnect")
            .await
            .expect("info")
            .writable
    );

    // `kill`, not `stop`: an abrupt drop is the case the AC names, and a
    // graceful shutdown would let the server close the session politely.
    docker(&["kill", "excalibur-samba"]);
    assert!(
        wait_for(
            &sessions,
            "smb-reconnect",
            MountStatus::Reconnecting,
            Duration::from_secs(20)
        )
        .await,
        "the mount never noticed the server had gone"
    );

    docker(&["start", "excalibur-samba"]);
    assert!(
        wait_for(
            &sessions,
            "smb-reconnect",
            MountStatus::Ready,
            Duration::from_secs(60)
        )
        .await,
        "the mount never came back"
    );

    // Re-read, not remembered. The mount could have come back read-only — an
    // export flipped while the app was running — and republishing the facts is
    // the point of reconnecting rather than a side effect of it.
    let info = sessions.mount_info("smb-reconnect").await.expect("info");
    assert!(info.writable, "{}", info.reason);
    assert_eq!(info.protocol_label, "SMB");

    // And the mount is usable again, which a status alone does not prove.
    let backend = sessions.backend("smb-reconnect").await.expect("backend");
    assert_eq!(
        backend.read_range("notes.txt", 0, 5).await.expect("read"),
        b"hello"
    );

    sessions.unmount("smb-reconnect").await;
}

#[tokio::test]
async fn a_dropped_session_reads_as_disconnected_not_as_an_io_error() {
    // Where the failure is shown depends on how it is classified: a dropped
    // mount belongs in the banner, an I/O failure on a transfer row. From
    // libsmbclient a dead tree arrives as a bad file descriptor on a listing
    // and as `ECONNREFUSED` on a stat, and both mean the mount is gone.
    if !disruptive_allowed() {
        return;
    }
    let Some(backend) = mount("public", false).await else {
        return;
    };
    let _guard = ServerGuard;
    docker(&["kill", "excalibur-samba"]);

    let listing = backend.list_dir("").await.expect_err("the mount is gone");
    let stat = backend
        .stat("notes.txt")
        .await
        .expect_err("the mount is gone");

    for error in [&listing, &stat] {
        assert!(
            matches!(error, EngineError::Disconnected(_)),
            "a dropped mount must read as disconnected, got {error:?}"
        );
        // Both feed the reconnect loop, which stops on anything it cannot fix.
        assert!(error.transient(), "{error:?}");
        assert_eq!(error.surface(), Surface::Banner, "{error:?}");
    }
}
