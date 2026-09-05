//! The transfer queue: what is moving, how fast, and what to do when it fails.
//!
//! Two decisions shape everything here.
//!
//! **The queue is state, not a side effect.** A transfer survives navigation,
//! a dropped mount and an app restart, so it is a record that a worker acts on
//! rather than a future somebody is awaiting. That is what makes pause,
//! cancel, retry and "restore as queued on launch" all the same operation on
//! the same field.
//!
//! **Progress is sampled, not streamed.** A 1 GiB copy in 1 MiB chunks
//! produces a thousand updates; a window that re-rendered a row for each of
//! them would spend more time drawing than copying. Every update goes through
//! [`Progress`], which decides whether the window is told.

pub mod conflict;
pub mod plan;
pub mod queue;
pub mod rate;
pub mod worker;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Which way the bytes are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// From this machine to the share.
    Up,
    /// From the share to this machine.
    Down,
}

/// Where a transfer is in its life.
///
/// `Paused` and `Queued` are deliberately different states rather than one
/// flag: a queued transfer starts when a worker frees up, and a paused one
/// does not, however idle the pool is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for a worker.
    Queued,
    /// Moving bytes now.
    Running,
    /// Stopped by the user; will not start on its own.
    Paused,
    /// Finished, every byte accounted for.
    Done,
    /// Stopped by an error. `error` says which.
    Failed,
    /// Abandoned by the user. Kept in the queue until cleared.
    Cancelled,
}

impl Status {
    /// Whether the queue is still expected to act on this.
    #[must_use]
    pub const fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

    /// Whether `Clear finished` should take it.
    #[must_use]
    pub const fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// One file moving in one direction.
///
/// A folder is not a transfer. It expands to one of these per file, because a
/// single row for a directory can only report a percentage that means nothing
/// when one file in it fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    /// Stable across restarts, so a restored row is the same row.
    pub id: String,
    pub direction: Direction,
    /// Base name, for the row's title.
    pub name: String,
    /// Where the bytes come from.
    pub source_path: String,
    /// Where they are going.
    pub dest_path: String,
    pub bytes_done: u64,
    /// `0` when the size is not known yet, which a progress bar must handle.
    pub bytes_total: u64,
    /// Bytes per second, smoothed. `None` until there is enough to say.
    pub rate: Option<u64>,
    /// Seconds remaining at the current rate. `None` when it cannot be known.
    pub eta_seconds: Option<u64>,
    pub status: Status,
    /// Why it failed, shown verbatim on the row.
    pub error: Option<String>,
    /// When it was enqueued, so the queue keeps its order across a restart.
    pub queued_at_ms: i64,
}

impl Transfer {
    /// A new queued transfer.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        direction: Direction,
        source_path: impl Into<String>,
        dest_path: impl Into<String>,
        bytes_total: u64,
    ) -> Self {
        let dest_path = dest_path.into();
        let source_path = source_path.into();
        // The name comes from the *destination*: that is what the file will be
        // called, and a "Keep both" rename should show the name it landed as.
        let name = dest_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&dest_path)
            .to_owned();
        Self {
            id: id.into(),
            direction,
            name,
            source_path,
            dest_path,
            bytes_done: 0,
            bytes_total,
            rate: None,
            eta_seconds: None,
            status: Status::Queued,
            error: None,
            queued_at_ms: now_ms(),
        }
    }

    /// Fraction complete in `0.0..=1.0`, or `None` when the size is unknown.
    ///
    /// `None` rather than zero: a bar that sits at zero for a whole transfer
    /// looks broken, where an indeterminate one is honest about not knowing.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        if self.bytes_total == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some((self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0))
    }

    /// Whether this failed for a reason worth offering `Remount as RW` on.
    ///
    /// The design puts that button on a failed row only when it would help.
    /// Offering it on a network timeout would be a button that reconnects and
    /// changes nothing.
    #[must_use]
    pub fn needs_write_access(&self) -> bool {
        self.status == Status::Failed
            && self.direction == Direction::Up
            && self
                .error
                .as_deref()
                .is_some_and(|error| error.contains("read-only") || error.contains("permission"))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
