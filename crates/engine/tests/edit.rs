//! Edit in place: the round trip, and what happens when the remote moves.

use std::sync::Arc;

use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::ShareBackend;
use excalibur_engine::edit::{EditStore, SaveOutcome};
use excalibur_engine::WriteMode;

fn share() -> Arc<dyn ShareBackend> {
    Arc::new(MockBackend::media_share())
}

async fn seed(backend: &Arc<dyn ShareBackend>, path: &str, body: &[u8]) {
    backend
        .write_range(path, WriteMode::Create, body)
        .await
        .expect("seed");
}

#[tokio::test]
async fn a_round_trip_writes_the_edit_back() {
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    assert_eq!(
        std::fs::read(&session.local_path).expect("read"),
        b"original"
    );

    std::fs::write(&session.local_path, b"edited by me").expect("edit");
    let outcome = store
        .save(&backend, &session.id, false)
        .await
        .expect("save");
    assert_eq!(outcome, SaveOutcome::Saved { saves: 1 });

    assert_eq!(
        backend
            .read_range("doc.txt", 0, 1 << 20)
            .await
            .expect("read back"),
        b"edited by me"
    );
}

#[tokio::test]
async fn a_second_save_is_not_a_conflict_with_the_first() {
    // The expectation has to advance on every write-back. Comparing against
    // the *original* would make every save after the first look like somebody
    // else had changed the file — which is this editor, and is not a conflict.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    std::fs::write(&session.local_path, b"first edit").expect("edit");
    assert_eq!(
        store
            .save(&backend, &session.id, false)
            .await
            .expect("save"),
        SaveOutcome::Saved { saves: 1 }
    );

    std::fs::write(&session.local_path, b"second edit").expect("edit");
    assert_eq!(
        store
            .save(&backend, &session.id, false)
            .await
            .expect("save"),
        SaveOutcome::Saved { saves: 2 }
    );
    assert_eq!(
        backend
            .read_range("doc.txt", 0, 1 << 20)
            .await
            .expect("read"),
        b"second edit"
    );
}

#[tokio::test]
async fn a_remote_that_moved_underneath_is_refused_rather_than_overwritten() {
    // The property the whole story exists for: never a silent overwrite.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");

    // Somebody else edits it while the editor is open.
    seed(&backend, "doc.txt", b"somebody else was here").await;

    std::fs::write(&session.local_path, b"my edit").expect("edit");
    let outcome = store
        .save(&backend, &session.id, false)
        .await
        .expect("save resolves");
    assert!(
        matches!(outcome, SaveOutcome::Conflict { .. }),
        "expected a conflict, got {outcome:?}"
    );
    // Nothing was written.
    assert_eq!(
        backend
            .read_range("doc.txt", 0, 1 << 20)
            .await
            .expect("read"),
        b"somebody else was here"
    );
}

#[tokio::test]
async fn forcing_a_save_replaces_what_is_there() {
    // "Replace" from the conflict prompt.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    seed(&backend, "doc.txt", b"somebody else").await;
    std::fs::write(&session.local_path, b"mine wins").expect("edit");

    assert_eq!(
        store
            .save(&backend, &session.id, true)
            .await
            .expect("forced save"),
        SaveOutcome::Saved { saves: 1 }
    );
    assert_eq!(
        backend
            .read_range("doc.txt", 0, 1 << 20)
            .await
            .expect("read"),
        b"mine wins"
    );
}

#[tokio::test]
async fn an_untouched_working_copy_is_not_written_back() {
    // Editors write files they have not changed — a save with no edits, a
    // plugin touching mtime. Uploading for each would put a row in the queue
    // for nothing.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    assert_eq!(
        store
            .save(&backend, &session.id, false)
            .await
            .expect("save"),
        SaveOutcome::Unchanged
    );
}

#[tokio::test]
async fn a_read_only_mount_refuses_the_checkout_rather_than_the_save() {
    // Downloading a file, letting the user edit it, and only then saying it
    // cannot be written back would waste their work.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend: Arc<dyn ShareBackend> =
        Arc::new(MockBackend::media_share().read_only("export is read-only"));

    let refused = store.checkout(&backend, "Arrival.2016.srt").await;
    assert!(
        matches!(refused, Err(excalibur_engine::EngineError::ReadOnly(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn opening_the_same_file_twice_reuses_one_working_copy() {
    // Otherwise the first edit is orphaned in a directory nobody looks in.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let first = store.checkout(&backend, "doc.txt").await.expect("checkout");
    let second = store.checkout(&backend, "doc.txt").await.expect("checkout");
    assert_eq!(first.id, second.id);
    assert_eq!(first.local_path, second.local_path);
    assert_eq!(store.all().await.len(), 1);
}

#[tokio::test]
async fn closing_a_session_removes_its_working_copy() {
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    assert!(std::path::Path::new(&session.local_path).exists());
    store.close(&session.id).await;
    assert!(!std::path::Path::new(&session.local_path).exists());
    assert!(store.all().await.is_empty());
}

#[tokio::test]
async fn the_sweep_never_takes_a_file_with_unsaved_changes() {
    // Whatever the retention says, an edit nobody has written back is the last
    // thing to delete on a timer.
    let cache = tempfile::tempdir().expect("temp dir");
    let store = EditStore::new(cache.path());
    let backend = share();
    seed(&backend, "doc.txt", b"original").await;

    let session = store.checkout(&backend, "doc.txt").await.expect("checkout");
    std::fs::write(&session.local_path, b"unsaved work").expect("edit");

    assert!(store.sweep().await.is_empty());
    assert!(std::path::Path::new(&session.local_path).exists());
}
