//! The transfer queue: ordering, persistence, and what a restart does to it.

use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::{
    Direction, Endpoint, Status, Transfer, TransferFileProgress, TransferProgress,
    TransferSegmentProgress, DEFAULT_QUEUE_ID,
};

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
async fn named_queues_move_rows_reorder_them_and_survive_restart() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    let named = queue
        .create_queue("Nightly", "blue", 2)
        .await
        .expect("valid queue");
    let mut first = transfer("first", 100);
    first.queue_id = named.id.clone();
    let mut second = transfer("second", 200);
    second.queue_id = named.id.clone();
    queue.add(first).await;
    queue.add(second).await;

    queue
        .reorder_queue(&named.id, &["second".to_owned(), "first".to_owned()])
        .await;
    let rows = queue.all().await;
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert!(queue.try_claim_queue(&named.id).await);
    assert!(queue.try_claim_queue(&named.id).await);
    assert!(!queue.try_claim_queue(&named.id).await);
    queue.release_queue(&named.id).await;

    let restored = TransferQueue::open(&path);
    assert_eq!(restored.queues().await.len(), 2);
    assert_eq!(restored.all().await[0].id, "second");
    assert_eq!(restored.all().await[0].queue_id, named.id);
}

#[tokio::test]
async fn deleting_a_named_queue_returns_its_rows_to_default() {
    let queue = TransferQueue::in_memory();
    let named = queue
        .create_queue("Later", "green", 1)
        .await
        .expect("valid queue");
    let mut row = transfer("row", 1);
    row.queue_id = named.id.clone();
    queue.add(row).await;
    assert!(queue.delete_queue(&named.id).await.is_some());
    let row = queue.all().await.remove(0);
    assert_eq!(row.queue_id, DEFAULT_QUEUE_ID);
    assert!(queue.delete_queue(DEFAULT_QUEUE_ID).await.is_none());
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
async fn live_progress_is_in_queue_events_but_not_restored_as_durable_state() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    queue.add(transfer("live", 1)).await;
    queue
        .update("live", |row| {
            row.status = Status::Running;
            row.progress = Some(TransferProgress {
                files: vec![TransferFileProgress {
                    path: "live.bin".to_owned(),
                    completed: 10,
                    total: Some(100),
                    segments: vec![TransferSegmentProgress {
                        stream: 2,
                        offset: 50,
                        length: Some(50),
                        written: 10,
                        rate: 20,
                        stalled: false,
                    }],
                }],
                retries: 1,
                redirects: 0,
            });
        })
        .await;

    let live = queue.all().await.remove(0);
    assert_eq!(
        live.progress.as_ref().unwrap().files[0].segments[0].offset,
        50
    );
    let restored = TransferQueue::open(&path).all().await;
    assert!(restored[0].progress.is_none());
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
async fn a_queue_written_before_endpoints_still_loads_and_names_share_ends() {
    // XDL-E1-S2, second AC: no queue-file rewrite. This is the shape a build
    // before `from`/`to` wrote — no endpoint keys, no mount, no verified, no
    // file counts — and it must open as share rows, re-queued, with both ends
    // derived lazily rather than invented.
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");
    std::fs::write(
        &path,
        serde_json::json!([
            {
                "id": "old-down",
                "direction": "down",
                "name": "file.bin",
                "sourcePath": "remote/file.bin",
                "destPath": "/local/file.bin",
                "bytesDone": 40,
                "bytesTotal": 100,
                "rate": 1_000_000,
                "etaSeconds": 9,
                "status": "running",
                "error": null,
                "queuedAtMs": 100
            },
            {
                "id": "old-up",
                "direction": "up",
                "name": "notes.txt",
                "sourcePath": "/local/notes.txt",
                "destPath": "remote/notes.txt",
                "bytesDone": 0,
                "bytesTotal": 9,
                "rate": null,
                "etaSeconds": null,
                "status": "queued",
                "error": null,
                "queuedAtMs": 200
            }
        ])
        .to_string(),
    )
    .expect("write");

    let restored = TransferQueue::open(&path);
    let all = restored.all().await;
    assert_eq!(all.len(), 2);

    let down = &all[0];
    assert_eq!(down.status, Status::Queued);
    assert_eq!(down.bytes_done, 40);
    assert_eq!(down.rate, None);
    assert_eq!(
        down.from_endpoint(),
        Some(Endpoint::Share {
            mount: None,
            path: "remote/file.bin".to_owned()
        })
    );
    assert_eq!(
        down.to_endpoint(),
        Some(Endpoint::Local {
            path: "/local/file.bin".to_owned()
        })
    );

    let up = &all[1];
    assert_eq!(
        up.from_endpoint(),
        Some(Endpoint::Local {
            path: "/local/notes.txt".to_owned()
        })
    );
    assert_eq!(
        up.to_endpoint(),
        Some(Endpoint::Share {
            mount: None,
            path: "remote/notes.txt".to_owned()
        })
    );
}

#[tokio::test]
async fn an_endpoint_native_download_row_survives_a_restart_without_a_direction() {
    // The other half of XDL-E1-S2: a row from a URL to a local folder keeps
    // its endpoints and its absent direction through the file, so the next
    // launch drains it as a download rather than a share row.
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("transfers.json");
    let destination = directory.path().join("downloads");
    {
        let queue = TransferQueue::open(&path);
        queue
            .add(
                Transfer::http(
                    "http",
                    "https://example.test/f",
                    destination.to_str().unwrap(),
                )
                .expect("valid URL"),
            )
            .await;
    }
    let restored = TransferQueue::open(&path);
    let all = restored.all().await;
    assert_eq!(all.len(), 1);
    let row = &all[0];
    assert_eq!(row.status, Status::Queued);
    assert_eq!(row.direction, None);
    assert_eq!(row.mount, None);
    assert_eq!(
        row.from,
        Some(Endpoint::Http {
            url: "https://example.test/f".to_owned()
        })
    );
    assert_eq!(
        row.to,
        Some(Endpoint::Local {
            path: destination.to_str().unwrap().to_owned()
        })
    );
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
    transfer.direction = Some(Direction::Down);
    transfer.error = Some("permission denied: /srv".to_owned());
    assert!(!transfer.needs_write_access());
}

#[tokio::test]
async fn a_row_the_worker_passed_over_is_not_handed_back_immediately() {
    // A transfer whose share is not connected stays `Queued`, because that is a
    // wait rather than a failure — and a loop asking for "the next queued
    // transfer" would then be handed the same one for ever. The skip set is
    // what makes passing over a row terminate.
    let queue = TransferQueue::in_memory();
    queue.add(transfer("waiting", 100)).await;
    queue.add(transfer("ready", 200)).await;

    let mut passed = std::collections::HashSet::new();
    passed.insert("waiting".to_owned());
    assert_eq!(
        queue.next_queued_except(&passed).await.map(|t| t.id),
        Some("ready".to_owned())
    );
    passed.insert("ready".to_owned());
    assert!(queue.next_queued_except(&passed).await.is_none());
    // And nothing was changed to achieve it: both are still queued.
    assert!(queue.all().await.iter().all(|t| t.status == Status::Queued));
}

#[tokio::test]
async fn clearing_failed_takes_the_failures_and_nothing_else() {
    // The other verb the panel's menu offers. It is deliberately not
    // `clear_finished` with `Failed` folded in: one is asked for by name and
    // the other would sweep away a Retry button the user had not pressed yet.
    let queue = TransferQueue::in_memory();
    for (id, status) in [
        ("failed", Status::Failed),
        ("done", Status::Done),
        ("running", Status::Running),
        ("queued", Status::Queued),
    ] {
        queue.add(transfer(id, 100)).await;
        queue.update(id, |t| t.status = status).await;
    }

    assert_eq!(queue.clear_failed().await, ["failed"]);
    let left: Vec<String> = queue.all().await.iter().map(|t| t.id.clone()).collect();
    assert_eq!(left, ["done", "queued", "running"]);
}

#[tokio::test]
async fn retrying_every_failure_re_queues_it_without_the_reason_it_stopped() {
    let queue = TransferQueue::in_memory();
    for (id, status) in [("failed", Status::Failed), ("done", Status::Done)] {
        queue.add(transfer(id, 100)).await;
        queue
            .update(id, |t| {
                t.status = status;
                t.error = Some("connection lost: not connected".to_owned());
                t.note = Some("waiting for its share".to_owned());
            })
            .await;
    }

    let requeued = queue.requeue_failed().await;
    assert_eq!(requeued.len(), 1);
    let all = queue.all().await;
    let back = all.iter().find(|t| t.id == "failed").expect("the row");
    assert_eq!(back.status, Status::Queued);
    // Both described the attempt that is over. A row about to be tried again
    // that still reads `failed` under its name is the app disagreeing with
    // itself.
    assert_eq!(back.error, None);
    assert_eq!(back.note, None);
    // A finished transfer is not swept back into the queue by a bulk retry.
    assert_eq!(
        all.iter().find(|t| t.id == "done").map(|t| t.status),
        Some(Status::Done)
    );
}

#[tokio::test]
async fn saying_why_a_row_is_waiting_writes_the_file_once_and_then_not_at_all() {
    // The boot cost `2b16451` removed, and the reason this takes a slice: a
    // restored queue against a share nothing has mounted is *every* row
    // waiting, and one write per row is quadratic in a file that starts at over
    // a megabyte. The second pass over a queue that is still waiting must cost
    // nothing at all — no write, and no event for a window to re-render on.
    let home = tempfile::tempdir().expect("temp dir");
    let path = home.path().join("transfers.json");
    let queue = TransferQueue::open(&path);
    let ids: Vec<String> = (0..20).map(|n| format!("row-{n}")).collect();
    for (index, id) in ids.iter().enumerate() {
        queue
            .add(transfer(id, i64::try_from(index).expect("small")))
            .await;
    }
    let mut watching = queue.events();

    assert_eq!(queue.note_all(&ids, "waiting for its share").await, 20);
    let after = saved(&path);
    assert!(after.contains("waiting for its share"), "the note was lost");

    // Everything was told once.
    let mut told = 0;
    while watching.try_recv().is_ok() {
        told += 1;
    }
    assert_eq!(told, 20);

    assert_eq!(queue.note_all(&ids, "waiting for its share").await, 0);
    assert_eq!(saved(&path), after, "a second pass rewrote the queue");
    assert!(
        watching.try_recv().is_err(),
        "a second pass told the window"
    );
}

#[tokio::test]
async fn only_a_queued_row_is_told_it_is_waiting() {
    // The note belongs to a row that has not started. Writing it onto one that
    // is running, or onto a finished row whose note is the planner's own
    // "unchanged", would overwrite an answer with a question.
    let queue = TransferQueue::in_memory();
    queue.add(transfer("running", 100)).await;
    queue
        .update("running", |t| t.status = Status::Running)
        .await;
    queue.add(transfer("done", 200)).await;
    queue
        .update("done", |t| {
            t.status = Status::Done;
            t.note = Some("unchanged".to_owned());
        })
        .await;

    let ids = ["running".to_owned(), "done".to_owned()];
    assert_eq!(queue.note_all(&ids, "waiting for its share").await, 0);
    let all = queue.all().await;
    assert_eq!(all[0].note, None);
    assert_eq!(all[1].note.as_deref(), Some("unchanged"));
}
