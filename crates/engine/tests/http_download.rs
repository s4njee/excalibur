//! XDL-E1: a real HTTP origin through the existing queue and endpoint planner.

#[path = "support/http_origin.rs"]
mod http_origin;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use excalibur_engine::backend::ShareBackend;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::{Endpoint, Status, Transfer};
use excalibur_engine::Result;
use http_origin::Origin;

struct NoShares;

#[async_trait]
impl BackendSource for NoShares {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        panic!("an HTTP download must not resolve a share backend");
    }

    async fn is_connected(&self, _: Option<&str>) -> bool {
        false
    }
}

fn patterned(size: usize) -> Vec<u8> {
    (0..size).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

fn worker(queue: &Arc<TransferQueue>) -> Arc<Worker> {
    Arc::new(Worker::new(Arc::clone(queue), Arc::new(NoShares), 2))
}

async fn drain(worker: &Arc<Worker>) {
    tokio::time::timeout(Duration::from_secs(40), worker.drain())
        .await
        .expect("the queue must settle");
}

#[tokio::test(flavor = "multi_thread")]
async fn http_download_uses_the_queue_without_a_connected_share_and_sanitises_the_name() {
    let bytes = patterned(128 * 1024);
    let origin = Origin::start(
        bytes.clone(),
        Some("attachment; filename=\"../../../.ssh/authorized_keys\""),
        true,
        Duration::ZERO,
    );
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("downloads");
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::http("http", &origin.url, destination.to_str().unwrap()).unwrap())
        .await;
    drain(&worker(&queue)).await;
    let rows = queue.all().await;
    let row = &rows[0];
    assert_eq!(row.status, Status::Done, "{:?}", row.error);
    assert_eq!(row.direction, None);
    assert_eq!(row.mount, None);
    assert_eq!(row.bytes_done, bytes.len() as u64);
    assert_eq!(row.bytes_total, bytes.len() as u64);
    assert!(row.verified);
    let files: Vec<_> = std::fs::read_dir(&destination)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(std::fs::read(&files[0]).unwrap(), bytes);
    assert_eq!(files[0].file_name().unwrap().to_str().unwrap(), row.name);
    assert_eq!(files[0].to_str().unwrap(), row.dest_path);
    assert!(!directory.path().join(".ssh").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_origin_without_range_support_still_downloads_through_the_same_route() {
    let bytes = patterned(256 * 1024);
    let origin = Origin::start(bytes.clone(), None, false, Duration::ZERO);
    let directory = tempfile::tempdir().unwrap();
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::http("stream", &origin.url, directory.path().to_str().unwrap()).unwrap())
        .await;
    drain(&worker(&queue)).await;
    let row = queue.all().await.remove(0);
    assert_eq!(row.status, Status::Done, "{:?}", row.error);
    assert_eq!(std::fs::read(&row.dest_path).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_http_failure_is_a_failed_row_and_does_not_publish_a_file() {
    let origin = Origin::start(vec![1], None, true, Duration::ZERO);
    let directory = tempfile::tempdir().unwrap();
    let queue = Arc::new(TransferQueue::in_memory());
    let url = origin.url.replace("object.bin", "missing");
    queue
        .add(Transfer::http("missing", url, directory.path().to_str().unwrap()).unwrap())
        .await;
    drain(&worker(&queue)).await;
    let row = queue.all().await.remove(0);
    assert_eq!(row.status, Status::Failed);
    assert!(!row.verified);
    assert!(!directory.path().join("missing").exists());
}

#[tokio::test]
async fn an_invalid_native_endpoint_pair_fails_instead_of_waiting_for_an_unrelated_share() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfer = Transfer::http(
        "invalid",
        "http://127.0.0.1:1/file",
        directory.path().to_str().unwrap(),
    )
    .unwrap();
    transfer.to = Some(Endpoint::Share {
        mount: None,
        path: "file".to_owned(),
    });
    let queue = Arc::new(TransferQueue::in_memory());
    queue.add(transfer).await;
    drain(&worker(&queue)).await;
    assert_eq!(queue.all().await[0].status, Status::Failed);
}

#[tokio::test]
async fn execution_revalidates_a_url_from_a_restored_row() {
    let directory = tempfile::tempdir().unwrap();
    let mut transfer = Transfer::http(
        "invalid",
        "http://127.0.0.1:1/file",
        directory.path().to_str().unwrap(),
    )
    .unwrap();
    transfer.from = Some(Endpoint::Http {
        url: "file:///etc/passwd".to_owned(),
    });
    let queue = Arc::new(TransferQueue::in_memory());
    queue.add(transfer).await;
    drain(&worker(&queue)).await;
    assert_eq!(queue.all().await[0].status, Status::Failed);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_an_http_download_keeps_it_unpublished_and_resuming_finishes_the_same_row() {
    let bytes = patterned(20 * 1024 * 1024);
    let origin = Origin::start(bytes.clone(), None, true, Duration::from_millis(2));
    let directory = tempfile::tempdir().unwrap();
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::http("pause", &origin.url, directory.path().to_str().unwrap()).unwrap())
        .await;
    let worker = worker(&queue);
    let running = Arc::clone(&worker);
    let task = tokio::spawn(async move { running.drain().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while origin.sent.load(Ordering::SeqCst) < 64 * 1024 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    worker.stop("pause", Status::Paused).await;
    tokio::time::timeout(Duration::from_secs(35), task)
        .await
        .unwrap()
        .unwrap();
    let row = queue.all().await.remove(0);
    assert_eq!(row.status, Status::Paused);
    assert!(!directory.path().join("object.bin").exists());
    queue
        .update("pause", |row| row.status = Status::Queued)
        .await;
    drain(&worker).await;
    let row = queue.all().await.remove(0);
    assert_eq!(row.id, "pause");
    assert_eq!(row.status, Status::Done, "{:?}", row.error);
    assert_eq!(std::fs::read(&row.dest_path).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_http_download_never_changes_the_row_to_done() {
    let origin = Origin::start(
        patterned(20 * 1024 * 1024),
        None,
        true,
        Duration::from_millis(2),
    );
    let directory = tempfile::tempdir().unwrap();
    let queue = Arc::new(TransferQueue::in_memory());
    queue
        .add(Transfer::http("cancel", &origin.url, directory.path().to_str().unwrap()).unwrap())
        .await;
    let worker = worker(&queue);
    let running = Arc::clone(&worker);
    let task = tokio::spawn(async move { running.drain().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while origin.sent.load(Ordering::SeqCst) < 64 * 1024 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    worker.stop("cancel", Status::Cancelled).await;
    tokio::time::timeout(Duration::from_secs(35), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queue.all().await[0].status, Status::Cancelled);
    assert!(!directory.path().join("object.bin").exists());
}
