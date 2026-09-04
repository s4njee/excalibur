//! The SMB backend against a real Samba server.
//!
//! Compiled only with the `smb` feature, which is the same feature that makes
//! the binary GPL-3.0-or-later — see `backend/smb.rs`.
//!
//! Skipped unless `EXCALIBUR_SMB` names the server as `host`. libsmbclient
//! offers no way to set the port, so the server must be on 445. Bring one up
//! with `tests/fixtures/smb/up.sh`.
#![cfg(feature = "smb")]

use excalibur_engine::backend::smb::{SmbBackend, SmbTarget};
use excalibur_engine::backend::{EntryKind, ShareBackend, WriteMode};
use excalibur_engine::EngineError;

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
