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
