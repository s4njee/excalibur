//! The one seam every protocol implements.

pub mod local;
pub mod mock;
pub mod nfs;
pub mod sftp;
/// Only with the `smb` feature: linking it makes the binary GPL-3.0-or-later.
#[cfg(feature = "smb")]
pub mod smb;
pub mod xsync;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::kind::Category;

use crate::error::{EngineError, Result};

/// Which protocol a connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// SMB2/3.
    Smb,
    /// NFS.
    Nfs,
    /// SFTP over SSH.
    Sftp,
    /// xsync protocol v3.
    Xsync,
    /// A directory on this machine, browsed through the same seam (E21).
    ///
    /// Not a network protocol, and deliberately not special-cased anywhere
    /// above the trait: the window sees one more kind of share, with the
    /// same listing, the same verbs and the same writability rule.
    Local,
}

impl Protocol {
    /// The URI scheme this protocol uses in the path field and bookmarks.
    #[must_use]
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::Smb => "smb",
            Self::Nfs => "nfs",
            Self::Sftp => "sftp",
            Self::Xsync => "xsync",
            Self::Local => "file",
        }
    }
}

/// What a bookmark needs to reach a share. Never contains a secret: those live
/// in the OS keychain, keyed by bookmark id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSpec {
    /// Stable identifier, also the keychain account.
    pub id: String,
    /// Display name in the sidebar.
    pub name: String,
    /// Protocol to speak.
    pub protocol: Protocol,
    /// Hostname or address.
    pub host: String,
    /// Port, or `None` for the protocol's default.
    pub port: Option<u16>,
    /// Share name, export path, or remote root.
    pub share: String,
    /// Username, where the protocol has one.
    pub username: Option<String>,
    /// Windows domain or workgroup. SMB is the only protocol with one, and it
    /// is carried here rather than folded into the username so that a domain
    /// containing a backslash cannot be mistaken for part of a name.
    #[serde(default)]
    pub domain: Option<String>,
    /// What the user asked for. The server decides what they get.
    pub mount_preference: Access,
}

/// Read-write or read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Read and write.
    Rw,
    /// Read only.
    Ro,
}

/// Where a mount is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MountStatus {
    /// Connecting.
    Mounting,
    /// Usable.
    Ready,
    /// Failed; see the banner.
    Error,
    /// Dropped and being re-established.
    Reconnecting,
}

/// Free and total space, as the status bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    /// Bytes this identity may actually use.
    pub free: u64,
    /// Total bytes.
    pub total: u64,
}

/// What a backend can do beyond the required set.
///
/// A capability that is false is not a failure; it changes what the UI offers.
/// `random_access` false, for example, means no video preview and whole-file
/// transfers rather than resumable ones.
// A capability set is a row of independent yes/no answers; grouping them into
// sub-structs to satisfy the lint would make every call site longer and say
// nothing extra.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Caps {
    /// Byte ranges can be read at an offset.
    pub random_access: bool,
    /// An interrupted upload can continue where it stopped.
    pub resume_write: bool,
    /// A temp-then-rename upload promotes atomically.
    pub atomic_rename: bool,
    /// Symlinks are represented.
    pub symlinks: bool,
    /// The backend can report directory changes.
    pub watch: bool,
}

/// Everything the status bar and the write gating read.
///
/// `writable` is the single derived value the design demands: it comes from the
/// protocol — SMB tree-connect flags, an NFS `ACCESS` probe, xsync's
/// `effective_writable` — never from the user's choice in the dialog, and it is
/// re-read on every reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountInfo {
    /// Lifecycle state.
    pub status: MountStatus,
    /// Whether this session may write. The only input to write gating.
    pub writable: bool,
    /// Why not, when `writable` is false. Shown verbatim in the status bar.
    pub reason: String,
    /// Protocol label, e.g. `SMB3` or `NFSv4.2`.
    pub protocol_label: String,
    /// Security label, e.g. `aes-128-gcm` or `krb5p`.
    pub security_label: String,
    /// Mount options, shown verbatim when the server supplies them.
    pub options: String,
    /// Free and total space, when known.
    pub capacity: Option<Capacity>,
    /// Optional capabilities.
    pub caps: Caps,
}

/// File, directory or symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// Anything else.
    Other,
}

/// Metadata for one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attrs {
    /// What it is.
    pub kind: EntryKind,
    /// Logical size in bytes.
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch.
    pub modified_ms: Option<i64>,
    /// Unix mode bits, when the protocol exposes them.
    pub mode: Option<u32>,
    /// Symlink target, when it is one.
    pub link_target: Option<String>,
    /// An opaque value that changes whenever the file does.
    ///
    /// `None` for a protocol that has no such thing, which is every one of
    /// them except xsync today. Edit-in-place falls back to comparing size and
    /// mtime there — weaker, because a same-size edit inside mtime granularity
    /// slips through, but it is what SMB, NFS and SFTP can offer.
    pub change_cookie: Option<Vec<u8>>,
}

impl Attrs {
    /// Whether this describes the same content as `other`.
    ///
    /// The cookie decides when both sides have one. Otherwise size and mtime
    /// together, which is the same comparison xsync's own v2 `Publish` used
    /// and the strongest thing available without one.
    #[must_use]
    pub fn unchanged_from(&self, other: &Self) -> bool {
        match (&self.change_cookie, &other.change_cookie) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => self.size == other.size && self.modified_ms == other.modified_ms,
        }
    }
}

/// What a compare-and-swap write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasOutcome {
    /// Written. Carries the destination's attributes afterwards.
    Written(Attrs),
    /// Refused: the destination is not what the caller expected. Carries what
    /// it is now, so the caller can offer replace / keep both / cancel.
    Changed(Attrs),
}

/// One row in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Base name, no separators.
    pub name: String,
    /// Full path in the share.
    pub path: String,
    /// Metadata.
    #[serde(flatten)]
    pub attrs: Attrs,
    /// Human-readable kind for the Kind column, e.g. "Matroska Video".
    pub kind_label: String,
    /// What sort of thing it is, for the row glyph and Quick Look's routing.
    ///
    /// Beside the label rather than derived from it in the window: the label is
    /// prose meant for a person, and matching on prose to decide which viewer
    /// to open is how "Matroska Video" and "MPEG-4 Video" end up handled
    /// differently by accident.
    pub kind_category: Category,
    /// A dotfile, or the platform's hidden flag.
    pub hidden: bool,
}

/// A directory listing, with the mount facts that must not drift from it.
///
/// Writability travels *with* the listing rather than being fetched
/// separately, because the status bar and the row it describes have to agree:
/// two round trips could disagree, and the badge is the thing a user trusts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    /// The directory's entries, unsorted.
    pub entries: Vec<Entry>,
    /// The mount as of this listing.
    pub mount: MountInfo,
}

/// How to open a file for writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Truncate or create at offset zero.
    Create,
    /// Continue an interrupted write at `offset`.
    Resume {
        /// Byte offset to resume at.
        offset: u64,
    },
}

/// Liveness of a connection, for the status bar dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    /// Answering.
    Healthy,
    /// Not answering.
    Failed,
}

/// One protocol, as the rest of the app sees it.
///
/// Object-safe so a session can hold `Box<dyn ShareBackend>` without knowing
/// which protocol it got.
#[async_trait]
pub trait ShareBackend: Send + Sync {
    /// Mount facts as of the last exchange with the server.
    fn mount_info(&self) -> MountInfo;

    /// What this backend can do beyond the required set.
    fn caps(&self) -> Caps;

    /// Free and total space.
    async fn capacity(&self) -> Result<Capacity>;

    /// List one directory, with the mount facts that go with it.
    async fn list_dir(&self, path: &str) -> Result<Listing>;

    /// Metadata for one path.
    async fn stat(&self, path: &str) -> Result<Attrs>;

    /// Read `length` bytes at `offset`.
    ///
    /// Ranges rather than a stream: previews, media seeking and resumable
    /// transfers all ask for a range, and a backend without random access
    /// reports `caps().random_access == false` rather than emulating it.
    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>>;

    /// Open `path` once, for a caller that will read all of it (E13-S7).
    ///
    /// `None` — the default — means this backend has nothing better than
    /// `read_range` to offer, and the caller should wrap that instead. Use
    /// [`read_handle`] rather than calling this directly: it does the
    /// wrapping, so a caller has one shape either way.
    ///
    /// Overriding it is worth it only where an open costs a round trip that a
    /// handle then stops paying per chunk. xsync does; SFTP, NFS and SMB
    /// currently do not, and their transfers are unaffected by this method
    /// existing.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be opened. A backend that simply has no
    /// handle to give returns `Ok(None)`, which is not a failure.
    async fn open_read(&self, path: &str) -> Result<Option<Arc<dyn ReadHandle>>> {
        let _ = path;
        Ok(None)
    }

    /// Write `data` at `offset` under `mode`.
    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64>;

    /// Create one directory.
    async fn mkdir(&self, path: &str) -> Result<()>;

    /// Rename, optionally replacing an existing destination.
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()>;

    /// Remove a file, or a directory and its contents when `recursive`.
    async fn remove(&self, path: &str, recursive: bool) -> Result<()>;

    /// Replace a file, but only if it is still what the caller last saw.
    ///
    /// The default is a stat, a comparison and a write — portable, and what
    /// SMB, NFS and SFTP will be able to do. It is **not atomic**: another
    /// writer between the stat and the write is not detected, which is the
    /// race a real compare-and-swap closes.
    ///
    /// A backend that can do better overrides this. xsync does, with
    /// `StageCommit` and `expect_cookie` (`xsyncv3.md` E4-S7): the check
    /// happens immediately before the publishing rename, so nothing can slip
    /// between them.
    ///
    /// `expected` of `None` writes unconditionally.
    ///
    /// # Errors
    ///
    /// Fails when the write itself fails. A *refusal* is `Ok(Changed)`, not an
    /// error: the destination having moved is an answer the caller asked for.
    async fn write_all_if_unchanged(
        &self,
        path: &str,
        expected: Option<&Attrs>,
        data: &[u8],
    ) -> Result<CasOutcome> {
        if let Some(expected) = expected {
            match self.stat(path).await {
                Ok(current) if !current.unchanged_from(expected) => {
                    return Ok(CasOutcome::Changed(current))
                }
                Ok(_) => {}
                // Gone is a change: the caller expected a file and there is
                // none. Recreating it silently would resurrect something
                // somebody deleted.
                Err(EngineError::NotFound(_)) => {
                    return Ok(CasOutcome::Changed(Attrs {
                        kind: EntryKind::Other,
                        size: 0,
                        modified_ms: None,
                        mode: None,
                        link_target: None,
                        change_cookie: None,
                    }))
                }
                Err(error) => return Err(error),
            }
        }
        self.write_range(path, WriteMode::Create, data).await?;
        Ok(CasOutcome::Written(self.stat(path).await?))
    }

    /// Whether the connection is still answering.
    async fn ping(&self) -> Result<Health>;
}

/// One file, opened once and read many times (E13-S7).
///
/// `read_range` is the right seam for a preview or a media seek: a seek is a
/// range, and each one is unrelated to the last. It is the wrong one for a
/// transfer, where the same file is read a thousand times in order and every
/// call pays for an open and a close it did not need — three round trips per
/// megabyte on xsync, which is what held a 1 GiB download to a quarter of the
/// link.
///
/// A handle is opened once per download attempt and closed when it finishes.
#[async_trait]
pub trait ReadHandle: Send + Sync {
    /// Read `length` bytes at `offset`.
    ///
    /// **Callable concurrently on one handle**, and meant to be: the offset
    /// travels in the request, so there is no cursor to protect and the
    /// pipelining is the whole point.
    ///
    /// Returns fewer bytes than asked for **only at the end of the file**.
    /// Every backend in the tree already honours that in `read_range` — SMB
    /// and NFS loop internally to make it true — so a caller may treat a
    /// short read as the end and stop.
    ///
    /// # Errors
    ///
    /// Fails as the underlying read does. `length` above [`Self::max_read`]
    /// is a failure, not a short read: silently truncating would look like an
    /// end of file that is not there.
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>>;

    /// The largest single read this handle accepts.
    fn max_read(&self) -> u32;

    /// Release the handle.
    ///
    /// Explicit rather than left to `Drop`, because dropping cannot await and
    /// cannot report. Calling it twice is not an error; reading after it is.
    ///
    /// # Errors
    ///
    /// Fails when the close itself is refused or the connection has gone.
    async fn close(&self) -> Result<()>;
}

/// What [`RangeReads`] claims as its limit.
///
/// The transfer worker's own chunk, so wrapping `read_range` neither splits a
/// chunk the backend would have taken whole nor invents a bigger one.
const RANGE_MAX_READ: u32 = 1024 * 1024;

/// A [`ReadHandle`] over `read_range`, for a backend that has no handle.
///
/// It is the honest wrapper rather than a fast path: every call still opens
/// and closes underneath. What it buys is one shape at the call site, so the
/// worker has a single code path and SFTP, NFS and SMB behave exactly as they
/// did before this trait existed.
struct RangeReads {
    share: Arc<dyn ShareBackend>,
    path: String,
}

#[async_trait]
impl ReadHandle for RangeReads {
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.share.read_range(&self.path, offset, length).await
    }

    fn max_read(&self) -> u32 {
        RANGE_MAX_READ
    }

    async fn close(&self) -> Result<()> {
        // Nothing was held open: each read closed itself.
        Ok(())
    }
}

/// A handle for reading all of `path`, whatever the backend can give.
///
/// The backend's own handle where it has one, and a wrapper around
/// `read_range` where it does not.
///
/// # Errors
///
/// Fails when the backend has a handle to give and cannot open the file.
pub async fn read_handle(share: &Arc<dyn ShareBackend>, path: &str) -> Result<Arc<dyn ReadHandle>> {
    if let Some(handle) = share.open_read(path).await? {
        return Ok(handle);
    }
    Ok(Arc::new(RangeReads {
        share: Arc::clone(share),
        path: path.to_owned(),
    }))
}
