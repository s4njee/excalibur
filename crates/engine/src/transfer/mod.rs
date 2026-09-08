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
pub mod endpoint;
pub mod http;
pub mod plan;
pub mod queue;
pub mod rate;
pub mod schedule;
pub mod worker;
pub mod xsync_engine;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The queue every transfer written before named queues existed belongs to.
pub const DEFAULT_QUEUE_ID: &str = "default";

/// The app-level policy for a group of transfers.
///
/// This is deliberately a small model. The worker's process-wide semaphore is
/// still the hard ceiling; `concurrency` only limits how many rows from this
/// queue may occupy that ceiling at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedQueue {
    /// Stable across restarts and safe to use as a row foreign key.
    pub id: String,
    /// Label shown in the queue picker.
    pub name: String,
    /// A palette token or other UI colour identifier.
    pub color: String,
    /// Maximum number of rows this queue may run at once.
    pub concurrency: usize,
    /// Position among the named queues. Lower values run first.
    pub order: u64,
}

impl NamedQueue {
    /// The built-in queue used by legacy rows and new rows without a choice.
    #[must_use]
    pub fn default_queue() -> Self {
        Self {
            id: DEFAULT_QUEUE_ID.to_owned(),
            name: "Default".to_owned(),
            color: "gray".to_owned(),
            concurrency: 1,
            order: 0,
        }
    }
}

/// Which way the bytes are going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// From this machine to the share.
    Up,
    /// From the share to this machine.
    Down,
}

/// One end of a transfer.
///
/// The optional `mount` on a share is intentional: restored legacy rows may
/// have meant the active share rather than naming one explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Endpoint {
    /// A path on this machine.
    Local { path: String },
    /// A path in a mounted share.
    Share { mount: Option<String>, path: String },
    /// An HTTP(S) resource.
    Http { url: String },
}

/// HTTP download controls persisted with a queue row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpDownloadOptions {
    /// Requested ranged reads. `None` lets the downloader choose.
    pub connections: Option<usize>,
    /// Alternate URLs for the same object, excluding the primary URL.
    pub mirrors: Vec<String>,
    /// Zero is the primary URL; subsequent values index `mirrors`.
    pub preferred_mirror: Option<usize>,
    /// Per-download transient failure policy. `None` uses provider defaults.
    pub retry: Option<HttpRetryPolicy>,
}

/// Serializable retry controls accepted by the HTTP provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRetryPolicy {
    /// Total attempts, including the first request.
    pub max_attempts: u32,
    /// Wait before the second attempt, in seconds.
    pub initial_wait_seconds: u64,
    /// Ceiling for exponential waits and `Retry-After`, in seconds.
    pub max_wait_seconds: u64,
}

/// Effective HTTP limits advertised by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpDownloadLimits {
    pub max_connections: usize,
    pub max_per_host: usize,
}

/// Result of the synchronous HTTP capability probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProbe {
    pub length: Option<u64>,
    pub ranges: Option<bool>,
    pub filename: Option<String>,
    pub final_url: Option<String>,
    pub warning: Option<String>,
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

/// One item moving in one direction — a file, or a whole folder.
///
/// A folder used to expand to one of these per file, on the argument that a
/// single row for a directory can only report a percentage that means nothing
/// when one file in it fails. E22-S3 withdrew that: since every transfer runs
/// xsync's planner, the folder row's percentage is the planner's own, and the
/// per-file expansion bought its honesty by running one whole session per file
/// — 21 seconds of pure scheduling for a thousand small files that the session
/// itself moves in a third of one. [`Self::files_total`] is what a folder row
/// says instead of a number it made up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    /// Stable across restarts, so a restored row is the same row.
    pub id: String,
    /// Legacy direction, absent for endpoint-native routes such as HTTP →
    /// local. Defaulting it keeps queue files written before endpoint routes
    /// readable.
    #[serde(default)]
    pub direction: Option<Direction>,
    /// Base name, for the row's title.
    pub name: String,
    /// Where the bytes come from.
    #[serde(default)]
    pub source_path: String,
    /// Where they are going.
    #[serde(default)]
    pub dest_path: String,
    /// Canonical source and destination. Legacy rows use the path fields and
    /// are upgraded lazily by [`Self::from_endpoint`] and [`Self::to_endpoint`].
    #[serde(default)]
    pub from: Option<Endpoint>,
    #[serde(default)]
    pub to: Option<Endpoint>,
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
    /// The mount whose backend moves these bytes, when the row names one.
    ///
    /// A row used to mean "the app's most recent mount", which was already a
    /// race — a download queued against one share and started after the user
    /// switched to another would read from the wrong one — and with two windows
    /// open (E23-S1) it is not even ambiguous, it is simply wrong: a drag
    /// between windows is by definition a transfer whose share is *not* the one
    /// the app most recently touched (E24-S2).
    ///
    /// `None` keeps the old meaning for rows restored from a queue written
    /// before this field existed, which is why it is defaulted rather than
    /// required.
    #[serde(default)]
    pub mount: Option<String>,
    /// Whether every byte that landed was checked against the source's digest.
    ///
    /// A finished row says `verified` rather than `done` when this is set, and
    /// the difference is not decoration: the two words are claims of different
    /// strength and a row that said the stronger one for a chunk loop would be
    /// vouching for something nobody measured. Only the xsync route sets it
    /// (E22-S1) — `download` and `upload` move bytes they never hash.
    ///
    /// Defaulted, so a queue written before this field existed still loads and
    /// its rows read exactly as they did.
    #[serde(default)]
    pub verified: bool,
    /// A short phrase the finished row shows instead of a size.
    ///
    /// For "the file was already there, byte for byte" — which is a real
    /// outcome of a copy through a planner and is not the same as having moved
    /// the bytes. `error` is the wrong field for it: nothing failed.
    #[serde(default)]
    pub note: Option<String>,
    /// Files the planner has finished with, of the ones it found.
    ///
    /// Both are `0` until the planner's scan ends, the same way `bytes_total`
    /// is — and for a single-file row they stay that way, because a row that
    /// names one file has nothing to add by saying `1 file`. A folder row is
    /// the reason they exist: it is one row for a tree, and "412 of 1002
    /// files" is the part of that tree's progress a byte count cannot show.
    ///
    /// Defaulted, so a queue written before E22-S3 still loads.
    #[serde(default)]
    pub files_done: u64,
    #[serde(default)]
    pub files_total: u64,
    /// Optional HTTP-specific controls; absent in queues written before E3.
    #[serde(default)]
    pub http_options: Option<HttpDownloadOptions>,
    /// The named queue responsible for this row.
    #[serde(default = "default_queue_id")]
    pub queue_id: String,
    /// Optional explicit priority within [`queue_id`]. Lower values run first.
    /// `None` retains enqueue time ordering, which keeps legacy rows and rows
    /// added before the first drag reorder stable.
    #[serde(default)]
    pub queue_position: Option<u64>,
}

fn default_queue_id() -> String {
    DEFAULT_QUEUE_ID.to_owned()
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
            direction: Some(direction),
            name,
            source_path,
            dest_path,
            from: None,
            to: None,
            bytes_done: 0,
            bytes_total,
            rate: None,
            eta_seconds: None,
            status: Status::Queued,
            error: None,
            queued_at_ms: now_ms(),
            mount: None,
            verified: false,
            note: None,
            files_done: 0,
            files_total: 0,
            http_options: None,
            queue_id: default_queue_id(),
            queue_position: None,
        }
    }

    /// Name the share this row moves bytes through.
    ///
    /// A builder rather than a sixth argument to [`Self::new`]: most callers
    /// have exactly one mount in play and would pass the same value they
    /// already resolved, and the ones that matter — a drop from another
    /// window — are the ones that should have to say it out loud.
    #[must_use]
    pub fn on_mount(mut self, mount: impl Into<String>) -> Self {
        self.mount = Some(mount.into());
        self
    }

    /// Create an HTTP(S) download into `destination`.
    ///
    /// The URL is retained as entered only after validation. Credential-bearing
    /// userinfo and query/fragment credentials are refused until the
    /// keychain-backed credential story exists.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::EngineError::Unsupported`] when the URL is not
    /// HTTP(S), carries credentials it must not, or names a non-absolute
    /// destination folder.
    pub fn new_http(
        id: impl Into<String>,
        url: impl Into<String>,
        destination: impl Into<String>,
        bytes_total: u64,
    ) -> Result<Self, crate::error::EngineError> {
        let url = url.into();
        validate_http_url(&url)?;
        let directory = destination.into();
        if !std::path::Path::new(&directory).is_absolute() {
            return Err(crate::error::EngineError::Unsupported(
                "download destination must be an absolute local folder".to_owned(),
            ));
        }
        let name = xsync_download::name::from_url(&url);
        let dest_path = std::path::Path::new(&directory).join(&name);
        let dest_path = dest_path.to_string_lossy().into_owned();
        Ok(Self {
            id: id.into(),
            direction: None,
            name,
            source_path: url.clone(),
            dest_path: dest_path.clone(),
            from: Some(Endpoint::Http { url }),
            to: Some(Endpoint::Local { path: directory }),
            bytes_done: 0,
            bytes_total,
            rate: None,
            eta_seconds: None,
            status: Status::Queued,
            error: None,
            queued_at_ms: now_ms(),
            mount: None,
            verified: false,
            note: None,
            files_done: 0,
            files_total: 0,
            http_options: None,
            queue_id: default_queue_id(),
            queue_position: None,
        })
    }

    /// Create an HTTP download whose length is not known until probing.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::EngineError::Unsupported`] for the same reasons
    /// as [`Self::new_http`].
    pub fn http(
        id: impl Into<String>,
        url: impl Into<String>,
        destination: impl Into<String>,
    ) -> Result<Self, crate::error::EngineError> {
        Self::new_http(id, url, destination, 0)
    }

    /// Create an HTTP download with explicit connection and mirror controls.
    ///
    /// # Errors
    ///
    /// Returns an error when the primary URL, a mirror URL, the destination,
    /// connection count, or preferred mirror index is invalid.
    pub fn http_with_options(
        id: impl Into<String>,
        url: impl Into<String>,
        destination: impl Into<String>,
        options: HttpDownloadOptions,
    ) -> Result<Self, crate::error::EngineError> {
        let mut transfer = Self::http(id, url, destination)?;
        let primary = transfer.from_endpoint();
        validate_http_options(primary.as_ref(), &options)?;
        transfer.http_options = Some(options);
        Ok(transfer)
    }

    /// Canonical source, falling back to the legacy fields for old rows.
    #[must_use]
    pub fn from_endpoint(&self) -> Option<Endpoint> {
        self.from.clone().or_else(|| match self.direction {
            Some(Direction::Up) => Some(Endpoint::Local {
                path: self.source_path.clone(),
            }),
            Some(Direction::Down) => Some(Endpoint::Share {
                mount: self.mount.clone(),
                path: self.source_path.clone(),
            }),
            None => None,
        })
    }

    /// Canonical destination, falling back to the legacy fields for old rows.
    #[must_use]
    pub fn to_endpoint(&self) -> Option<Endpoint> {
        self.to.clone().or_else(|| match self.direction {
            Some(Direction::Up) => Some(Endpoint::Share {
                mount: self.mount.clone(),
                path: self.dest_path.clone(),
            }),
            Some(Direction::Down) => Some(Endpoint::Local {
                path: self.dest_path.clone(),
            }),
            None => None,
        })
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
            && self.direction == Some(Direction::Up)
            && self
                .error
                .as_deref()
                .is_some_and(|error| error.contains("read-only") || error.contains("permission"))
    }
}

pub(crate) fn validate_http_url(url: &str) -> Result<(), crate::error::EngineError> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://"))
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(crate::error::EngineError::Unsupported(
            "download URL must be HTTP or HTTPS".to_owned(),
        ));
    }
    let authority = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(crate::error::EngineError::Unsupported(
            "download URL credentials are not supported".to_owned(),
        ));
    }
    if url.contains(['?', '#']) {
        return Err(crate::error::EngineError::Unsupported(
            "download URL query and fragment credentials are not supported".to_owned(),
        ));
    }
    if xsync_download::redact_url(url) != url {
        return Err(crate::error::EngineError::Unsupported(
            "download URL credentials are not supported".to_owned(),
        ));
    }
    Ok(())
}

fn validate_http_options(
    primary: Option<&Endpoint>,
    options: &HttpDownloadOptions,
) -> Result<(), crate::error::EngineError> {
    if options.connections == Some(0) {
        return Err(crate::error::EngineError::Unsupported(
            "HTTP connection count must be at least one".to_owned(),
        ));
    }
    if let Some(retry) = options.retry {
        if retry.max_attempts == 0 || retry.max_attempts > 10 {
            return Err(crate::error::EngineError::Unsupported(
                "HTTP retry attempts must be between one and ten".to_owned(),
            ));
        }
        if retry.initial_wait_seconds > retry.max_wait_seconds || retry.max_wait_seconds > 300 {
            return Err(crate::error::EngineError::Unsupported(
                "HTTP retry waits must increase and stay within five minutes".to_owned(),
            ));
        }
    }
    let primary_url = match primary {
        Some(Endpoint::Http { url }) => url,
        _ => "",
    };
    for (index, mirror) in options.mirrors.iter().enumerate() {
        validate_http_url(mirror).map_err(|_| {
            crate::error::EngineError::Unsupported(format!("invalid mirror URL at index {index}"))
        })?;
        if mirror == primary_url {
            return Err(crate::error::EngineError::Unsupported(
                "mirror URL duplicates the primary URL".to_owned(),
            ));
        }
    }
    if let Some(index) = options.preferred_mirror {
        if index > options.mirrors.len() {
            return Err(crate::error::EngineError::Unsupported(
                "preferred mirror index is out of range".to_owned(),
            ));
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
