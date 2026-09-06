//! The transfer queue: ordering, persistence, and what a restart does to it.

use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::{Direction, Status, Transfer};

fn transfer(id: &str, at: i64) -> Transfer {
    let mut transfer = Transfer::new(
        id,
        Direction::Down,
        format!("remote/{id}"),
        format!("/local/{id}"),
        100,
    );
    transfer.queued_at_ms = at;
    transfer
}

#[tokio::test]
async fn transfers_are_served_oldest_first() {
    let queue = TransferQueue::in_memory();
    queue.add(transfer("third", 300)).await;
    queue.add(transfer("first", 100)).await;
    queue.add(transfer("second", 200)).await;

    let next = queue.next_queued().await.expect("something queued");
    assert_eq!(next.id, "first");
    assert_eq!(
        queue
            .all()
            .await
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
}

#[tokio::test]
async fn a_paused_transfer_is_never_picked_up() {
    // The whole difference between Paused and Queued: a paused transfer stays
    // paused however idle the pool is.
    let queue = TransferQueue::in_memory();
    queue.add(transfer("paused", 100)).await;
    queue
        .update("paused", |transfer| transfer.status = Status::Paused)
        .await;
    assert!(queue.next_queued().await.is_none());
}

#[tokio::test]
async fn an_update_cannot_overwrite_a_status_set_while_it_was_in_flight() {
    // A worker reporting progress must not resurrect a transfer the user
    // cancelled while the chunk was moving. The closure sees the current
    // state, under the lock.
    let queue = TransferQueue::in_memory();
    queue.add(transfer("job", 100)).await;
    queue
        .update("job", |transfer| transfer.status = Status::Cancelled)
        .await;

    queue
        .update("job", |transfer| {
            if transfer.status == Status::Cancelled {
                return;
            }
            transfer.bytes_done = 50;
            transfer.status = Status::Running;
        })
        .await;

    let all = queue.all().await;
    assert_eq!(all[0].status, Status::Cancelled);
    assert_eq!(all[0].bytes_done, 0);
}

#[tokio::test]
async fn updating_something_that_is_gone_is_not_an_error() {
    let queue = TransferQueue::in_memory();
    assert!(queue.update("absent", |_| ()).await.is_none());
}

#[tokio::test]
async fn clearing_finished_leaves_the_active_ones() {
    let queue = TransferQueue::in_memory();
    for (id, status) in [
        ("done", Status::Done),
        ("cancelled", Status::Cancelled),
        ("running", Status::Running),
        ("failed", Status::Failed),
    ] {
        queue.add(transfer(id, 100)).await;
        queue.update(id, |t| t.status = status).await;
    }

    let removed = queue.clear_finished().await;
    assert_eq!(removed.len(), 2);
    let left: Vec<String> = queue.all().await.iter().map(|t| t.id.clone()).collect();
    // A failed transfer is kept: it has a Retry button on it, and clearing it
    // would throw away the thing the user is about to act on.
    assert!(left.contains(&"failed".to_owned()), "{left:?}");
    assert!(left.contains(&"running".to_owned()), "{left:?}");
}

#[tokio::test]
async fn pause_all_takes_queued_and_running_but_not_finished() {
    let queue = TransferQueue::in_memory();
    for (id, status) in [
        ("queued", Status::Queued),
        ("running", Status::Running),
        ("done", Status::Done),
    ] {
        queue.add(transfer(id, 100)).await;
        queue.update(id, |t| t.status = status).await;
    }

    queue.pause_all().await;
    for transfer in queue.all().await {
        let expected = if transfer.id == "done" {
            Status::Done
        } else {
            Status::Paused
        };
        assert_eq!(transfer.status, expected, "{}", transfer.id);
    }
}

#[tokio::test]
async fn the_queue_survives_a_restart_with_unfinished_work_requeued() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");

    {
        let queue = TransferQueue::open(&path);
        queue.add(transfer("mid-flight", 100)).await;
        queue
            .update("mid-flight", |transfer| {
                transfer.status = Status::Running;
                transfer.bytes_done = 40;
                transfer.rate = Some(1_000_000);
                transfer.eta_seconds = Some(9);
            })
            .await;
        queue.add(transfer("finished", 200)).await;
        queue.update("finished", |t| t.status = Status::Done).await;
    }

    let restored = TransferQueue::open(&path);
    let all = restored.all().await;
    assert_eq!(all.len(), 2);

    let running = &all[0];
    // The process that was moving those bytes is gone, so it is queued again.
    assert_eq!(running.status, Status::Queued);
    // But its progress is kept: a download resumes from its `.part` file, and
    // starting from zero would re-transfer what is already on disk.
    assert_eq!(running.bytes_done, 40);
    // The rate belonged to a process that no longer exists.
    assert_eq!(running.rate, None);
    assert_eq!(running.eta_seconds, None);

    // A finished transfer is restored as finished, not re-run.
    assert_eq!(all[1].status, Status::Done);
}

#[tokio::test]
async fn an_unreadable_queue_file_is_an_empty_queue() {
    // Losing the record of a transfer is survivable. Refusing to start the app
    // because of it is not.
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");
    std::fs::write(&path, b"{ not json at all").expect("write");
    assert!(TransferQueue::open(&path).all().await.is_empty());
}

/// What is on disk right now, or `""` when nothing has been written.
fn saved(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tokio::test]
async fn a_progress_tick_is_broadcast_but_not_written_down() {
    // The measured cost of writing every update: a real `transfers.json` on
    // this machine was 1.2 MB over 2253 rows, and `persist` rewrites all of it,
    // so every tick of every running transfer rewrote 1.2 MB. What a restart
    // needs is the row, and `restore` throws the rate away and re-queues
    // anything that was moving — so progress is exactly the part it does not
    // need.
    let home = tempfile::tempdir().expect("temp dir");
    let path = home.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    queue.add(transfer("job", 100)).await;
    let mut watching = queue.events();
    let after_add = saved(&path);

    queue
        .update("job", |transfer| {
            transfer.bytes_done = 64;
            transfer.files_done = 1;
            transfer.rate = Some(1_000_000);
            transfer.eta_seconds = Some(3);
        })
        .await;

    assert_eq!(saved(&path), after_add, "a progress tick rewrote the queue");
    let seen = watching.try_recv().expect("the window is still told");
    assert_eq!(seen.transfer.bytes_done, 64);
}

#[tokio::test]
async fn a_status_a_size_and_an_error_are_all_written_down() {
    // The other half of the rule, and the one that makes it safe: everything a
    // restart has to be able to reconstruct still reaches the file on the
    // update that changes it, with no timer in between.
    let home = tempfile::tempdir().expect("temp dir");
    let path = home.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    queue.add(transfer("job", 100)).await;

    queue
        .update("job", |transfer| transfer.status = Status::Running)
        .await;
    assert!(saved(&path).contains("running"), "a status change was lost");

    // `bytes_total` becoming known is the planner's answer, not progress: a
    // row restored without it would come back as an indeterminate bar.
    queue
        .update("job", |transfer| {
            transfer.bytes_total = 999_111;
            transfer.files_total = 42;
        })
        .await;
    assert!(saved(&path).contains("999111"), "a new total was lost");
    assert!(
        saved(&path).contains(r#""filesTotal": 42"#),
        "a count was lost"
    );

    queue
        .update("job", |transfer| {
            transfer.status = Status::Failed;
            transfer.error = Some("the share went away".to_owned());
        })
        .await;
    assert!(
        saved(&path).contains("the share went away"),
        "an error was lost"
    );
}

#[tokio::test]
async fn a_removed_transfer_is_gone_from_the_file_too() {
    // A promised drag's row names a staging directory that stops existing when
    // the drag ends. Leaving it in the file is not a harmless record: `restore`
    // re-queues it on the next launch and copies a tree into a folder that was
    // deleted.
    let home = tempfile::tempdir().expect("temp dir");
    let path = home.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    queue.add(transfer("promised", 100)).await;
    assert!(saved(&path).contains("promised"));

    assert!(queue.remove("promised").await.is_some());
    assert!(
        !saved(&path).contains("promised"),
        "the row survived in the file"
    );
    assert!(
        queue.remove("promised").await.is_none(),
        "removing twice is not an event"
    );
}

#[tokio::test]
async fn a_change_reaches_subscribers() {
    let queue = TransferQueue::in_memory();
    let mut events = queue.events();
    queue.add(transfer("job", 100)).await;
    let event = events.recv().await.expect("an event");
    assert_eq!(event.transfer.id, "job");
}

#[test]
fn a_transfer_is_named_for_where_it_lands() {
    // "Keep both" renames the destination, and the row should show the name
    // the file actually got.
    let transfer = Transfer::new("id", Direction::Down, "remote/a.txt", "/local/a 2.txt", 10);
    assert_eq!(transfer.name, "a 2.txt");
}

#[test]
fn an_unknown_size_has_no_fraction_rather_than_zero() {
    // A bar sitting at zero for a whole transfer looks broken; an
    // indeterminate one is honest about not knowing.
    let transfer = Transfer::new("id", Direction::Up, "a", "b", 0);
    assert_eq!(transfer.fraction(), None);
}

#[test]
fn remount_is_offered_only_where_it_would_help() {
    let mut transfer = Transfer::new("id", Direction::Up, "a", "b", 10);
    transfer.status = Status::Failed;
    transfer.error = Some("read-only mount: export is ro".to_owned());
    assert!(transfer.needs_write_access());

    // Reconnecting read-write changes nothing about a timeout.
    transfer.error = Some("connection lost: timed out".to_owned());
    assert!(!transfer.needs_write_access());

    // Nor about a download.
    transfer.direction = Direction::Down;
    transfer.error = Some("permission denied: /srv".to_owned());
    assert!(!transfer.needs_write_access());
}
