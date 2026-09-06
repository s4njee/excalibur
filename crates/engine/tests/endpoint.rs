//! Transfers through xsync's planner with a `ShareBackend` as one end (E22-S2).
//!
//! These are the pairs E22-S1 does not cover. Two backends stand in for all of
//! them and they cover different halves of the seam: [`MockBackend`] is an
//! in-memory share with no filesystem underneath, which is what an NFS, SMB or
//! SFTP mount looks like to this code — no `local_root`, no reflink, no hash
//! cache; [`LocalBackend`] is a real directory browsed as a mount (E21), which
//! is the local↔local pair the AC names and the one where a bug would be
//! hidden by both sides being `std::fs`.
//!
//! The case worth writing down first is the one a user reported: **a folder
//! dragged from Finder into a share**. There was no upload planner, so the
//! worker read the directory as a file and the transfer failed with `EISDIR`.

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::ShareBackend;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::{Direction, Status, Transfer};
use excalibur_engine::Result;

struct Fixed(Arc<dyn ShareBackend>);

#[async_trait]
impl BackendSource for Fixed {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.0))
    }
}

async fn run_one(share: &Arc<dyn ShareBackend>, transfer: Transfer) -> Transfer {
    let queue = Arc::new(TransferQueue::in_memory());
    queue.add(transfer).await;
    Arc::new(Worker::new(
        Arc::clone(&queue),
        Arc::new(Fixed(Arc::clone(share))),
        1,
    ))
    .drain()
    .await;
    queue.all().await.remove(0)
}

/// A folder on disk with a file at every depth a walk has to reach.
///
/// An empty subdirectory is in there deliberately: it has no file to make its
/// parent on the way past, so it is the one a copy silently loses.
fn tree(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("season 1/extras")).unwrap();
    std::fs::create_dir_all(root.join("season 2")).unwrap();
    std::fs::create_dir_all(root.join("artwork")).unwrap();
    std::fs::write(root.join("readme.txt"), b"top level").unwrap();
    std::fs::write(root.join("season 1/e01.txt"), b"first episode").unwrap();
    std::fs::write(root.join("season 1/extras/deleted.txt"), vec![9_u8; 4096]).unwrap();
    std::fs::write(root.join("season 2/e01.txt"), b"a whole other year").unwrap();
}

#[tokio::test]
async fn a_folder_dropped_onto_a_share_is_walked_and_every_file_lands() {
    // The live bug. Before E22-S2 this row failed with
    // `io: IsDirectory: path is a directory; open it with the DIRECTORY flag`
    // on xsync, and `Is a directory (os error 21)` on everything else: the
    // upload read its source with one `std::fs::read`, and there was no
    // planner on this route to walk it.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("Show");
    tree(&source);

    let share: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share());
    let done = run_one(
        &share,
        Transfer::new(
            "folder",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Series/Show",
            0,
        ),
    )
    .await;

    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    for (path, expected) in [
        ("Series/Show/readme.txt", b"top level".to_vec()),
        ("Series/Show/season 1/e01.txt", b"first episode".to_vec()),
        ("Series/Show/season 1/extras/deleted.txt", vec![9_u8; 4096]),
        (
            "Series/Show/season 2/e01.txt",
            b"a whole other year".to_vec(),
        ),
    ] {
        assert_eq!(
            share.read_range(path, 0, 1 << 20).await.expect(path),
            expected,
            "{path} did not land"
        );
    }
    // The empty directory too: nothing inside it would have created it.
    assert_eq!(
        share
            .stat("Series/Show/artwork")
            .await
            .expect("artwork")
            .kind,
        excalibur_engine::EntryKind::Dir
    );
    // And the planner's own count reached the row, so the bar was not
    // indeterminate for the whole transfer.
    assert!(done.bytes_total > 4096, "{}", done.bytes_total);
    assert!(done.verified, "a planned copy vouches for its bytes");
}

#[tokio::test]
async fn a_folder_copied_between_two_local_mounts_arrives_whole() {
    // local↔local, which the AC names and which is the pair where a bug hides:
    // both ends are `std::fs`, so a walk that never happened still produces a
    // plausible-looking result.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("Show");
    tree(&source);
    let destination = workspace.path().join("share");
    std::fs::create_dir(&destination).unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(
        LocalBackend::open(&destination, false)
            .await
            .expect("open the destination as a mount"),
    );
    let done = run_one(
        &share,
        Transfer::new(
            "folder",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Copied",
            0,
        ),
    )
    .await;

    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        std::fs::read(destination.join("Copied/season 1/extras/deleted.txt")).expect("deep file"),
        vec![9_u8; 4096]
    );
    assert!(destination.join("Copied/artwork").is_dir());
    // Nothing is left under a staging name. A stage that outlived its transfer
    // would show up in the user's own folder listing.
    assert!(
        !std::fs::read_dir(destination.join("Copied"))
            .unwrap()
            .any(|entry| {
                let name = entry.unwrap().file_name();
                name.to_string_lossy().starts_with(".excalibur-stage-")
            }),
        "a staging file was left behind"
    );
}

#[tokio::test]
async fn a_folder_pulled_off_a_share_is_walked_too() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let exported = workspace.path().join("export");
    let source = exported.join("Show");
    tree(&source);
    let destination = workspace.path().join("downloads");
    std::fs::create_dir(&destination).unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(
        LocalBackend::open(&exported, false)
            .await
            .expect("open the export as a mount"),
    );
    let done = run_one(
        &share,
        Transfer::new(
            "pull",
            Direction::Down,
            "Show",
            destination.join("Show").to_str().expect("utf-8"),
            0,
        ),
    )
    .await;

    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        std::fs::read(destination.join("Show/season 2/e01.txt")).expect("deep file"),
        b"a whole other year"
    );
    assert!(destination.join("Show/artwork").is_dir());
}

#[tokio::test]
async fn one_file_moves_without_dragging_its_siblings_along() {
    // A single-file transfer is rooted at its parent, and the parent has other
    // files in it. Narrowing that to one entry is `Only`'s whole job; without
    // it a drag of one file copies the folder it was in.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("outgoing");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("wanted.txt"), b"this one").unwrap();
    std::fs::write(source.join("not wanted.txt"), b"not this one").unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share());
    let done = run_one(
        &share,
        Transfer::new(
            "one",
            Direction::Up,
            source.join("wanted.txt").to_str().expect("utf-8"),
            "Transcodes/wanted.txt",
            0,
        ),
    )
    .await;

    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        share
            .read_range("Transcodes/wanted.txt", 0, 64)
            .await
            .expect("read back"),
        b"this one"
    );
    assert!(
        share.stat("Transcodes/not wanted.txt").await.is_err(),
        "the sibling was copied too"
    );
}

#[tokio::test]
async fn keep_both_publishes_one_file_under_the_name_the_row_promised() {
    // E22-S1 did this with a directory holding one hard link, because its
    // engine names what it publishes after the source's basename. The seam
    // says the destination name outright, so the scaffolding is gone.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("report.pdf");
    std::fs::write(&source, b"contents").unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share());
    let done = run_one(
        &share,
        Transfer::new(
            "renamed",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Transcodes/report 2.pdf",
            0,
        ),
    )
    .await;

    assert_eq!(done.status, Status::Done, "{:?}", done.error);
    assert_eq!(
        share
            .read_range("Transcodes/report 2.pdf", 0, 64)
            .await
            .expect("read back"),
        b"contents"
    );
    assert!(share.stat("Transcodes/report.pdf").await.is_err());
    // The hard-link staging directory E22-S1 needed is not created any more.
    assert!(!workspace.path().join(".excalibur-renamed").exists());
}

#[tokio::test]
async fn an_upload_to_a_read_only_share_is_refused_with_the_servers_reason() {
    // The seam rule: one derived value gates every write, and the sink is
    // where the endpoint route asks it.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("outgoing.txt");
    std::fs::write(&source, b"nope").unwrap();

    let share: Arc<dyn ShareBackend> =
        Arc::new(MockBackend::media_share().read_only("export is read-only"));
    let failed = run_one(
        &share,
        Transfer::new(
            "blocked",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Transcodes/blocked.txt",
            0,
        ),
    )
    .await;

    assert_eq!(failed.status, Status::Failed);
    // The row shows this verbatim, and it is what decides whether the design's
    // `Remount as RW` button appears on it.
    assert!(failed.needs_write_access(), "{:?}", failed.error);
}

#[tokio::test]
async fn a_file_already_on_this_machine_is_not_pulled_again() {
    // The planner's answer, and it is not "done": a copy that moved no bytes
    // because the file was already there is a different outcome from one that
    // moved them, and the row says so rather than claiming a transfer.
    let workspace = tempfile::tempdir().expect("temp dir");
    let exported = workspace.path().join("export");
    std::fs::create_dir(&exported).unwrap();
    std::fs::write(exported.join("stable.txt"), b"unchanged between two runs").unwrap();
    let destination = workspace.path().join("downloads");
    std::fs::create_dir(&destination).unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(
        LocalBackend::open(&exported, false)
            .await
            .expect("open the export as a mount"),
    );
    let pull = |id: &'static str| {
        Transfer::new(
            id,
            Direction::Down,
            "stable.txt",
            destination.join("stable.txt").to_str().expect("utf-8"),
            0,
        )
    };

    let first = run_one(&share, pull("first")).await;
    assert_eq!(first.status, Status::Done, "{:?}", first.error);
    assert!(first.verified, "the first run published the bytes");

    let second = run_one(&share, pull("second")).await;
    assert_eq!(second.status, Status::Done, "{:?}", second.error);
    assert_eq!(second.note.as_deref(), Some("unchanged"));
    assert!(
        !second.verified,
        "nothing was published, so nothing was verified"
    );
}

#[tokio::test]
async fn a_file_already_on_the_share_is_sent_again_because_its_mtime_cannot_be_restored() {
    // Not a bug being asserted into permanence — a limitation being kept
    // visible. The planner classifies an unchanged file by size and mtime, and
    // `ShareBackend` has no verb for setting either: SMB, NFSv3, SFTP and
    // xsync each have one, and the trait has never needed it. So a file copied
    // *onto* a share carries the server's write time, the next run sees a
    // different mtime, and the bytes go again.
    //
    // Turn this test red by giving the sink an mtime to apply, and delete it.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("stable.txt");
    std::fs::write(&source, b"unchanged between two runs").unwrap();

    let share: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share());
    let push = |id: &'static str| {
        Transfer::new(
            id,
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Transcodes/stable.txt",
            0,
        )
    };

    assert_eq!(run_one(&share, push("first")).await.status, Status::Done);
    let second = run_one(&share, push("second")).await;
    assert_eq!(second.status, Status::Done, "{:?}", second.error);
    assert_eq!(second.note, None, "the upload was skipped after all");
    assert!(second.verified, "it was sent again, and checked again");
}

#[tokio::test]
async fn nothing_appears_under_its_final_name_until_every_byte_is_there() {
    // The staging protocol's whole point, and the one thing that has to be
    // true of every backend: a transfer that fails part-way leaves the
    // destination name untouched.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = workspace.path().join("big.bin");
    // Past `LARGE_FILE_STAGE_MIN_BYTES`, so it takes the staged path rather
    // than being published in one call.
    std::fs::write(&source, vec![3_u8; 9 * 1024 * 1024]).unwrap();

    // Enough calls to get through the scan and the first ranges, and not
    // enough to finish.
    let share: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share().failing_after(12));
    let failed = run_one(
        &share,
        Transfer::new(
            "torn",
            Direction::Up,
            source.to_str().expect("utf-8"),
            "Transcodes/big.bin",
            0,
        ),
    )
    .await;

    assert_eq!(failed.status, Status::Failed, "{:?}", failed.error);
    assert!(
        share.stat("Transcodes/big.bin").await.is_err(),
        "a half-written file appeared under its final name"
    );
}
