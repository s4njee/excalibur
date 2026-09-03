//! The xsync backend against a real `xs --server`.
//!
//! Skipped unless `EXCALIBUR_XS` names an `xs` binary. The alternative would be
//! either a mock of the protocol — which would test the mock — or a hard
//! dependency on a sibling checkout being built, which would fail the suite for
//! anyone who has not built it.

use std::path::PathBuf;

use excalibur_engine::backend::xsync::XsyncBackend;
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
async fn the_verbs_protocol_v3_lacks_are_refused_as_unsupported() {
    let Some(xs) = xs_binary() else {
        eprintln!("skipping: set EXCALIBUR_XS to an xs binary to run this");
        return;
    };
    let root = share();
    let backend = XsyncBackend::connect_local(&xs, root.path().to_str().unwrap(), false)
        .await
        .expect("mount the export");

    // These are not failures of this backend but gaps in the protocol
    // (`xsyncv3.md` E5-S4). A UI reads `Unsupported` and disables the verb,
    // rather than offering it and reporting a confusing error.
    for error in [
        backend.mkdir("new").await.expect_err("mkdir"),
        backend.rename("a", "b", false).await.expect_err("rename"),
        backend
            .remove("notes.txt", false)
            .await
            .expect_err("remove"),
    ] {
        assert!(
            matches!(error, EngineError::Unsupported(_)),
            "expected Unsupported, got {error}"
        );
    }
    assert!(
        !backend.caps().atomic_rename,
        "no rename verb means no atomic promote"
    );
    assert!(backend.caps().random_access);
}
