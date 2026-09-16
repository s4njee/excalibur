//! Native transfer queue state and controls.
//!
//! The engine owns persistence and worker semantics. This model owns only a
//! metadata snapshot and translates panel events into queue/worker operations.
//! A Worker is optional: the shell can attach one when it has a native
//! `BackendSource`, while queue browsing and safe non-running controls work
//! without inventing a second protocol policy.

use std::sync::Arc;

use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::Worker;
use excalibur_engine::{EngineError, KnownHosts, NamedQueue, Result, Transfer, TransferStatus};
use gpui_kit::Context;

use crate::services::{ServiceTask, Services};
use crate::transfers::TransfersEvent;

/// The queue's current metadata, suitable for handing to the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferSnapshot {
    pub transfers: Vec<Transfer>,
    pub queues: Vec<NamedQueue>,
}

/// A generation-tagged queue snapshot request.
pub struct SnapshotRequest {
    token: u64,
    task: ServiceTask<TransferSnapshot>,
}

impl SnapshotRequest {
    #[must_use]
    pub const fn token(&self) -> u64 {
        self.token
    }

    #[must_use]
    pub fn into_task(self) -> ServiceTask<TransferSnapshot> {
        self.task
    }
}

/// Persistent transfer queue model for the GPUI shell.
pub struct TransferModel {
    services: Arc<Services>,
    queue: Arc<TransferQueue>,
    worker: Option<Arc<Worker>>,
    transfers: Arc<[Transfer]>,
    queues: Arc<[NamedQueue]>,
    next_token: u64,
    applied_token: u64,
}

impl TransferModel {
    /// Open and restore the persistent queue. Missing or malformed files are
    /// handled by the engine queue exactly as they are for the Tauri shell.
    #[must_use]
    pub fn open(path: impl Into<std::path::PathBuf>, services: Arc<Services>) -> Self {
        Self::from_queue(Arc::new(TransferQueue::open(path)), services)
    }

    #[must_use]
    pub fn from_queue(queue: Arc<TransferQueue>, services: Arc<Services>) -> Self {
        Self {
            services,
            queue,
            worker: None,
            transfers: Arc::from(Vec::<Transfer>::new()),
            queues: Arc::from(Vec::<NamedQueue>::new()),
            next_token: 0,
            applied_token: 0,
        }
    }

    /// Attach an externally constructed worker. Worker construction remains a
    /// shell concern because its `BackendSource` must carry native host-key and
    /// session routing policy.
    #[must_use]
    pub fn with_worker(mut self, worker: Arc<Worker>) -> Self {
        self.worker = Some(worker);
        self
    }

    /// Attach a worker using the same native sessions as `Services`.
    ///
    /// This is deliberately explicit: restoring a queue should not start
    /// network work before the shell has mounted its initial share and hooked
    /// up the transfer panel. Call [`Worker::drain`] after that setup when the
    /// app wants queued rows to run.
    #[must_use]
    pub fn with_native_worker(mut self, known_hosts: KnownHosts, concurrency: usize) -> Self {
        let worker =
            self.services
                .start_transfer_worker(Arc::clone(&self.queue), known_hosts, concurrency);
        self.worker = Some(worker);
        self
    }

    /// Start the attached worker on the service runtime.
    ///
    /// `Worker::drain` only claims queued rows. Paused rows therefore remain
    /// paused across startup; use the panel's `ResumeAll` event when the user
    /// explicitly asks to resume them.
    pub fn start_worker(&self) -> ServiceTask<()> {
        let Some(worker) = self.worker.as_ref().map(Arc::clone) else {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "transfer worker is not attached".to_owned(),
                ))
            });
        };
        self.services.start_worker(worker)
    }

    /// Alias for hosts that call the engine operation `drain`.
    #[must_use]
    pub fn drain(&self) -> ServiceTask<()> {
        self.start_worker()
    }

    #[must_use]
    pub fn queue(&self) -> &Arc<TransferQueue> {
        &self.queue
    }

    #[must_use]
    pub fn transfers(&self) -> &[Transfer] {
        &self.transfers
    }

    #[must_use]
    pub fn queues(&self) -> &[NamedQueue] {
        &self.queues
    }

    /// Read a fresh queue snapshot on the service runtime.
    pub fn refresh(&mut self) -> SnapshotRequest {
        self.next_token = self.next_token.wrapping_add(1).max(1);
        let token = self.next_token;
        let queue = Arc::clone(&self.queue);
        let task = self.services.submit(async move {
            Ok(TransferSnapshot {
                transfers: queue.all().await,
                queues: queue.queues().await,
            })
        });
        SnapshotRequest { token, task }
    }

    /// Install a snapshot only when it belongs to the latest refresh.
    pub fn apply_snapshot(&mut self, token: u64, snapshot: Result<TransferSnapshot>) -> bool {
        if token < self.next_token || token < self.applied_token {
            return false;
        }
        let Ok(snapshot) = snapshot else {
            return false;
        };
        self.applied_token = token;
        self.transfers = Arc::from(snapshot.transfers);
        self.queues = Arc::from(snapshot.queues);
        true
    }

    /// Start a coalescing watcher for transfer and named-queue changes.
    pub fn subscribe(&mut self, cx: &mut Context<Self>) {
        let queue = Arc::clone(&self.queue);
        cx.spawn(async move |model, cx| {
            let mut events = queue.events();
            let mut queue_events = queue.queue_events();
            loop {
                let received = tokio::select! {
                    event = events.recv() => event.is_ok(),
                    event = queue_events.recv() => event.is_ok(),
                };
                if !received {
                    break;
                }
                while events.try_recv().is_ok() {}
                while queue_events.try_recv().is_ok() {}
                let snapshot = TransferSnapshot {
                    transfers: queue.all().await,
                    queues: queue.queues().await,
                };
                if model
                    .update(cx, |model, cx| {
                        model.next_token = model.next_token.wrapping_add(1).max(1);
                        model.applied_token = model.next_token;
                        model.transfers = Arc::from(snapshot.transfers);
                        model.queues = Arc::from(snapshot.queues);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Translate a panel event into a real queue/worker operation.
    pub fn handle_event(&self, event: TransfersEvent) -> ServiceTask<()> {
        let queue = Arc::clone(&self.queue);
        let worker = self.worker.as_ref().map(Arc::clone);
        self.services.submit(async move {
            match event {
                TransfersEvent::PauseAll => {
                    let all = queue.all().await;
                    let running: Vec<String> = all
                        .iter()
                        .filter(|transfer| transfer.status == TransferStatus::Running)
                        .map(|transfer| transfer.id.clone())
                        .collect();
                    if let Some(worker) = worker {
                        for id in running {
                            worker.stop(&id, TransferStatus::Paused).await;
                        }
                        queue.pause_all().await;
                    } else if !running.is_empty() {
                        return Err(no_worker_error());
                    } else {
                        queue.pause_all().await;
                    }
                }
                TransfersEvent::ResumeAll => {
                    requeue_paused(&queue).await;
                    if let Some(worker) = worker {
                        worker.drain().await;
                    }
                }
                TransfersEvent::ClearFinished => {
                    queue.clear_finished().await;
                }
                TransfersEvent::Retry { id } => {
                    let Some(updated) = queue
                        .update(&id, |transfer| {
                            if transfer.status != TransferStatus::Running {
                                transfer.status = TransferStatus::Queued;
                                transfer.error = None;
                            }
                        })
                        .await
                    else {
                        return Err(EngineError::NotFound(format!("transfer {id}")));
                    };
                    if updated.status == TransferStatus::Running {
                        return Err(EngineError::Unsupported(
                            "a running transfer cannot be retried".to_owned(),
                        ));
                    }
                    if let Some(worker) = worker {
                        worker.drain().await;
                    }
                }
                TransfersEvent::Pause { id } => {
                    if let Some(worker) = worker {
                        worker.stop(&id, TransferStatus::Paused).await;
                    } else {
                        update_nonrunning(&queue, &id, TransferStatus::Paused).await?;
                    }
                }
                TransfersEvent::Cancel { id } => {
                    if let Some(worker) = worker {
                        worker.stop(&id, TransferStatus::Cancelled).await;
                    } else {
                        update_nonrunning(&queue, &id, TransferStatus::Cancelled).await?;
                    }
                }
            }
            Ok(())
        })
    }
}

async fn requeue_paused(queue: &TransferQueue) {
    let ids: Vec<String> = queue
        .all()
        .await
        .into_iter()
        .filter(|transfer| transfer.status == TransferStatus::Paused)
        .map(|transfer| transfer.id)
        .collect();
    for id in ids {
        let _ = queue
            .update(&id, |transfer| {
                if transfer.status == TransferStatus::Paused {
                    transfer.status = TransferStatus::Queued;
                    transfer.error = None;
                }
            })
            .await;
    }
}

async fn update_nonrunning(queue: &TransferQueue, id: &str, status: TransferStatus) -> Result<()> {
    let Some(updated) = queue
        .update(id, |transfer| {
            if transfer.status != TransferStatus::Running {
                transfer.status = status;
                transfer.rate = None;
                transfer.eta_seconds = None;
            }
        })
        .await
    else {
        return Err(EngineError::NotFound(format!("transfer {id}")));
    };
    if updated.status == TransferStatus::Running {
        return Err(no_worker_error());
    }
    Ok(())
}

fn no_worker_error() -> EngineError {
    EngineError::Unsupported(
        "transfer worker is not attached; running work was left unchanged".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Services, TransferModel, TransfersEvent};
    use excalibur_engine::transfer::queue::TransferQueue;
    use excalibur_engine::{Direction, Transfer, TransferStatus};
    use std::sync::Arc;

    fn row(id: &str) -> Transfer {
        Transfer::new(id, Direction::Down, "remote.txt", "/tmp/remote.txt", 4)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn snapshot_reads_transfers_and_named_queues() {
        let queue = Arc::new(TransferQueue::in_memory());
        queue.add(row("one")).await;
        let model = TransferModel::from_queue(
            Arc::clone(&queue),
            Arc::new(Services::mock().expect("runtime")),
        );
        let mut model = model;
        let request = model.refresh();
        let token = request.token();
        let snapshot = request.into_task().await.expect("snapshot");
        assert!(model.apply_snapshot(token, Ok(snapshot)));
        assert_eq!(model.transfers().len(), 1);
        assert_eq!(model.queues().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn panel_events_map_to_queue_operations() {
        let queue = Arc::new(TransferQueue::in_memory());
        queue.add(row("one")).await;
        let model = TransferModel::from_queue(
            Arc::clone(&queue),
            Arc::new(Services::mock().expect("runtime")),
        );
        model
            .handle_event(TransfersEvent::Pause {
                id: "one".to_owned(),
            })
            .await
            .expect("pause");
        assert_eq!(queue.all().await[0].status, TransferStatus::Paused);
        model
            .handle_event(TransfersEvent::Cancel {
                id: "one".to_owned(),
            })
            .await
            .expect("cancel");
        assert_eq!(queue.all().await[0].status, TransferStatus::Cancelled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn running_control_without_worker_is_not_fake_success() {
        let queue = Arc::new(TransferQueue::in_memory());
        let mut transfer = row("one");
        transfer.status = TransferStatus::Running;
        queue.add(transfer).await;
        let model = TransferModel::from_queue(queue, Arc::new(Services::mock().expect("runtime")));
        assert!(model
            .handle_event(TransfersEvent::Cancel {
                id: "one".to_owned()
            })
            .await
            .is_err());
    }
}
