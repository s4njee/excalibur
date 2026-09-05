//! The local filesystem backend (E21).
//!
//! A directory on this machine, behind the same seam as the four network
//! protocols. Nothing above the trait learns that it is local: the listing,
//! the verbs, the transfer worker and the writability rule are the ones that
//! were already written, which is the whole reason this is a backend rather
//! than a mode of the window.
//!
//! Two things here are not like the others. The first is that every call is a
//! syscall rather than a round trip, so the work runs on
//! [`tokio::task::spawn_blocking`] — a `statvfs` on a stalled network volume
//! under `/Volumes` would otherwise park a runtime thread. The second is
//! removal: on your own Mac, deleting a file must not be more permanent than
//! Finder makes it, so `remove` moves to the Trash and only falls back to a
//! real delete where the platform has none. The mount's `options` says which
//! of the two this build does.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, MountStatus, ReadHandle,
    ShareBackend, WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// The largest single read a [`LocalReads`] handle accepts.
///
/// It bounds one allocation, so it is a real limit rather than a formality: a
/// caller asking for `u32::MAX` would otherwise reserve four gigabytes before
/// the first byte is read. Eight times the transfer worker's chunk, so nothing
/// it asks for is ever split.
const MAX_READ: u32 = 8 * 1024 * 1024;

/// The local backend.
///
/// `root` is canonical: resolved once, at open, so that a root reached through
/// a symlink (`/tmp`, which is `/private/tmp`) is the same string every path
/// is built from. Paths *inside* it are deliberately never canonicalised —
/// see [`LocalBackend::resolve`].
pub struct LocalBackend {
    /// The share root, absolute and canonical.
    root: PathBuf,
    /// Mount facts as of `open`.
    info: MountInfo,
}

impl LocalBackend {
    /// Open a directory on this machine as a share.
    ///
    /// Writability is *probed*, not assumed: a temp file is created and
    /// removed in `root`, because the permission bits, the ACL and the mount
    /// flags all have a say and only an attempted write asks all three at
    /// once.
    ///
    /// # Errors
    ///
    /// Fails when `root` does not exist, cannot be read, or is not a
    /// directory. A root that exists but cannot be written to is not a
    /// failure: it is a read-only mount, with a reason the status bar shows.
    pub async fn open(root: &Path, read_only: bool) -> Result<Self> {
        let requested = root.to_path_buf();
        let root = blocking(move || {
            let metadata =
                std::fs::metadata(&requested).map_err(|error| map_error(&error, &requested))?;
            if !metadata.is_dir() {
                return Err(EngineError::Unsupported(format!(
                    "{} is not a directory",
                    requested.display()
                )));
            }
            // Once, here, and never again per call: the root is a place the
            // user picked and it should read the same however they spelled it.
            std::fs::canonicalize(&requested).map_err(|error| map_error(&error, &requested))
        })
        .await?;

        let probe_root = root.clone();
        let (writable, reason) = if read_only {
            (false, "Read-only — mounted read-only".to_owned())
        } else {
            match blocking(move || probe_write(&probe_root)).await {
                Ok(()) => (true, String::new()),
                // The usual case is permission, and it is worth naming as
                // such. Anything else — a volume mounted read-only, a full
                // disk — says what it actually was rather than being reported
                // as a permission problem the user would go and look for.
                Err(EngineError::PermissionDenied(_)) => (
                    false,
                    format!("Read-only — no write permission on {}", root.display()),
                ),
                Err(error) => (
                    false,
                    format!("Read-only — {} on {}", plainly(&error), root.display()),
                ),
            }
        };

        let capacity_root = root.clone();
        let capacity = blocking(move || capacity_of(&capacity_root)).await.ok();

        Ok(Self {
            root,
            info: MountInfo {
                status: MountStatus::Ready,
                writable,
                reason,
                // Both labels are the same word on purpose. There is no
                // protocol and no transport security to name, and inventing
                // "APFS" or "none" for the status bar would be claiming to
                // have measured something nobody asked the filesystem.
                protocol_label: "local".to_owned(),
                security_label: "local".to_owned(),
                // What `remove` does, said out loud, because it is the one
                // behaviour here a user could be surprised by.
                options: recycle::OPTIONS.to_owned(),
                capacity,
                caps: Caps {
                    random_access: true,
                    resume_write: true,
                    // `rename(2)` replaces atomically.
                    atomic_rename: true,
                    symlinks: true,
                    // A watcher is E21's next story at the earliest: it needs
                    // FSEvents or inotify and a debounce, and a listing that
                    // refreshes on focus is what the window does today.
                    watch: false,
                },
            },
        })
    }

    /// The share root, absolute and canonical.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A path on this machine, from one relative to the share.
    ///
    /// Confinement is a property of the **path model**, not a privilege
    /// boundary. Every component has to be an ordinary name: `""` is the root,
    /// and an absolute path, a `..`, a `.`, an empty component (which is what
    /// a leading, trailing or doubled `/` produces) or a NUL is refused as
    /// `NotFound` rather than normalised into something that resolves. Refused
    /// and not repaired, because a path that climbs out of the share is either
    /// a bug above this line or an attempt, and neither should be served.
    ///
    /// What this deliberately does **not** do is canonicalise the result. A
    /// symlink inside the share that points outside it stays listable and
    /// openable, because this is the user's own machine and Finder follows
    /// them — and because a `realpath` per call would cost a syscall per
    /// component on every listing to enforce a boundary the user's own
    /// permissions already enforce. The share root is the model's origin, not
    /// a sandbox: anything the user can reach in Finder they can reach here.
    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let refuse = || EngineError::NotFound(format!("{path} is not a path in this share"));
        if path.is_empty() {
            return Ok(self.root.clone());
        }
        if path.contains('\0') {
            return Err(refuse());
        }
        let mut resolved = self.root.clone();
        for component in path.split('/') {
            // One ordinary name, and nothing else. `Path` does the deciding so
            // that a platform's own idea of a separator or a prefix — `C:\` on
            // Windows, say — cannot slip through a hand-written check.
            let mut parts = Path::new(component).components();
            match (parts.next(), parts.next()) {
                (Some(Component::Normal(name)), None) => resolved.push(name),
                _ => return Err(refuse()),
            }
        }
        Ok(resolved)
    }

    /// The mount, with capacity as of now.
    ///
    /// Listings carry it so the status bar cannot drift from the rows above
    /// it, and free space on a local disk changes while you look at it.
    async fn snapshot(&self) -> MountInfo {
        let mut info = self.info.clone();
        let root = self.root.clone();
        info.capacity = blocking(move || capacity_of(&root)).await.ok();
        info
    }

    /// Refuse a write against a read-only mount, with the reason.
    fn writable(&self) -> Result<()> {
        if self.info.writable {
            return Ok(());
        }
        Err(EngineError::ReadOnly(self.info.reason.clone()))
    }
}

#[async_trait]
impl ShareBackend for LocalBackend {
    fn mount_info(&self) -> MountInfo {
        self.info.clone()
    }

    fn caps(&self) -> Caps {
        self.info.caps
    }

    async fn capacity(&self) -> Result<Capacity> {
        let root = self.root.clone();
        blocking(move || capacity_of(&root)).await
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let directory = self.resolve(path)?;
        let prefix = path.to_owned();
        let entries = blocking(move || {
            let reader =
                std::fs::read_dir(&directory).map_err(|error| map_error(&error, &directory))?;
            let mut rows = Vec::new();
            for entry in reader {
                let entry = entry.map_err(|error| map_error(&error, &directory))?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    // A name that is not UTF-8 cannot be addressed by the
                    // share's path model, so listing it would put a row on
                    // screen whose every verb fails. Lossy conversion would be
                    // worse: the repaired name resolves to nothing, or — with
                    // two such names in one directory — to the wrong file.
                    continue;
                };
                // `symlink_metadata`, so a link is a link. `stat` follows, the
                // same split SFTP has between `readdir` and `SSH_FXP_STAT`.
                // A file deleted between the readdir and the stat is not an
                // error for the listing; it simply is not there any more.
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let link_target = if metadata.is_symlink() {
                    std::fs::read_link(entry.path())
                        .ok()
                        .map(|target| target.to_string_lossy().into_owned())
                } else {
                    None
                };
                let attrs = attrs_of(&metadata, link_target);
                let kind = classify_entry(&name, attrs.kind);
                rows.push(Entry {
                    path: if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    },
                    hidden: hidden(&name, &metadata),
                    name,
                    attrs,
                    kind_label: kind.label,
                    kind_category: kind.category,
                });
            }
            Ok(rows)
        })
        .await?;

        Ok(Listing {
            entries,
            mount: self.snapshot().await,
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let resolved = self.resolve(path)?;
        blocking(move || {
            // Follows, so that a symlink stats as what it points at and a
            // download of one copies the file rather than the link. The
            // listing's `symlink_metadata` is what shows it as a link.
            let metadata =
                std::fs::metadata(&resolved).map_err(|error| map_error(&error, &resolved))?;
            let link_target = std::fs::read_link(&resolved)
                .ok()
                .map(|target| target.to_string_lossy().into_owned());
            Ok(attrs_of(&metadata, link_target))
        })
        .await
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let resolved = self.resolve(path)?;
        blocking(move || {
            let file =
                std::fs::File::open(&resolved).map_err(|error| map_error(&error, &resolved))?;
            read_positional(&file, offset, length, &resolved)
        })
        .await
    }

    async fn open_read(&self, path: &str) -> Result<Option<Arc<dyn ReadHandle>>> {
        let resolved = self.resolve(path)?;
        let opened = resolved.clone();
        let file = blocking(move || {
            std::fs::File::open(&opened).map_err(|error| map_error(&error, &opened))
        })
        .await?;
        Ok(Some(Arc::new(LocalReads {
            file: std::sync::Mutex::new(Some(Arc::new(file))),
            path: resolved,
        })))
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        self.writable()?;
        let resolved = self.resolve(path)?;
        let data = data.to_vec();
        blocking(move || {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true);
            let offset = match mode {
                WriteMode::Create => {
                    options.truncate(true);
                    0
                }
                // Never `append`: a resumed upload has to land at the offset it
                // stopped at, and append puts it at whatever the length happens
                // to be — the same thing only when nothing else has touched the
                // file.
                WriteMode::Resume { offset } => offset,
            };
            let file = options
                .open(&resolved)
                .map_err(|error| map_error(&error, &resolved))?;
            write_positional(&file, offset, &data, &resolved)?;
            Ok(data.len() as u64)
        })
        .await
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        self.writable()?;
        let resolved = self.resolve(path)?;
        blocking(move || {
            // One level, like every other backend's `mkdir`: creating the
            // missing parents of a path the user only half-typed is a decision
            // that belongs above this line.
            std::fs::create_dir(&resolved).map_err(|error| map_error(&error, &resolved))
        })
        .await
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.writable()?;
        let from = self.resolve(from)?;
        let to = self.resolve(to)?;
        blocking(move || {
            if !replace && std::fs::symlink_metadata(&to).is_ok() {
                // `rename(2)` would replace it silently. The trait's `replace`
                // flag exists precisely so that a caller who did not ask for
                // that gets told instead.
                return Err(EngineError::AlreadyExists(to.display().to_string()));
            }
            std::fs::rename(&from, &to).map_err(|error| map_error(&error, &from))
        })
        .await
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.writable()?;
        let resolved = self.resolve(path)?;
        if resolved == self.root {
            return Err(EngineError::Unsupported(
                "the share's own root cannot be removed".to_owned(),
            ));
        }
        blocking(move || {
            // `symlink_metadata`: removing a symlink removes the link, never
            // what it points at, which is what makes a link out of the share
            // safe to delete.
            let metadata = std::fs::symlink_metadata(&resolved)
                .map_err(|error| map_error(&error, &resolved))?;
            if metadata.is_dir() && !recursive {
                let mut children =
                    std::fs::read_dir(&resolved).map_err(|error| map_error(&error, &resolved))?;
                if children.next().is_some() {
                    return Err(EngineError::Io(format!(
                        "{} is not empty",
                        resolved.display()
                    )));
                }
            }
            // A folder goes whole, contents and all — one item in the Trash,
            // one Put Back. Emptying it first would leave the user with a
            // folder they can restore and nothing in it.
            recycle::delete(&resolved, metadata.is_dir())
        })
        .await
    }

    async fn ping(&self) -> Result<Health> {
        let root = self.root.clone();
        // A local root goes away when a volume is ejected or the folder is
        // renamed under us, which is the same answer the network backends give
        // for a connection that stopped answering.
        blocking(move || {
            Ok(if std::fs::metadata(&root).is_ok_and(|it| it.is_dir()) {
                Health::Healthy
            } else {
                Health::Failed
            })
        })
        .await
    }
}

/// One file, opened once and read concurrently until the transfer is done
/// (E13-S7).
///
/// The `Mutex` guards the *handle*, never a read: it is held long enough to
/// clone an `Arc` and is not held across an await. The reads themselves are
/// positional, so there is no cursor to protect and any number of them can be
/// in flight on one open file.
///
/// The `Option` is what makes `close` mean something. Taking the handle out of
/// it is what closes the file, and it also makes a read after a close an error
/// rather than a read of a descriptor that has been handed back to the OS.
struct LocalReads {
    /// The open file, until it is closed.
    file: std::sync::Mutex<Option<Arc<std::fs::File>>>,
    /// Where it came from, for the error messages.
    path: PathBuf,
}

impl LocalReads {
    /// The handle, or why there is none.
    fn file(&self) -> Result<Arc<std::fs::File>> {
        self.file
            .lock()
            .map_err(|_| EngineError::Io("the read handle's lock is poisoned".to_owned()))?
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| EngineError::Io("read after close".to_owned()))
    }
}

#[async_trait]
impl ReadHandle for LocalReads {
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        let file = self.file()?;
        let path = self.path.clone();
        blocking(move || read_positional(&file, offset, length, &path)).await
    }

    fn max_read(&self) -> u32 {
        MAX_READ
    }

    async fn close(&self) -> Result<()> {
        let taken = self
            .file
            .lock()
            .map_err(|_| EngineError::Io("the read handle's lock is poisoned".to_owned()))?
            .take();
        // Dropping the last `Arc` closes the descriptor. A read still in
        // flight holds one of its own, so the close waits for it rather than
        // pulling the file out from under it. Closing twice is not an error.
        drop(taken);
        Ok(())
    }
}

/// Run one piece of filesystem work off the runtime's threads.
///
/// Every syscall here goes through this. `read_dir` on a stalled network
/// volume under `/Volumes` blocks for as long as the volume takes, and doing
/// that on a runtime thread stalls every other mount in the process.
async fn blocking<T, F>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| EngineError::Io(format!("filesystem task: {error}")))?
}

/// The `io:`-shaped part of an error, without the engine's own prefix.
///
/// The reason string is shown verbatim in the status bar, where "read-only —
/// io: /Volumes/Archive: Read-only file system (os error 30)" is noise around
/// the four words that matter.
fn plainly(error: &EngineError) -> String {
    let text = error.to_string();
    let text = text
        .split_once(": ")
        .map_or(text.as_str(), |(_, rest)| rest);
    let text = text
        .rsplit_once(" (os error ")
        .map_or(text, |(head, _)| head);
    text.split_once(": ")
        .map_or(text, |(_, rest)| rest)
        .to_lowercase()
}

/// Create a file in `root` and remove it, to learn whether this user may write.
///
/// Unique per attempt, so a probe left behind by a killed process cannot make
/// a writable folder look read-only; and removed on every exit path, including
/// the one where the create succeeded and something later failed.
fn probe_write(root: &Path) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let probe = root.join(format!(
        ".excalibur-write-probe-{}-{stamp}",
        std::process::id()
    ));
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);
    match created {
        Ok(file) => {
            drop(file);
            // Best effort: a probe left behind is untidy, not incorrect, and
            // failing the mount because the cleanup failed would be worse.
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(map_error(&error, &probe)),
    }
}

/// Free and total bytes for the filesystem `root` lives on.
///
/// `available_space` rather than `free_space`: the status bar's number should
/// be what this user can actually use, which on a filesystem with reserved
/// blocks is the smaller of the two.
fn capacity_of(root: &Path) -> Result<Capacity> {
    let stats = fs4::statvfs(root).map_err(|error| map_error(&error, root))?;
    Ok(Capacity {
        free: stats.available_space(),
        total: stats.total_space(),
    })
}

/// Read one range, positionally, without a cursor.
fn read_positional(file: &std::fs::File, offset: u64, length: u32, path: &Path) -> Result<Vec<u8>> {
    if length > MAX_READ {
        return Err(EngineError::Io(format!(
            "a read of {length} bytes is larger than this backend's {MAX_READ}"
        )));
    }
    let mut buffer = vec![0_u8; length as usize];
    let mut filled = 0_usize;
    while filled < buffer.len() {
        match positional_read(file, &mut buffer[filled..], offset + filled as u64) {
            // The end of the file, and the only reason a short read is
            // allowed: the trait promises callers may stop on one.
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(map_error(&error, path)),
        }
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// Write all of `data` at `offset`, positionally.
fn write_positional(file: &std::fs::File, offset: u64, data: &[u8], path: &Path) -> Result<()> {
    let mut written = 0_usize;
    while written < data.len() {
        match positional_write(file, &data[written..], offset + written as u64) {
            Ok(0) => {
                return Err(EngineError::Io(format!(
                    "{} accepted no more bytes",
                    path.display()
                )))
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(map_error(&error, path)),
        }
    }
    Ok(())
}

/// One positional read. The two platforms spell it differently and mean the
/// same thing: no seek, no shared cursor, safe to call concurrently.
#[cfg(unix)]
fn positional_read(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, offset)
}

/// One positional read.
#[cfg(windows)]
fn positional_read(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
}

/// One positional write.
#[cfg(unix)]
fn positional_write(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(file, data, offset)
}

/// One positional write.
#[cfg(windows)]
fn positional_write(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(file, data, offset)
}

/// Filesystem metadata as the engine describes a file.
fn attrs_of(metadata: &std::fs::Metadata, link_target: Option<String>) -> Attrs {
    let kind = if metadata.is_dir() {
        EntryKind::Dir
    } else if metadata.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Attrs {
        kind,
        size: metadata.len(),
        modified_ms: metadata.modified().ok().map(millis),
        mode: mode_of(metadata),
        link_target,
        // No cheap, stable per-file version on a POSIX filesystem: the inode
        // generation is not exposed and `st_ctime` moves for reasons that are
        // not content. Edit-in-place falls back to size and mtime here, the
        // same as SFTP, NFS and SMB.
        change_cookie: None,
    }
}

/// Unix mode bits, where the platform has them.
///
/// The `Option` is the platform difference, not padding: the same function on
/// a target without mode bits answers `None`, and Properties (E14-S4) renders
/// the two cases differently.
#[allow(clippy::unnecessary_wraps)]
#[cfg(unix)]
fn mode_of(metadata: &std::fs::Metadata) -> Option<u32> {
    Some(std::os::unix::fs::PermissionsExt::mode(
        &metadata.permissions(),
    ))
}

/// Unix mode bits, where the platform has them.
#[cfg(not(unix))]
fn mode_of(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

/// Milliseconds since the Unix epoch, negative before it.
fn millis(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Whether an entry is hidden: a dotfile, or the platform's hidden flag.
fn hidden(name: &str, metadata: &std::fs::Metadata) -> bool {
    name.starts_with('.') || flagged_hidden(metadata)
}

/// `UF_HIDDEN`, which is how macOS hides a folder that is not dotted.
///
/// `~/Library` is the one every user meets: it carries the flag and no dot, so
/// a dotfile-only rule shows it in the middle of the home folder, which Finder
/// does not.
///
/// The constant is written out rather than taken from `libc`, which the engine
/// links only behind the `smb` feature. Pulling a whole FFI crate into the
/// default build for one integer that has been `0x8000` since 4.4BSD — it is
/// `UF_HIDDEN` in `sys/stat.h`, and it is ABI — is not a trade worth making.
#[cfg(target_os = "macos")]
fn flagged_hidden(metadata: &std::fs::Metadata) -> bool {
    const UF_HIDDEN: u32 = 0x8000;
    std::os::macos::fs::MetadataExt::st_flags(metadata) & UF_HIDDEN != 0
}

/// No equivalent flag on this platform, so hiddenness is the leading dot alone.
///
/// Windows has `FILE_ATTRIBUTE_HIDDEN` and would belong here; it is left out
/// because Windows is kept building rather than targeted, and an untested
/// hiddenness rule is worse than an honest absence.
#[cfg(not(target_os = "macos"))]
fn flagged_hidden(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Translate an OS error into the engine's taxonomy.
///
/// By `ErrorKind` rather than by message: the text is the platform's and
/// differs between macOS and Linux for the same errno.
fn map_error(error: &std::io::Error, path: &Path) -> EngineError {
    let where_ = path.display();
    let message = format!("{where_}: {error}");
    match error.kind() {
        std::io::ErrorKind::NotFound => EngineError::NotFound(message),
        std::io::ErrorKind::PermissionDenied => EngineError::PermissionDenied(message),
        std::io::ErrorKind::AlreadyExists => EngineError::AlreadyExists(message),
        _ => EngineError::Io(message),
    }
}

/// Removal, which is the one verb here that is not a thin wrapper over `std`.
///
/// Deleting a file on your own machine must not be more permanent than Finder
/// makes it, so the default is the Trash. The fallback is a real delete, on a
/// platform that has no Trash to move anything to — and the mount says which
/// one this build does, because the difference is the user's to know.
#[cfg(any(windows, all(unix, not(target_os = "ios"), not(target_os = "android"))))]
mod recycle {
    use std::path::Path;

    use crate::error::{EngineError, Result};

    /// What the mount reports in `options`.
    pub const OPTIONS: &str = "trash";

    /// Move one file or folder to the Trash.
    pub fn delete(path: &Path, _directory: bool) -> Result<()> {
        // Not "delete the contents, then the folder": `trash` moves the whole
        // thing, so what lands in the Trash is one item the user can put back.
        //
        // A failure is reported rather than turned into a real delete. A
        // volume with no Trash — a read-only image, some network mounts — is
        // exactly where silently making the delete permanent would be worst.
        trash::delete(path).map_err(|error| {
            EngineError::Io(format!("move {} to the Trash: {error}", path.display()))
        })
    }
}

/// Removal on a platform with no Trash: a real delete, said so in `options`.
#[cfg(not(any(windows, all(unix, not(target_os = "ios"), not(target_os = "android")))))]
mod recycle {
    use std::path::Path;

    use crate::error::Result;

    /// What the mount reports in `options`.
    pub const OPTIONS: &str = "permanent delete";

    /// Delete one file or folder outright.
    pub fn delete(path: &Path, directory: bool) -> Result<()> {
        let removed = if directory {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        removed.map_err(|error| super::map_error(&error, path))
    }
}

#[cfg(test)]
mod tests {
    use super::plainly;
    use crate::error::EngineError;

    #[test]
    fn the_read_only_reason_is_four_words_and_not_an_errno() {
        // What a volume mounted read-only answers the write-probe with. The
        // status bar shows this verbatim, so everything around the part a
        // person can act on has to come off.
        let error = EngineError::Io(
            "/Volumes/Archive/.excalibur-write-probe-1-2: Read-only file system (os error 30)"
                .to_owned(),
        );
        assert_eq!(plainly(&error), "read-only file system");
    }

    #[test]
    fn an_error_with_nothing_to_strip_survives_intact() {
        // Only the engine's own `io:` prefix comes off here: there is no path
        // and no errno, and a reason trimmed down to nothing would leave the
        // status bar saying "Read-only —  on /x".
        let error = EngineError::Io("disk quota exceeded".to_owned());
        assert_eq!(plainly(&error), "disk quota exceeded");
    }
}
