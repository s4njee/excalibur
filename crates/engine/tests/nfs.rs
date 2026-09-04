//! The `NFSv3` backend against a real Linux `nfsd`.
//!
//! Skipped unless `EXCALIBUR_NFS` names the server as `host:port`. The fixture
//! that provides it — and the four export shapes these tests rely on — lives in
//! `tests/fixtures/nfs`; run `up.sh` there first. It is not started from here
//! because it needs a privileged container and a Docker build, which is too
//! much to do per test run.

use excalibur_engine::backend::nfs::{Identity, NfsBackend, NfsTarget};
use excalibur_engine::backend::{EntryKind, ShareBackend, WriteMode};
use excalibur_engine::EngineError;

/// `mountd`'s port. Fixed by the fixture, because behind a port mapping the
/// portmapper reports the port inside the container.
const MOUNT_PORT: u16 = 20048;

/// The server, or `None` to skip.
fn server() -> Option<(String, u16)> {
    let value = std::env::var("EXCALIBUR_NFS").ok()?;
    let (host, port) = value.rsplit_once(':')?;
    Some((host.to_owned(), port.parse().ok()?))
}

/// Connect to one export as the current user.
async fn mount(export: &str) -> Option<NfsBackend> {
    mount_as(export, Identity::default(), false).await
}

async fn mount_as(export: &str, identity: Identity, read_only: bool) -> Option<NfsBackend> {
    let (host, port) = server()?;
    let target = NfsTarget {
        host: &host,
        port,
        mount_port: Some(MOUNT_PORT),
        export,
        identity,
        read_only,
    };
    Some(
        NfsBackend::connect(&target)
            .await
            .unwrap_or_else(|error| panic!("mount {export}: {error}")),
    )
}

#[tokio::test]
async fn lists_reads_and_writes_a_real_export() {
    let Some(backend) = mount("/exports/rw").await else {
        eprintln!("skipping: set EXCALIBUR_NFS to run this");
        return;
    };

    let listing = backend.list_dir("").await.expect("list the root");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"notes.txt"), "{names:?}");
    assert!(names.contains(&"Series"), "{names:?}");
    assert_eq!(listing.mount.protocol_label, "NFSv3");
    // AUTH_SYS is an assertion the server chooses to believe. Anything
    // stronger in this label would be a lie to the user.
    assert_eq!(listing.mount.security_label, "sys");
    assert!(listing.mount.writable, "{}", listing.mount.reason);

    let folder = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Series")
        .expect("the folder");
    assert_eq!(folder.attrs.kind, EntryKind::Dir);
    assert_eq!(folder.kind_label, "Folder");
    // READDIRPLUS carries attributes, so a listing must not need a GETATTR
    // per row to know how big a file is.
    let notes = listing
        .entries
        .iter()
        .find(|entry| entry.name == "notes.txt")
        .expect("the file");
    assert_eq!(notes.attrs.size, 15);
    assert!(notes.attrs.modified_ms.is_some_and(|ms| ms > 0));

    assert_eq!(
        backend
            .read_range("notes.txt", 0, 4096)
            .await
            .expect("read"),
        b"hello from nfs\n"
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

    let capacity = backend.capacity().await.expect("fsstat");
    assert!(capacity.total > 0 && capacity.free <= capacity.total);
}

#[tokio::test]
async fn a_read_only_export_refuses_writes_and_says_why() {
    let Some(backend) = mount("/exports/ro").await else {
        return;
    };
    let info = backend.mount_info();
    assert!(!info.writable);
    // The status bar shows this verbatim, so an empty reason would leave the
    // user with a disabled button and no explanation.
    assert!(!info.reason.is_empty(), "{info:?}");
    assert!(
        info.reason.contains("/exports/ro"),
        "the reason should name the export: {}",
        info.reason
    );

    let refused = backend
        .write_range("blocked.txt", WriteMode::Create, b"nope")
        .await
        .expect_err("a write must be refused");
    assert!(matches!(refused, EngineError::ReadOnly(_)), "{refused:?}");

    // Reading is unaffected.
    assert_eq!(
        backend.read_range("fixed.txt", 0, 5).await.expect("read"),
        b"you m"
    );
}

#[tokio::test]
async fn a_squashed_identity_is_named_rather_than_called_permission_denied() {
    // The export says `rw`. The user is still refused, because uid 0 maps to
    // `nobody` — and "permission denied" would send them hunting through their
    // own account instead of the export line. E5-S3 exists for this case.
    let identity = Identity {
        uid: 0,
        gid: 0,
        gids: Vec::new(),
    };
    let Some(backend) = mount_as("/exports/squash", identity, false).await else {
        return;
    };
    let info = backend.mount_info();
    assert!(!info.writable, "root_squash should refuse a write");
    assert!(
        info.reason.contains("root_squash"),
        "the reason must name the option that caused it: {}",
        info.reason
    );
}

#[tokio::test]
async fn a_read_only_request_is_honoured_without_probing() {
    // Asked-for read-only is not the same as refused: nothing should be
    // created on the server to find out, which would be a surprising thing for
    // a mount the user asked to be read-only to do.
    let Some(backend) = mount_as("/exports/rw", Identity::default(), true).await else {
        return;
    };
    let info = backend.mount_info();
    assert!(!info.writable);
    assert!(info.reason.contains("you asked"), "{}", info.reason);
}

#[tokio::test]
async fn an_export_without_insecure_explains_the_option() {
    // The server wants a source port below 1024. This program is not root and
    // E5-S2 forbids reaching for one, so the only useful thing it can do is
    // say which export option would fix it.
    let Some((host, port)) = server() else { return };
    let refused = NfsBackend::connect(&NfsTarget {
        host: &host,
        port,
        mount_port: Some(MOUNT_PORT),
        export: "/exports/secure",
        identity: Identity::default(),
        read_only: false,
    })
    .await
    .err()
    .expect("the mount must be refused");

    let message = refused.to_string();
    assert!(
        message.contains("insecure"),
        "the error must name the export option: {message}"
    );
    assert!(
        message.contains("privileged"),
        "and say it will not bind a privileged port: {message}"
    );
}

#[tokio::test]
async fn a_large_read_is_reassembled_in_order() {
    // Reads are split across the connection pool, so they complete out of
    // order. A version that concatenated them in completion order rather than
    // by offset passes every test whose file fits in one chunk and corrupts
    // every file that does not.

    /// The seeded blob's size.
    const SIZE: u32 = 1024 * 1024;
    /// A range starting and ending inside different chunks.
    const OFFSET: u32 = 300_000;
    const LENGTH: u32 = 200_000;

    let Some(backend) = mount("/exports/rw").await else {
        return;
    };
    let whole = backend
        .read_range("blob.bin", 0, SIZE)
        .await
        .expect("read the whole file");
    assert_eq!(whole.len(), SIZE as usize);

    // Compare against the same bytes fetched in one piece per region, which
    // would differ if the reassembly were wrong.
    let middle = backend
        .read_range("blob.bin", u64::from(OFFSET), LENGTH)
        .await
        .expect("ranged read");
    assert!(
        middle == whole[OFFSET as usize..(OFFSET + LENGTH) as usize],
        "a ranged read across chunk boundaries came back wrong"
    );

    // Past the end returns what is there, not an error and not padding.
    let tail = backend
        .read_range("blob.bin", u64::from(SIZE) - 10, 4096)
        .await
        .expect("read past the end");
    assert_eq!(tail.len(), 10);
    assert!(backend
        .read_range("blob.bin", u64::from(SIZE) + 1000, 4096)
        .await
        .expect("wholly past the end")
        .is_empty());
}

#[tokio::test]
async fn a_large_write_survives_being_split() {
    // Writes are chunked and committed once at the end. The failure this
    // catches is a chunk landing at the wrong offset, which produces a file of
    // the right length and the wrong contents.
    let Some(backend) = mount("/exports/rw").await else {
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
    assert!(read_back == payload, "a chunked write landed out of order");
    backend
        .remove("big-write.bin", false)
        .await
        .expect("clean up");
}

#[tokio::test]
async fn the_mutations_work_against_a_real_server() {
    let Some(backend) = mount("/exports/rw").await else {
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

    // NFS RENAME is POSIX rename and replaces silently, so refusing an
    // existing destination is a separate lookup this backend does itself.
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
    // The replacing rename is the server's own, so it is atomic — unlike the
    // SFTP backend, which has to stand the old file aside.
    assert!(backend.caps().atomic_rename);

    let not_empty = backend.remove("box", false).await.expect_err("rmdir");
    assert!(matches!(not_empty, EngineError::Io(_)), "{not_empty:?}");

    backend.remove("box", true).await.expect("recursive remove");
    assert!(backend.stat("box").await.is_err());
}

#[tokio::test]
async fn a_directory_larger_than_one_reply_lists_completely() {
    // READDIRPLUS answers with as many entries as fit and expects to be asked
    // again from the last cookie. Stopping at the first reply truncates the
    // listing, which looks like missing files rather than a bug.
    let Some(backend) = mount("/exports/rw").await else {
        return;
    };
    let _ = backend.remove("many", true).await;
    backend.mkdir("many").await.expect("mkdir");
    for index in 0..300 {
        backend
            .write_range(
                &format!("many/file-{index:04}.txt"),
                WriteMode::Create,
                b"x",
            )
            .await
            .expect("write");
    }

    let listing = backend.list_dir("many").await.expect("list");
    assert_eq!(listing.entries.len(), 300, "the listing was truncated");
    backend.remove("many", true).await.expect("clean up");
}

#[tokio::test]
async fn a_path_that_climbs_out_of_the_export_is_refused() {
    let Some(backend) = mount("/exports/rw").await else {
        return;
    };
    // Not followed, because a path leaving the export is either a bug or an
    // attempt, and neither should be served.
    assert!(backend.list_dir("../ro").await.is_err());
    assert!(backend.stat("Series/../../ro/fixed.txt").await.is_err());
}
