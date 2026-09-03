//! The one seam every protocol implements.

pub mod mock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

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

    /// Write `data` at `offset` under `mode`.
    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64>;

    /// Create one directory.
    async fn mkdir(&self, path: &str) -> Result<()>;

    /// Rename, optionally replacing an existing destination.
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()>;

    /// Remove a file, or a directory and its contents when `recursive`.
    async fn remove(&self, path: &str, recursive: bool) -> Result<()>;

    /// Whether the connection is still answering.
    async fn ping(&self) -> Result<Health>;
}
