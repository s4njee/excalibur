//! The protocol message log (`twopane.md` XTP-E5-S1).
//!
//! The handoff is specific that this is not a convenience: "The log is where
//! protocol truth lives: mount command and its export flags, listing timings,
//! and failures verbatim". So the log is a stream the shell subscribes to and
//! a ring it can ask for, not a debug facility behind a build flag.
//!
//! **What resolution the lines are.** The design's stream is per *frame*: every
//! protocol command and response. `xsync-client` exposes no tap — there is no
//! callback, channel or observer anywhere in it, and every frame goes through a
//! private `Connection::request` — so that is XTP-E5-S0, in another repo
//! (`integration.md` §8). What this records instead is one line per *engine
//! verb*, at the `ShareBackend` seam, which is the resolution every backend can
//! actually produce. The pane says which it is showing rather than implying
//! frames it does not have; when the tap lands, frame lines join this stream
//! and nothing above has to change.
//!
//! A bounded ring, because a log that grows with uptime is a leak with a nice
//! name. The oldest lines go first and the count of what was dropped is not
//! kept: a message log is a tail, and a user asking "what just happened" is
//! never asking about the line that fell off an hour ago.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// The handoff's six categories, which are also its six colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogCategory {
    /// Something this app asked for.
    Command,
    /// What came back.
    Response,
    /// A lifecycle fact: mounted, reconnecting, gone.
    Status,
    /// A directory listing, with its timing.
    Listing,
    /// Something the user could do about the line above.
    Hint,
    /// A failure, in the words the server or the OS used.
    Error,
}

/// A recovery a line offers, named rather than described.
///
/// `2b` draws `Remount as RW` on the hint line under a read-only error, and
/// `remount_read_write` is already a command — so the line carries which verb
/// rather than a label the shell has to match on prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogAction {
    /// Reconnect this mount asking for read-write.
    RemountRw,
}

/// What a line was made from.
///
/// The design's log is per protocol frame, and only xsync can produce those —
/// through the tap its client gained for this (XTP-E5-S0). SMB, NFS and SFTP
/// have no frames to expose, and their mounts are recorded a verb at a time at
/// the `ShareBackend` seam. Both land in one log, so each line says which it
/// is and the pane can tell the reader what it is showing rather than implying
/// frames it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// One engine verb: `list_dir`, `rename`, a mount's life.
    #[default]
    Verb,
    /// One protocol frame, as it crossed the wire.
    Frame,
}

/// One line of the log, as the shell renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Milliseconds since the Unix epoch; the pane draws `HH:MM:SS`.
    pub at: i64,
    pub category: LogCategory,
    /// Verbatim. A message the server sent is never paraphrased here.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<LogAction>,
    /// Which mount the line is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Whether this is a frame or a verb.
    pub resolution: Resolution,
}

impl LogLine {
    /// A line about no mount in particular.
    pub fn new(category: LogCategory, text: impl Into<String>) -> Self {
        Self {
            at: now_ms(),
            category,
            text: text.into(),
            action: None,
            mount: None,
            resolution: Resolution::Verb,
        }
    }

    /// The same, attributed to a mount.
    pub fn on(mount: impl Into<String>, category: LogCategory, text: impl Into<String>) -> Self {
        Self {
            mount: Some(mount.into()),
            ..Self::new(category, text)
        }
    }

    /// Offer a recovery on this line.
    #[must_use]
    pub fn with_action(mut self, action: LogAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Say this line is one frame off the wire rather than one engine verb.
    #[must_use]
    pub fn at_frame_resolution(mut self) -> Self {
        self.resolution = Resolution::Frame;
        self
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// How many lines the ring holds.
///
/// Enough to cover a mount, a browse and whatever went wrong — which is the
/// span a user reads a log over — and small enough that holding it costs
/// nothing worth measuring.
pub const LOG_CAPACITY: usize = 500;

/// The app's message log: a bounded ring, and a stream of what joins it.
#[derive(Debug)]
pub struct MessageLog {
    lines: Mutex<VecDeque<LogLine>>,
    sender: broadcast::Sender<LogLine>,
}

impl Default for MessageLog {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageLog {
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            lines: Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)),
            sender,
        }
    }

    /// Add a line, and tell whoever is listening.
    ///
    /// Never fails and never blocks the caller: a log that could refuse a line
    /// would put the recording of a failure inside the failure path. A send to
    /// nobody is fine, and a subscriber too slow to keep up loses lines rather
    /// than holding up the verb that wrote one.
    pub fn record(&self, line: LogLine) {
        if let Ok(mut lines) = self.lines.lock() {
            if lines.len() == LOG_CAPACITY {
                lines.pop_front();
            }
            lines.push_back(line.clone());
        }
        let _ = self.sender.send(line);
    }

    /// Everything the ring holds, oldest first.
    ///
    /// How a window opened later catches up: the stream only carries what
    /// happens next, and a log that started empty in every new window would
    /// lose exactly the lines explaining why the user opened it.
    #[must_use]
    pub fn lines(&self) -> Vec<LogLine> {
        self.lines
            .lock()
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Empty it. The stream is untouched; new lines still arrive.
    pub fn clear(&self) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.clear();
        }
    }

    /// Listen for lines from here on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.sender.subscribe()
    }
}
