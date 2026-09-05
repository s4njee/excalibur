//! The xsync backend against a real `xs --server`.
//!
//! Skipped unless `EXCALIBUR_XS` names an `xs` binary. The alternative would be
//! either a mock of the protocol — which would test the mock — or a hard
//! dependency on a sibling checkout being built, which would fail the suite for
//! anyone who has not built it.

use std::path::PathBuf;

use excalibur_engine::backend::xsync::XsyncBackend;
use excalibur_engine::backend::CasOutcome;
use excalibur_engine::{EngineError, EntryKind, ShareBackend, WriteMode};

/// The `xs` binary to serve with, or `None` to skip.
fn xs_binary() -> Option<String> {
    let path = std::env::var("EXCALIBUR_XS").ok()?;
    PathBuf::from(&path).is_file().then_some(path)
}

/// A share with a couple of files and a directory in it.
fn share() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(root.path().join("Documentaries")).unwrap();
    std::fs::write(root.path().join("notes.txt"), b"hello from a real export").unwrap();
    std::fs::write(root.path().join("Arrival.2016.mkv"), vec![7_u8; 4096]).unwrap();
    std::fs::write(root.path().join(".DS_Store"), b"x").unwrap();
    root
}

#[tokio::test]
async fn lists_reads_and_writes_a_real_export() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    let listing = backend.list_dir("").await.expect("list the root");
    assert_eq!(listing.entries.len(), 4);
    assert!(listing.mount.writable);
    assert_eq!(listing.mount.protocol_label, "xsync v3");

    let film = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Arrival.2016.mkv")
        .expect("the film");
    assert_eq!(film.attrs.size, 4096);
    // The kind label is the engine's, from the name; the protocol has no
    // opinion about it.
    assert_eq!(film.kind_label, "Matroska Video");
    assert_eq!(film.attrs.kind, EntryKind::File);
    assert!(film.attrs.modified_ms.is_some_and(|ms| ms > 0));

    let hidden = listing
        .entries
        .iter()
        .find(|entry| entry.name == ".DS_Store")
        .expect("the dotfile");
    assert!(hidden.hidden);

    let folder = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Documentaries")
        .expect("the folder");
    assert_eq!(folder.attrs.kind, EntryKind::Dir);

    // Reads go through the protocol's verified path: the server hashes what it
    // sends and the client checks it before this returns.
    let bytes = backend
        .read_range("notes.txt", 0, 4096)
        .await
        .expect("read");
    assert_eq!(bytes, b"hello from a real export");
    let ranged = backend
        .read_range("notes.txt", 6, 4)
        .await
        .expect("ranged read");
    assert_eq!(ranged, b"from");

    // A file created through the backend must be readable afterwards, which is
    // not automatic: a create that sends mode 0 produces a file nobody can open.
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

    let capacity = backend.capacity().await.expect("capacity");
    assert!(capacity.total > 0 && capacity.free <= capacity.total);
}

#[tokio::test]
async fn a_read_only_export_refuses_writes_and_says_why() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), true)
        .await
        .expect("mount the export read-only");

    let listing = backend.list_dir("").await.expect("list the root");
    assert!(!listing.mount.writable);
    // The status bar shows this verbatim, so an empty reason would leave the
    // user with a disabled button and no explanation.
    assert!(!listing.mount.reason.is_empty());

    let refused = backend
        .write_range("blocked.txt", WriteMode::Create, b"nope")
        .await
        .expect_err("a write must be refused");
    assert!(
        matches!(refused, EngineError::ReadOnly(_)),
        "refused for the wrong reason: {refused}"
    );
    assert!(!root.path().join("blocked.txt").exists());

    // Reading is unaffected.
    assert_eq!(
        backend.read_range("notes.txt", 0, 5).await.expect("read"),
        b"hello"
    );
}

#[tokio::test]
async fn the_namespace_mutations_work_against_a_real_server() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    backend.mkdir("box").await.expect("mkdir");
    assert!(root.path().join("box").is_dir());

    // `mkdir`, not `mkdir -p`: the parent has to exist, and a caller that
    // wants the parents makes them itself so it knows what it made.
    let deep = backend.mkdir("absent/child").await.expect_err("mkdir deep");
    assert!(
        matches!(deep, EngineError::NotFound(_)),
        "expected NotFound, got {deep}"
    );

    backend
        .write_range("box/note.txt", WriteMode::Create, b"hello")
        .await
        .expect("write");

    // `replace: false` refuses an existing destination rather than clobbering
    // it; the server decides that atomically, so there is no window here.
    backend
        .write_range("box/other.txt", WriteMode::Create, b"other")
        .await
        .expect("write");
    let clash = backend
        .rename("box/other.txt", "box/note.txt", false)
        .await
        .expect_err("no-replace rename");
    assert!(
        matches!(clash, EngineError::AlreadyExists(_)),
        "expected AlreadyExists, got {clash}"
    );
    backend
        .rename("box/other.txt", "box/note.txt", true)
        .await
        .expect("replacing rename");
    assert_eq!(
        std::fs::read(root.path().join("box/note.txt")).unwrap(),
        b"other"
    );

    // A non-recursive remove of a directory with entries fails rather than
    // taking them with it.
    let not_empty = backend.remove("box", false).await.expect_err("rmdir");
    assert!(
        matches!(
            not_empty,
            EngineError::Io(_) | EngineError::PermissionDenied(_)
        ),
        "expected the server's refusal, got {not_empty}"
    );

    backend.remove("box", true).await.expect("recursive remove");
    assert!(!root.path().join("box").exists());

    // The promote path is real now, so the transfer worker may use it.
    assert!(backend.caps().atomic_rename);
    assert!(backend.caps().random_access);
}

#[tokio::test]
async fn a_recursive_remove_clears_a_nested_tree() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    // Depth matters: the walk removes children before parents, and a version
    // that got the order wrong would fail on the first non-empty directory.
    backend.mkdir("a").await.expect("mkdir a");
    backend.mkdir("a/b").await.expect("mkdir a/b");
    backend.mkdir("a/b/c").await.expect("mkdir a/b/c");
    for path in ["a/one.txt", "a/b/two.txt", "a/b/c/three.txt"] {
        backend
            .write_range(path, WriteMode::Create, b"x")
            .await
            .expect("write");
    }

    backend.remove("a", true).await.expect("recursive remove");
    assert!(!root.path().join("a").exists());
}

#[tokio::test]
async fn a_guarded_write_is_atomic_and_refuses_a_file_that_moved() {
    // E12's write-back over the real protocol: the cookie is checked on the
    // server immediately before the publishing rename, so nothing can slip
    // between the check and the write the way the portable default allows.
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    let before = backend.stat("notes.txt").await.expect("stat");
    // The protocol carries a real cookie, which is what makes this exact
    // rather than a size-and-mtime guess.
    assert!(before.change_cookie.is_some());

    // Unchanged: the write goes through.
    let written = backend
        .write_all_if_unchanged("notes.txt", Some(&before), b"first edit")
        .await
        .expect("guarded write");
    assert!(matches!(written, CasOutcome::Written(_)), "{written:?}");
    assert_eq!(
        std::fs::read(root.path().join("notes.txt")).unwrap(),
        b"first edit"
    );

    // The old cookie is now stale, so a second write with it is refused and
    // the file is left exactly as it was.
    let refused = backend
        .write_all_if_unchanged("notes.txt", Some(&before), b"second edit")
        .await
        .expect("the guard resolves rather than failing");
    assert!(matches!(refused, CasOutcome::Changed(_)), "{refused:?}");
    assert_eq!(
        std::fs::read(root.path().join("notes.txt")).unwrap(),
        b"first edit",
        "a refused write published anyway"
    );
    // And the refused bytes are gone from the server, not parked in a stage
    // for the next save to resume. Found 2026-09-05: the third write below
    // used to inherit "second edit" from the abandoned stage and fail its
    // digest, because `StageOpen` with an empty token adopts any stage for
    // the destination.
    let leftovers: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".xsync.tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a refused write left a stage behind: {leftovers:?}"
    );

    // No expectation writes unconditionally.
    let forced = backend
        .write_all_if_unchanged("notes.txt", None, b"forced")
        .await
        .expect("unconditional write");
    assert!(matches!(forced, CasOutcome::Written(_)));
    assert_eq!(
        std::fs::read(root.path().join("notes.txt")).unwrap(),
        b"forced"
    );
}

#[tokio::test]
async fn the_mount_facts_a_listing_carries_include_capacity() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    // Seeded at mount, so the first listing already has a number to carry
    // rather than the second one (E7-S5).
    let mounted = backend
        .mount_info()
        .capacity
        .expect("mounting reads StatFs once");
    assert!(mounted.total > 0, "{mounted:?}");
    assert!(mounted.free <= mounted.total, "{mounted:?}");

    // The window never calls `capacity()`: the status bar reads the mount
    // facts that rode along with the listing (E10-S1), so a capacity that is
    // only available from its own call is a capacity nothing ever shows.
    let listing = backend.list_dir("").await.expect("list the root");
    assert_eq!(listing.mount.capacity, Some(mounted));
}
