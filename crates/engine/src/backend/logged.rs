//! One backend, wrapped so every verb it serves reaches the message log
//! (`twopane.md` XTP-E5-S1).
//!
//! At the seam, not at the call sites. The design's log is "where protocol
//! truth lives", and a log assembled by remembering to write a line next to
//! each call is a log that is missing whichever call somebody added last. Every
//! verb crosses this trait, so wrapping it is the one place that cannot be
//! forgotten — the same argument `writeGuarded` makes on the frontend for
//! refusing writes.
//!
//! **This is engine-verb resolution, not frames.** `xsync-client` exposes no
//! tap (XTP-E5-S0, `integration.md` §8), and SMB, NFS and SFTP have no frames
//! to expose in the first place. So a line here says `list_dir /media` and how
//! long it took, not the wire message that carried it. The pane says as much.
//!
//! Reads that move bytes — `read_range`, `write_range`, `open_read` — are
//! deliberately *not* logged per call: a transfer makes thousands of them, and
//! a log they fill is one nobody can read. The verbs a person performs are the
//! ones worth a line.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;

use super::{
    Attrs, Capacity, Caps, CasOutcome, Health, Listing, MountInfo, ReadHandle, ShareBackend,
    WriteMode,
};
use crate::error::Result;
use crate::log::{LogCategory, LogLine, MessageLog};

/// Wraps a backend, recording the verbs that cross it.
pub struct LoggedBackend {
    inner: Arc<dyn ShareBackend>,
    log: Arc<MessageLog>,
    mount: String,
}

impl LoggedBackend {
    /// Wrap `inner`, attributing its lines to the mount `id`.
    #[must_use]
    pub fn new(inner: Arc<dyn ShareBackend>, log: Arc<MessageLog>, id: impl Into<String>) -> Self {
        Self {
            inner,
            log,
            mount: id.into(),
        }
    }

    fn command(&self, text: impl Into<String>) {
        self.log
            .record(LogLine::on(&self.mount, LogCategory::Command, text));
    }

    /// The answer, or the failure, in the words the backend used.
    ///
    /// One place, so the timing and the error text cannot come to be reported
    /// differently by different verbs. A failure is the server's own message
    /// verbatim — paraphrasing it loses the detail that says whether it is the
    /// export or the credentials.
    fn answer<T>(&self, started: Instant, category: LogCategory, outcome: &Result<T>, said: &str) {
        let millis = started.elapsed().as_millis();
        match outcome {
            Ok(_) => self.log.record(LogLine::on(
                &self.mount,
                category,
                format!("{said} in {millis} ms"),
            )),
            Err(error) => self.log.record(LogLine::on(
                &self.mount,
                LogCategory::Error,
                format!("{error}"),
            )),
        }
    }
}

#[async_trait]
impl ShareBackend for LoggedBackend {
    fn mount_info(&self) -> MountInfo {
        self.inner.mount_info()
    }

    fn caps(&self) -> Caps {
        self.inner.caps()
    }

    async fn capacity(&self) -> Result<Capacity> {
        self.inner.capacity().await
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        self.command(format!("list_dir {}", display(path)));
        let started = Instant::now();
        let outcome = self.inner.list_dir(path).await;
        let entries = outcome.as_ref().map(|listing| listing.entries.len());
        self.answer(
            started,
            LogCategory::Listing,
            &outcome,
            &match entries {
                Ok(count) => format!("{count} entries"),
                Err(_) => String::new(),
            },
        );
        outcome
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let outcome = self.inner.stat(path).await;
        // Only the failure: a stat is what half the other verbs are made of,
        // and logging every one of them buries the verb that asked for it.
        if let Err(error) = &outcome {
            self.log.record(LogLine::on(
                &self.mount,
                LogCategory::Error,
                format!("stat {}: {error}", display(path)),
            ));
        }
        outcome
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.inner.read_range(path, offset, length).await
    }

    async fn open_read(&self, path: &str) -> Result<Option<Arc<dyn ReadHandle>>> {
        self.inner.open_read(path).await
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        self.inner.write_range(path, mode, data).await
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        self.command(format!("mkdir {}", display(path)));
        let started = Instant::now();
        let outcome = self.inner.mkdir(path).await;
        self.answer(started, LogCategory::Response, &outcome, "created");
        outcome
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.command(format!(
            "rename {} → {}{}",
            display(from),
            display(to),
            if replace { " (replacing)" } else { "" }
        ));
        let started = Instant::now();
        let outcome = self.inner.rename(from, to, replace).await;
        self.answer(started, LogCategory::Response, &outcome, "renamed");
        outcome
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.command(format!(
            "remove {}{}",
            display(path),
            if recursive { " (recursive)" } else { "" }
        ));
        let started = Instant::now();
        let outcome = self.inner.remove(path, recursive).await;
        self.answer(started, LogCategory::Response, &outcome, "removed");
        outcome
    }

    async fn write_all_if_unchanged(
        &self,
        path: &str,
        expected: Option<&Attrs>,
        data: &[u8],
    ) -> Result<CasOutcome> {
        self.command(format!("write {} ({} bytes)", display(path), data.len()));
        let started = Instant::now();
        let outcome = self
            .inner
            .write_all_if_unchanged(path, expected, data)
            .await;
        // A refusal is not a failure — the destination having moved is the
        // answer the caller asked for — so it is a `Status`, not an `Error`.
        match &outcome {
            Ok(CasOutcome::Changed(_)) => self.log.record(LogLine::on(
                &self.mount,
                LogCategory::Status,
                format!("{} changed underneath; not written", display(path)),
            )),
            _ => self.answer(started, LogCategory::Response, &outcome, "written"),
        }
        outcome
    }

    async fn ping(&self) -> Result<Health> {
        // The health loop runs on a timer for the life of the mount. Logging
        // it would fill the ring with the app talking to itself.
        self.inner.ping().await
    }
}

/// The share root reads as `/`, which is what the user sees in the path bar.
fn display(path: &str) -> &str {
    if path.is_empty() {
        "/"
    } else {
        path
    }
}
