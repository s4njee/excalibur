//! `ShareBackend` as an xsync endpoint (E22-S2).
//!
//! [`super::xsync_engine`] gives one pair — a folder on this machine and an
//! xsync export — xsync's planner, scheduler, hash cache and resume journal by
//! speaking xsync's own wire protocol to a second `xs --server`. Every other
//! pair had nothing but [`super::worker`]'s chunk loop, which had no planner at
//! all: a folder dropped onto an SMB share was one `read` of a directory and
//! one `EISDIR`.
//!
//! This file is the other half. `xsync-endpoint` exists precisely so a consumer
//! can plug filesystems it already speaks into that engine — its own module
//! comment names Excalibur — so NFS, SMB, SFTP and a local folder reach the
//! same planner through [`ShareSource`] and [`ShareSink`], and
//! `xsync_core::local::sync_endpoints` drives them.
//!
//! Four things shape the adapter.
//!
//! - **The seam is synchronous and [`ShareBackend`] is not.** xsync's traits
//!   take `&self` and return, and the engine calls them from its own worker
//!   *threads* — not from tasks. So the session runs on
//!   [`tokio::task::spawn_blocking`], as E22-S1's does, and each trait method
//!   pushes its future onto the runtime and blocks the calling thread until the
//!   answer comes back. See [`block_on`] for why it is spawn-and-wait rather
//!   than [`tokio::runtime::Handle::block_on`].
//!
//! - **Staging is temp-then-rename, in the destination's own directory.** The
//!   sink's `stage_open`/`stage_write`/`stage_commit` map onto
//!   `write_range` + `rename`, so nothing appears under its final name until
//!   every byte is there. Where a backend renames atomically — xsync's
//!   `renameat2`, SFTP's `posix-rename@openssh.com` — it is atomic, and
//!   [`ShareBackend::rename`] is already the place each backend decided that.
//!
//! - **A single-file transfer is not its parent directory.** xsync's own
//!   layout roots a file source at the parent and leans on a filter to narrow
//!   it; a file manager cannot build a glob out of a filename a server chose.
//!   [`Only`] narrows *any* source to one named entry instead, which is also
//!   what publishes it under a different name — so E22-S1's hard-link staging
//!   for "Keep both" is gone.
//!
//! - **`delete` is off and stays off**, for the reason E22-S1 gives: this is a
//!   copy, not a sync, and a transfer must not remove files at the destination
//!   nobody asked about.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use xsync_core::handle::ProgressWatch;
use xsync_core::local::{sync_endpoints, LocalSyncError, LocalSyncOptions, LocalSyncReport};
use xsync_core::sink::LocalSink;
use xsync_core::source::LocalSource;
use xsync_endpoint::{
    blake3, CancelToken, EndpointError, EndpointErrorKind, Enumeration, FileEntry, Source,
    SourceFingerprint, SymlinkTarget, SymlinkTargetKind, WirePath,
};

use super::rate::Progress;
use super::xsync_engine::{Moved, Settled, Tally};
use super::{Direction, Transfer};
use crate::backend::{Attrs, EntryKind, ShareBackend, WriteMode};
use crate::error::{EngineError, Result};

/// How often the worker's stop flag is turned into a cancel.
///
/// The same tenth of a second E22-S1 polls at, and for the same reason:
/// [`super::worker::Worker::stop`] sets an `AtomicBool` and has no channel to
/// wake anyone with. Unlike E22-S1 this does not have to kill a child — the
/// engine takes a [`CancelToken`] and drains what is in flight, which is why a
/// cancelled transfer here leaves a resumable stage rather than a torn file.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// Bytes per `write_range` when a whole file is published in one call.
///
/// The small-file path hands the sink an entire file, and "small" is the
/// engine's threshold, not ours. Splitting at a megabyte keeps one write inside
/// what every backend here accepts without asking each of them.
const WRITE_CHUNK: usize = 1024 * 1024;

/// Directories visited before a share walk gives up.
///
/// The same backstop [`super::plan`] carries, and for the same reason: a walk
/// over a server's answers is a walk over data somebody else controls, and a
/// generated tree can be endless.
const MAX_DIRECTORIES: usize = 100_000;

/// Run one async backend call from a thread that is not the runtime's.
///
/// **Spawn-and-wait rather than [`tokio::runtime::Handle::block_on`]**, and the
/// difference matters twice. The engine calls the seam from worker threads it
/// spawned itself, which have no runtime context at all; and the runtime this
/// app is tested under is often a current-thread one, where a second
/// `block_on` from another thread would sit behind the first forever. Pushing
/// the future onto the runtime and blocking on a channel works from any thread
/// and on either flavour of runtime, because the thread that is waiting is
/// never the thread that has to make progress.
fn block_on<T: Send + 'static>(
    runtime: &tokio::runtime::Handle,
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> std::result::Result<T, EndpointError> {
    let (done, wait) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = done.send(work.await);
    });
    wait.recv().map_err(|_| {
        EndpointError::new(
            EndpointErrorKind::Io,
            String::new(),
            "the transfer's runtime stopped before the share answered",
        )
    })
}

/// Put an engine failure on the seam without losing the backend's own words.
///
/// The kind is what the engine branches on; the sentence is what a person
/// reads, and it is already written — every backend in this tree phrases its
/// own errors, and reconstructing them here would silently rewrite messages
/// users have been shown for four epics.
fn refuse(path: &str, operation: &str, error: &EngineError) -> EndpointError {
    let kind = match error {
        EngineError::NotFound(_) => EndpointErrorKind::NotFound,
        EngineError::PermissionDenied(_) | EngineError::ReadOnly(_) => {
            EndpointErrorKind::PermissionDenied
        }
        _ => EndpointErrorKind::Io,
    };
    EndpointError::new(kind, path, format!("cannot {operation} '{path}': {error}"))
}

fn kind_of(kind: EntryKind) -> xsync_endpoint::EntryKind {
    match kind {
        EntryKind::File => xsync_endpoint::EntryKind::File,
        EntryKind::Dir => xsync_endpoint::EntryKind::Directory,
        EntryKind::Symlink => xsync_endpoint::EntryKind::Symlink,
        EntryKind::Other => xsync_endpoint::EntryKind::Other,
    }
}

/// The parent of a `/`-separated share path, or `""` when it has none.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// One share path, rooted, as both ends of the seam need to see it.
///
/// Held by [`ShareSource`] and [`ShareSink`] rather than duplicated: the
/// walking, the path joining and the `Attrs` → [`FileEntry`] translation are
/// the same job whichever direction the bytes are going.
struct Share {
    backend: Arc<dyn ShareBackend>,
    /// Share-relative root with no trailing slash. Empty is the share itself.
    root: String,
    runtime: tokio::runtime::Handle,
}

impl Share {
    fn new(backend: &Arc<dyn ShareBackend>, root: &str) -> Self {
        Self {
            backend: Arc::clone(backend),
            root: root.trim_end_matches('/').to_owned(),
            runtime: tokio::runtime::Handle::current(),
        }
    }

    /// The share path one root-relative wire path names.
    fn join(&self, path: &WirePath) -> String {
        let relative = String::from_utf8_lossy(path.as_bytes()).into_owned();
        if relative.is_empty() {
            return self.root.clone();
        }
        if self.root.is_empty() {
            return relative;
        }
        format!("{}/{relative}", self.root)
    }

    /// Metadata for one share path, or `None` when it is not there.
    fn stat(&self, path: &str) -> std::result::Result<Option<Attrs>, EndpointError> {
        let backend = Arc::clone(&self.backend);
        let owned = path.to_owned();
        match block_on(&self.runtime, async move { backend.stat(&owned).await })? {
            Ok(attrs) => Ok(Some(attrs)),
            Err(EngineError::NotFound(_)) => Ok(None),
            Err(error) => Err(refuse(path, "stat", &error)),
        }
    }

    /// Turn one path's attributes into the entry the engine plans against.
    ///
    /// `change_cookie` becomes the fingerprint's **validator** where a protocol
    /// has one, which today is xsync alone. That slot is documented as the
    /// place a non-filesystem source puts a strong version token, and it is
    /// what lets the resume journal reuse a stage across runs: without it the
    /// journal refuses to trust that the far end still holds the bytes it
    /// described, so SMB, NFS and SFTP resume within a run but re-send a
    /// partial file after a restart. That is the honest answer for a protocol
    /// whose only validator is size and mtime.
    fn entry(path: WirePath, attrs: &Attrs) -> FileEntry {
        let kind = kind_of(attrs.kind);
        let mtime = attrs.modified_ms.map_or(SystemTime::UNIX_EPOCH, |ms| {
            let ms = u64::try_from(ms).unwrap_or(0);
            SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
        });
        let mut fingerprint = SourceFingerprint::synthetic(kind, attrs.size, mtime);
        if let Some(cookie) = &attrs.change_cookie {
            fingerprint = fingerprint.with_validator(cookie);
        }
        FileEntry {
            path,
            kind,
            size: attrs.size,
            mtime,
            // Permission bits only. `Attrs::mode` is the whole `st_mode`,
            // file-type bits and all, and xsync's is masked — comparing the
            // two classifies every file as having drifted, on every run,
            // forever. The planner then puts it in the metadata-repair plan
            // rather than in `unchanged`, so a copy that had nothing to do
            // reported neither a transfer nor a skip.
            //
            // A protocol that does not report mode at all gets the ordinary
            // default for its kind rather than zero: zero would publish an
            // unreadable file on any sink that honours the bits.
            mode: attrs.mode.map_or(
                if attrs.kind == EntryKind::Dir {
                    0o755
                } else {
                    0o644
                },
                |mode| mode & 0o7777,
            ),
            fingerprint,
        }
    }

    fn entry_at(&self, path: &WirePath) -> std::result::Result<Option<FileEntry>, EndpointError> {
        let full = self.join(path);
        Ok(self
            .stat(&full)?
            .map(|attrs| Self::entry(path.clone(), &attrs)))
    }

    /// Every object below the root, with root-relative paths.
    ///
    /// A root that is not there enumerates as empty. On the sink that is the
    /// ordinary first transfer; on the source the planner reports the missing
    /// root itself, which is a better message than one this walk could write.
    fn walk(&self) -> std::result::Result<Enumeration, EndpointError> {
        let Some(root_attrs) = self.stat(&self.root)? else {
            return Ok(Enumeration::default());
        };
        if root_attrs.kind != EntryKind::Dir {
            // A file root is one entry named by its own basename, which is what
            // `Only` narrows a single-file transfer to.
            let name = WirePath::from_wire(basename(&self.root).as_bytes().to_vec())
                .map_err(|error| refuse(&self.root, "name", &EngineError::Io(error.to_string())))?;
            return Ok(Enumeration {
                entries: vec![Self::entry(name, &root_attrs)],
                queue_high_water: 0,
            });
        }

        let mut entries = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut pending = vec![WirePath::default()];
        visited.insert(self.root.clone());
        let mut directories = 0_usize;

        while let Some(relative) = pending.pop() {
            directories += 1;
            if directories > MAX_DIRECTORIES {
                return Err(EndpointError::new(
                    EndpointErrorKind::Io,
                    self.root.clone(),
                    format!("gave up after {MAX_DIRECTORIES} directories"),
                ));
            }
            let full = self.join(&relative);
            let backend = Arc::clone(&self.backend);
            let owned = full.clone();
            let listing = block_on(&self.runtime, async move { backend.list_dir(&owned).await })?
                .map_err(|error| refuse(&full, "list", &error))?;
            for row in listing.entries {
                let Ok(child) = WirePath::from_wire(row.name.as_bytes().to_vec()) else {
                    // A name that cannot be a relative path component is a
                    // server describing a write outside the root. Refused
                    // rather than sanitised: `plan.rs` makes the same call.
                    return Err(EndpointError::new(
                        EndpointErrorKind::InvalidPath,
                        row.path.clone(),
                        format!("{:?} is not a name this transfer can use", row.name),
                    ));
                };
                let child = child.with_prefix(&relative);
                if row.attrs.kind == EntryKind::Dir {
                    // A listing that names a directory already walked is a
                    // loop, and following it would copy a share into itself.
                    if !visited.insert(row.path.clone()) {
                        return Err(EndpointError::new(
                            EndpointErrorKind::Io,
                            row.path,
                            "already visited: the listing describes a loop",
                        ));
                    }
                    pending.push(child.clone());
                }
                entries.push(Self::entry(child, &row.attrs));
            }
        }
        Ok(Enumeration {
            entries,
            queue_high_water: 0,
        })
    }

    /// Create one share directory and every parent it needs.
    fn make_directory(&self, full: &str) -> std::result::Result<(), EndpointError> {
        let mut built = String::new();
        for part in full.split('/').filter(|part| !part.is_empty()) {
            if !built.is_empty() {
                built.push('/');
            }
            built.push_str(part);
            if self
                .stat(&built)?
                .is_some_and(|attrs| attrs.kind == EntryKind::Dir)
            {
                continue;
            }
            let backend = Arc::clone(&self.backend);
            let owned = built.clone();
            match block_on(&self.runtime, async move { backend.mkdir(&owned).await })? {
                // `AlreadyExists` is another worker getting there first. Two
                // transfers into one folder race here by construction, and
                // losing that race is success.
                Ok(()) | Err(EngineError::AlreadyExists(_)) => {}
                Err(error) => return Err(refuse(&built, "create directory", &error)),
            }
        }
        Ok(())
    }

    /// Read a whole share file through one handle, hashing as it goes.
    fn read_whole(
        &self,
        full: &str,
    ) -> std::result::Result<(Vec<u8>, blake3::Hash), EndpointError> {
        let backend = Arc::clone(&self.backend);
        let owned = full.to_owned();
        let handle = block_on(&self.runtime, async move {
            crate::backend::read_handle(&backend, &owned).await
        })?
        .map_err(|error| refuse(full, "open", &error))?;

        let chunk = handle.max_read().max(1);
        let mut bytes = Vec::new();
        let mut hasher = blake3::Hasher::new();
        let mut offset = 0_u64;
        loop {
            let reader = Arc::clone(&handle);
            let read = block_on(
                &self.runtime,
                async move { reader.read_at(offset, chunk).await },
            )?
            .map_err(|error| refuse(full, "read", &error));
            let read = match read {
                Ok(read) => read,
                Err(error) => {
                    let closing = Arc::clone(&handle);
                    let _ = block_on(&self.runtime, async move { closing.close().await });
                    return Err(error);
                }
            };
            if read.is_empty() {
                break;
            }
            hasher.update(&read);
            offset += read.len() as u64;
            let short = read.len() < chunk as usize;
            bytes.extend_from_slice(&read);
            if short {
                break;
            }
        }
        let closing = Arc::clone(&handle);
        block_on(&self.runtime, async move { closing.close().await })?
            .map_err(|error| refuse(full, "close", &error))?;
        Ok((bytes, hasher.finalize()))
    }

    fn read_at(
        &self,
        full: &str,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, EndpointError> {
        let backend = Arc::clone(&self.backend);
        let owned = full.to_owned();
        let length = u32::try_from(length).unwrap_or(u32::MAX);
        block_on(&self.runtime, async move {
            backend.read_range(&owned, offset, length).await
        })?
        .map_err(|error| refuse(full, "read", &error))
    }

    /// Write `bytes` at `offset`, splitting what the protocol would not take.
    fn write_at(
        &self,
        full: &str,
        offset: u64,
        bytes: &[u8],
        truncate: bool,
    ) -> std::result::Result<(), EndpointError> {
        let mut at = offset;
        let mut first = truncate;
        // An empty write still has to happen once: it is how a stage that does
        // not exist yet is created.
        let mut chunks = bytes.chunks(WRITE_CHUNK).peekable();
        if chunks.peek().is_none() {
            let backend = Arc::clone(&self.backend);
            let owned = full.to_owned();
            let mode = if first {
                WriteMode::Create
            } else {
                WriteMode::Resume { offset: at }
            };
            block_on(&self.runtime, async move {
                backend.write_range(&owned, mode, &[]).await
            })?
            .map_err(|error| refuse(full, "write", &error))?;
            return Ok(());
        }
        for chunk in chunks {
            let backend = Arc::clone(&self.backend);
            let owned = full.to_owned();
            let mode = if first {
                WriteMode::Create
            } else {
                WriteMode::Resume { offset: at }
            };
            let payload = chunk.to_vec();
            block_on(&self.runtime, async move {
                backend.write_range(&owned, mode, &payload).await
            })?
            .map_err(|error| refuse(full, "write", &error))?;
            at += chunk.len() as u64;
            first = false;
        }
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> std::result::Result<(), EndpointError> {
        let backend = Arc::clone(&self.backend);
        let (owned_from, owned_to) = (from.to_owned(), to.to_owned());
        block_on(&self.runtime, async move {
            backend.rename(&owned_from, &owned_to, true).await
        })?
        .map_err(|error| refuse(from, "publish", &error))
    }

    fn remove(&self, full: &str, recursive: bool) -> std::result::Result<(), EndpointError> {
        let backend = Arc::clone(&self.backend);
        let owned = full.to_owned();
        match block_on(&self.runtime, async move {
            backend.remove(&owned, recursive).await
        })? {
            // Already gone is the outcome the caller asked for.
            Ok(()) | Err(EngineError::NotFound(_)) => Ok(()),
            Err(error) => Err(refuse(full, "remove", &error)),
        }
    }
}

/// A share read as one side of a transfer.
pub struct ShareSource {
    share: Share,
    /// Whether the root is the directory being copied as itself.
    transfer_root: bool,
    /// How many ranged reads to keep in flight against one file.
    concurrency: usize,
}

impl ShareSource {
    /// Read `root` on `backend`, as the transfer root when `transfer_root`.
    ///
    /// `concurrency` is E13-S7's read depth handed straight through: one chunk
    /// at a time makes a transfer round-trip bound, and depth is the only thing
    /// that fixes it. The engine may clamp it; it is a request.
    #[must_use]
    pub fn new(
        backend: &Arc<dyn ShareBackend>,
        root: &str,
        transfer_root: bool,
        concurrency: usize,
    ) -> Self {
        // A source with no random access cannot serve disjoint ranges
        // independently, and answering more than one there is a promise this
        // backend has not made.
        let concurrency = if backend.caps().random_access {
            concurrency.max(1)
        } else {
            1
        };
        Self {
            share: Share::new(backend, root),
            transfer_root,
            concurrency,
        }
    }
}

/// One share file, opened once for a transfer that reads all of it.
struct ShareReads {
    handle: Arc<dyn crate::backend::ReadHandle>,
    runtime: tokio::runtime::Handle,
    path: String,
}

impl xsync_endpoint::ReadHandle for ShareReads {
    fn read_at(&self, offset: u64, length: u64) -> std::result::Result<Vec<u8>, EndpointError> {
        let handle = Arc::clone(&self.handle);
        let length = u32::try_from(length)
            .unwrap_or(u32::MAX)
            .min(handle.max_read());
        block_on(&self.runtime, async move {
            handle.read_at(offset, length).await
        })?
        .map_err(|error| refuse(&self.path, "read", &error))
    }

    fn max_read(&self) -> u64 {
        u64::from(self.handle.max_read())
    }

    fn close(&self) -> std::result::Result<(), EndpointError> {
        let handle = Arc::clone(&self.handle);
        block_on(&self.runtime, async move { handle.close().await })?
            .map_err(|error| refuse(&self.path, "close", &error))
    }
}

impl Source for ShareSource {
    fn enumerate(&self) -> std::result::Result<Enumeration, EndpointError> {
        self.share.walk()
    }

    fn stat(&self, path: &WirePath) -> std::result::Result<Option<FileEntry>, EndpointError> {
        self.share.entry_at(path)
    }

    fn root_entry(&self) -> std::result::Result<Option<FileEntry>, EndpointError> {
        if !self.transfer_root {
            return Ok(None);
        }
        self.share.entry_at(&WirePath::default())
    }

    fn read_file(
        &self,
        entry: &FileEntry,
    ) -> std::result::Result<xsync_endpoint::StableRead, EndpointError> {
        let full = self.share.join(&entry.path);
        let (bytes, digest) = self.share.read_whole(&full)?;
        // The trait's contract: a file replaced underneath the read must be
        // reported, not mixed into the destination copy. There is no ranged
        // protocol here that can pin a version, so the check is the same one
        // `Attrs::unchanged_from` makes everywhere else in this tree — the
        // change cookie where there is one, size and mtime where there is not.
        let after = self.share.entry_at(&entry.path)?;
        let stable = after.as_ref().is_some_and(|after| {
            after.size == entry.size
                && after.mtime == entry.mtime
                && after.fingerprint.validator == entry.fingerprint.validator
        });
        if !stable {
            return Err(EndpointError::new(
                EndpointErrorKind::Unstable,
                full.clone(),
                format!("source file '{full}' changed during read after 1 attempts"),
            ));
        }
        Ok(xsync_endpoint::StableRead {
            entry: after.unwrap_or_else(|| entry.clone()),
            bytes,
            blake3: digest,
            attempts: 1,
        })
    }

    fn read_range(
        &self,
        entry: &FileEntry,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, EndpointError> {
        self.share
            .read_at(&self.share.join(&entry.path), offset, length)
    }

    fn open<'a>(
        &'a self,
        entry: &FileEntry,
    ) -> std::result::Result<Arc<dyn xsync_endpoint::ReadHandle + 'a>, EndpointError> {
        // The backend's own handle where it has one (E13-S7): xsync pays three
        // round trips per megabyte without it. `read_handle` falls back to
        // wrapping `read_range` for the backends that have nothing better, so
        // this is one shape either way.
        let full = self.share.join(&entry.path);
        let backend = Arc::clone(&self.share.backend);
        let owned = full.clone();
        let handle = block_on(&self.share.runtime, async move {
            crate::backend::read_handle(&backend, &owned).await
        })?
        .map_err(|error| refuse(&full, "open", &error))?;
        Ok(Arc::new(ShareReads {
            handle,
            runtime: self.share.runtime.clone(),
            path: full,
        }))
    }

    fn read_symlink(&self, entry: &FileEntry) -> std::result::Result<SymlinkTarget, EndpointError> {
        let full = self.share.join(&entry.path);
        let attrs = self.share.stat(&full)?.ok_or_else(|| {
            EndpointError::new(EndpointErrorKind::NotFound, full.clone(), "link is gone")
        })?;
        let target = attrs.link_target.ok_or_else(|| {
            EndpointError::new(
                EndpointErrorKind::WrongKind,
                full.clone(),
                format!("'{full}' is not a symlink"),
            )
        })?;
        Ok(SymlinkTarget {
            target: std::path::PathBuf::from(target),
            // Nothing in `Attrs` says what the target *is*, and following it to
            // find out would be a round trip that could resolve outside the
            // share. Only Windows needs the answer, which this does not ship to.
            kind: SymlinkTargetKind::File,
        })
    }

    fn read_concurrency(&self) -> usize {
        self.concurrency
    }
}

/// A share written as one side of a transfer.
///
/// Every publication is a temp file in the destination's own directory,
/// promoted by [`ShareBackend::rename`]. Same directory, so the rename cannot
/// cross a filesystem; hidden by a leading dot, so a listing taken mid-transfer
/// does not show scaffolding; named from a digest of the destination path, so
/// the next run finds the same stage and resumes into it.
pub struct ShareSink {
    share: Share,
}

impl ShareSink {
    /// Write into `root` on `backend`.
    #[must_use]
    pub fn new(backend: &Arc<dyn ShareBackend>, root: &str) -> Self {
        Self {
            share: Share::new(backend, root),
        }
    }

    fn stage_of(&self, path: &WirePath) -> String {
        let full = self.share.join(path);
        let digest = blake3::hash(path.as_bytes()).to_hex();
        let name = format!(".excalibur-stage-{}", &digest[..16]);
        let parent = parent_of(&full);
        if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        }
    }

    fn require_file(entry: &FileEntry, path: &str) -> std::result::Result<(), EndpointError> {
        if entry.kind == xsync_endpoint::EntryKind::File {
            return Ok(());
        }
        Err(EndpointError::new(
            EndpointErrorKind::WrongKind,
            path.to_owned(),
            format!("'{path}' is not a regular file"),
        ))
    }

    fn verify(
        path: &str,
        expected: &blake3::Hash,
        bytes: &[u8],
    ) -> std::result::Result<(), EndpointError> {
        if blake3::hash(bytes) == *expected {
            return Ok(());
        }
        Err(EndpointError::new(
            EndpointErrorKind::Verification,
            path.to_owned(),
            format!("'{path}' did not match the digest the source computed"),
        ))
    }
}

impl xsync_endpoint::Sink for ShareSink {
    fn prepare_root(&self) -> std::result::Result<(), EndpointError> {
        // The seam rule, applied where the write is about to happen: one
        // derived value gates every write, and it comes from the protocol
        // rather than from what the user asked for at connect time.
        let info = self.share.backend.mount_info();
        if !info.writable {
            return Err(EndpointError::new(
                EndpointErrorKind::PermissionDenied,
                self.share.root.clone(),
                format!("this share is read-only: {}", info.reason),
            ));
        }
        if self.share.root.is_empty() {
            return Ok(());
        }
        self.share.make_directory(&self.share.root)
    }

    fn enumerate(&self) -> std::result::Result<Enumeration, EndpointError> {
        self.share.walk()
    }

    fn stat(&self, path: &WirePath) -> std::result::Result<Option<FileEntry>, EndpointError> {
        self.share.entry_at(path)
    }

    fn read_file(&self, path: &WirePath) -> std::result::Result<Vec<u8>, EndpointError> {
        let full = self.share.join(path);
        self.share.read_whole(&full).map(|(bytes, _)| bytes)
    }

    fn create_directory(&self, entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        if entry.kind != xsync_endpoint::EntryKind::Directory {
            return Err(EndpointError::new(
                EndpointErrorKind::WrongKind,
                full.clone(),
                format!("'{full}' is not a directory"),
            ));
        }
        self.share.make_directory(&full)
    }

    /// A no-op, and deliberately.
    ///
    /// [`ShareBackend`] has no `chmod` and no `utimes`: SMB, `NFSv3`, SFTP and
    /// xsync each have a way to set them and the trait has never needed one, so
    /// inventing four now would be a much larger change than this story. Failing
    /// the transfer instead would mean a copy that landed every byte correctly
    /// is reported as failed because a permission bit could not be restored,
    /// which is the wrong trade. What is lost is stated rather than hidden: a
    /// file copied *onto* a share arrives with whatever mode that server gives a
    /// new file. Copies *off* a share, where the sink is the local one, keep
    /// their modes as they always did.
    fn finish_directory(&self, _entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        Ok(())
    }

    fn finish_root(&self, _entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        Ok(())
    }

    fn set_mode(&self, _path: &WirePath, _mode: u32) -> std::result::Result<(), EndpointError> {
        Ok(())
    }

    /// Nothing was repaired, and the count says so.
    ///
    /// The default loops [`Self::set_mode`] and counts what it called. With a
    /// `set_mode` that does nothing that would report repairs this sink did not
    /// make, and the number reaches the run's report.
    fn repair_metadata(&self, _entries: &[FileEntry]) -> std::result::Result<usize, EndpointError> {
        Ok(0)
    }

    fn write_file(
        &self,
        entry: &FileEntry,
        digest: &blake3::Hash,
        bytes: &[u8],
    ) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        Self::require_file(entry, &full)?;
        Self::verify(&full, digest, bytes)?;
        // Staged even though the whole file is in hand: a `write_range` loop
        // straight onto the destination would leave a short file under the
        // final name if the connection dropped halfway, which is the one thing
        // this protocol exists to prevent.
        let stage = self.stage_of(&entry.path);
        self.share.write_at(&stage, 0, bytes, true)?;
        match self.share.rename(&stage, &full) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.share.remove(&stage, false);
                Err(error)
            }
        }
    }

    fn stage_open(&self, entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        Self::require_file(entry, &full)?;
        let stage = self.stage_of(&entry.path);
        match self.share.stat(&stage)? {
            // Kept, which is the whole point: the ranges written before a crash
            // survive into the next run and the journal decides which of them
            // it still trusts.
            Some(attrs) if attrs.kind == EntryKind::File && attrs.size <= entry.size => Ok(()),
            // Longer than the file it claims to be, or not a file at all: it
            // belongs to something else and committing it would publish bytes
            // past the declared size.
            Some(_) => {
                self.share.remove(&stage, true)?;
                self.share.write_at(&stage, 0, &[], true)
            }
            None => self.share.write_at(&stage, 0, &[], true),
        }
    }

    fn stage_write(
        &self,
        entry: &FileEntry,
        offset: u64,
        digest: &blake3::Hash,
        bytes: &[u8],
    ) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        Self::require_file(entry, &full)?;
        if offset.saturating_add(bytes.len() as u64) > entry.size {
            return Err(EndpointError::new(
                EndpointErrorKind::InvalidRange,
                full.clone(),
                format!("a range past the end of '{full}' was refused"),
            ));
        }
        Self::verify(&full, digest, bytes)?;
        // Never `Create`, whatever the offset: truncating at zero would discard
        // ranges a previous run had already landed, which is exactly what the
        // stage exists to keep.
        self.share
            .write_at(&self.stage_of(&entry.path), offset, bytes, false)
    }

    /// A no-op, because every range is already as durable as this can make it.
    ///
    /// Each `write_range` in this tree opens, writes, flushes and closes — that
    /// is the shape [`ShareBackend`] settled on — so a range that returned has
    /// been acknowledged by the server. There is no `fsync` verb behind it, and
    /// there is no honest way to invent one: what a remote filesystem has done
    /// with an acknowledged write is the server's business.
    fn stage_sync(&self, _entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        Ok(())
    }

    fn stage_commit(&self, entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        Self::require_file(entry, &full)?;
        let stage = self.stage_of(&entry.path);
        let staged = self.share.stat(&stage)?.ok_or_else(|| {
            EndpointError::new(
                EndpointErrorKind::NotFound,
                stage.clone(),
                format!("the stage for '{full}' is gone"),
            )
        })?;
        if staged.size != entry.size {
            return Err(EndpointError::new(
                EndpointErrorKind::Io,
                full.clone(),
                format!(
                    "the stage for '{full}' holds {} bytes of {}",
                    staged.size, entry.size
                ),
            ));
        }
        self.share.rename(&stage, &full)
    }

    fn stage_discard(&self, entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        self.share.remove(&self.stage_of(&entry.path), false)
    }

    fn create_symlink(
        &self,
        entry: &FileEntry,
        _target: &SymlinkTarget,
    ) -> std::result::Result<(), EndpointError> {
        // `ShareBackend` reports symlinks and does not create them. Saying so
        // is what turns a symlink into one reported entry rather than a
        // silently missing one.
        let full = self.share.join(&entry.path);
        Err(EndpointError::new(
            EndpointErrorKind::Unsupported,
            full.clone(),
            format!("this share cannot be given a symlink at '{full}'"),
        ))
    }

    fn remove(&self, entry: &FileEntry) -> std::result::Result<(), EndpointError> {
        let full = self.share.join(&entry.path);
        self.share
            .remove(&full, entry.kind == xsync_endpoint::EntryKind::Directory)
    }

    fn native_path(
        &self,
        _path: &WirePath,
    ) -> std::result::Result<std::path::PathBuf, EndpointError> {
        // Even the local backend answers `Unsupported` here, and on purpose:
        // this is the `std::fs` escape hatch the engine uses for reflinks and
        // the hash cache, and a backend reached through the trait must not have
        // its confinement rules bypassed by handing out a real path.
        Err(EndpointError::unsupported("native paths"))
    }
}

/// Any source narrowed to one named entry, published under one name.
///
/// A single-file transfer is not a transfer of its parent directory, and both
/// sides need that said. xsync's own layout roots a file source at the parent
/// and relies on the run's filter to narrow it; a file manager has no filter to
/// build, because a filename a server chose is not a glob it can escape.
///
/// So the narrowing happens here instead, and it is also where the destination
/// name is decided — which is what retires E22-S1's hard-link staging for "Keep
/// both". `sole_destination_name` is the other half: without it the planner
/// judges the whole destination directory against a one-file source, and a
/// stranger's files there are classified as extraneous.
struct Only<S> {
    inner: S,
    /// The one source-relative name this transfer moves.
    name: String,
    /// The name it lands under.
    destination: String,
}

impl<S: Source> Only<S> {
    fn wire(&self) -> std::result::Result<WirePath, EndpointError> {
        WirePath::from_wire(self.name.as_bytes().to_vec()).map_err(|error| {
            EndpointError::new(
                EndpointErrorKind::InvalidPath,
                self.name.clone(),
                error.to_string(),
            )
        })
    }
}

impl<S: Source> Source for Only<S> {
    fn enumerate(&self) -> std::result::Result<Enumeration, EndpointError> {
        let path = self.wire()?;
        let entry = self.inner.stat(&path)?.ok_or_else(|| {
            EndpointError::new(
                EndpointErrorKind::NotFound,
                self.name.clone(),
                format!(
                    "cannot stat source '{}': no such file or directory",
                    self.name
                ),
            )
        })?;
        Ok(Enumeration {
            entries: vec![entry],
            queue_high_water: 0,
        })
    }

    fn stat(&self, path: &WirePath) -> std::result::Result<Option<FileEntry>, EndpointError> {
        self.inner.stat(path)
    }

    /// `None`: a file source has no transfer root whose mode to restore.
    fn root_entry(&self) -> std::result::Result<Option<FileEntry>, EndpointError> {
        Ok(None)
    }

    fn read_file(
        &self,
        entry: &FileEntry,
    ) -> std::result::Result<xsync_endpoint::StableRead, EndpointError> {
        self.inner.read_file(entry)
    }

    fn read_range(
        &self,
        entry: &FileEntry,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, EndpointError> {
        self.inner.read_range(entry, offset, length)
    }

    fn open<'a>(
        &'a self,
        entry: &FileEntry,
    ) -> std::result::Result<Arc<dyn xsync_endpoint::ReadHandle + 'a>, EndpointError> {
        self.inner.open(entry)
    }

    fn open_region<'a>(
        &'a self,
        entry: &FileEntry,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Arc<dyn xsync_endpoint::ReadHandle + 'a>, EndpointError> {
        self.inner.open_region(entry, offset, length)
    }

    fn read_symlink(&self, entry: &FileEntry) -> std::result::Result<SymlinkTarget, EndpointError> {
        self.inner.read_symlink(entry)
    }

    fn destination_path(&self, source: &WirePath) -> WirePath {
        if source == self.name.as_str() {
            return WirePath::from(self.destination.as_str());
        }
        self.inner.destination_path(source)
    }

    fn sole_destination_name(&self) -> Option<WirePath> {
        Some(WirePath::from(self.destination.as_str()))
    }

    fn is_cloud_placeholder(&self, entry: &FileEntry) -> std::result::Result<bool, EndpointError> {
        self.inner.is_cloud_placeholder(entry)
    }

    fn read_concurrency(&self) -> usize {
        self.inner.read_concurrency()
    }

    /// Every capability below is the inner source's answer, unchanged.
    ///
    /// Narrowing a source to one entry says nothing about what it can do, and
    /// answering `false` here would quietly cost a local file its reflink.
    fn supports_clone(&self) -> bool {
        self.inner.supports_clone()
    }

    fn supports_sparse_probe(&self) -> bool {
        self.inner.supports_sparse_probe()
    }

    fn supports_cloud_placeholders(&self) -> bool {
        self.inner.supports_cloud_placeholders()
    }

    fn supports_hash_cache(&self) -> bool {
        self.inner.supports_hash_cache()
    }

    fn local_root(&self) -> Option<&std::path::Path> {
        self.inner.local_root()
    }
}

/// The options every endpoint transfer runs under.
///
/// `delete` and `max_delete` are E22-S1's, for E22-S1's reason: a copy is not a
/// sync, and a transfer must not remove destination files nobody asked about.
/// `checksum` and `paranoid` are off, and E22-S1's note on
/// [`super::xsync_engine`] explains both — neither is what makes a transfer
/// verified, and there is nowhere for a preference to live until E22-S3.
///
/// `journal_root` is the one that needed a decision, and it is **`None` for a
/// share destination**, which switches the resume journal off there.
///
/// The engine asks a sink whether its stage survived, and answers *yes* for any
/// sink with no local path — "only it can know", says the code, and this one
/// cannot: the question is put before `stage_open` is called, so there is no
/// moment at which a share sink could say that the file it staged into is gone.
/// A journal record that outlived its stage would then make the engine skip
/// ranges the destination does not hold, and publish a file with holes in it
/// under the right name and the right length. Silent corruption is not a
/// trade worth a head start, and `StageJournal` opens no journal at all when
/// neither the sink nor this field names a directory.
///
/// So a **download** — where the sink is xsync's own local one, which really
/// can tell whether its stage survived — keeps the journal, and gets a
/// directory under the user's cache rather than the default
/// [`std::env::temp_dir`]: many systems clear `/tmp` at boot, and a transfer
/// resumed after a restart would find no record of what had landed. An
/// **upload** keeps its stage, so nothing appears under its final name until
/// every byte is there, but starts the file again rather than trusting a record
/// it cannot check.
fn options(
    cancel: CancelToken,
    journal: Option<std::path::PathBuf>,
    progress: ProgressWatch,
) -> LocalSyncOptions {
    LocalSyncOptions {
        checksum: false,
        delete: false,
        dry_run: false,
        max_delete: Some(0),
        paranoid: false,
        journal_root: journal,
        cancel,
        progress: Some(progress),
        ..LocalSyncOptions::default()
    }
}

/// Where a local sink's resume journal lives, or `None` to take xsync's default.
fn journal_root() -> Option<std::path::PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("excalibur").join("resume"))
}

/// Where the two ends of one transfer are rooted.
enum Ends {
    /// Share to this machine.
    Down {
        /// Share-relative root of the source.
        source_root: String,
        /// The one source name, when a file rather than a directory.
        only: Option<(String, String)>,
        /// Local root the sink writes into.
        destination: std::path::PathBuf,
        /// Ranged reads to keep in flight against one file (E13-S7).
        read_depth: usize,
    },
    /// This machine to the share.
    Up {
        /// Local root of the source.
        source_root: std::path::PathBuf,
        /// The one source name, when a file rather than a directory.
        only: Option<(String, String)>,
        /// Share-relative root the sink writes into.
        destination: String,
    },
}

impl Ends {
    /// Work out both roots for one transfer.
    ///
    /// The rule is the same on both sides and it is xsync's own: a **directory**
    /// is the transfer root itself, so its contents land in a root that already
    /// carries the wanted name and renaming it costs nothing. A **file** hangs
    /// off its parent, and the one name it moves — and the name it lands under
    /// — are what [`Only`] carries.
    async fn build(
        share: &Arc<dyn ShareBackend>,
        transfer: &Transfer,
        read_depth: usize,
    ) -> Result<Self> {
        match transfer.direction {
            Some(Direction::Down) => {
                let attrs = share.stat(&transfer.source_path).await?;
                let destination = std::path::PathBuf::from(&transfer.dest_path);
                if attrs.kind == EntryKind::Dir {
                    return Ok(Self::Down {
                        source_root: transfer.source_path.clone(),
                        only: None,
                        destination,
                        read_depth,
                    });
                }
                Ok(Self::Down {
                    source_root: parent_of(transfer.source_path.trim_end_matches('/')).to_owned(),
                    only: Some((
                        basename(&transfer.source_path).to_owned(),
                        basename(&transfer.dest_path).to_owned(),
                    )),
                    destination: destination
                        .parent()
                        .map_or_else(|| std::path::PathBuf::from("."), Into::into),
                    read_depth,
                })
            }
            Some(Direction::Up) => {
                let source = std::path::PathBuf::from(&transfer.source_path);
                let metadata = std::fs::symlink_metadata(&source).map_err(|error| {
                    EngineError::Io(format!("inspect {}: {error}", source.display()))
                })?;
                if metadata.is_dir() {
                    return Ok(Self::Up {
                        source_root: source,
                        only: None,
                        destination: transfer.dest_path.trim_end_matches('/').to_owned(),
                    });
                }
                let parent = source
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map_or_else(|| std::path::PathBuf::from("."), Into::into);
                Ok(Self::Up {
                    source_root: parent,
                    only: Some((
                        basename(&transfer.source_path).to_owned(),
                        basename(&transfer.dest_path).to_owned(),
                    )),
                    destination: parent_of(transfer.dest_path.trim_end_matches('/')).to_owned(),
                })
            }
            None => Err(EngineError::Io(
                "this transfer does not name a share direction".to_owned(),
            )),
        }
    }
}

/// Drive one session on a blocking thread, with both endpoints built for it.
///
/// Everything the engine touches lives inside this closure: it borrows the two
/// endpoints for the length of the run, and the `emit` closure it is given does
/// nothing but push, because the engine calls it on its own thread between
/// files and anything slow in there is time the transfer is not running.
fn spawn_session(
    share: Arc<dyn ShareBackend>,
    ends: Ends,
    options: LocalSyncOptions,
    events: tokio::sync::mpsc::UnboundedSender<xsync_core::local::LocalEvent>,
) -> tokio::task::JoinHandle<std::result::Result<LocalSyncReport, LocalSyncError>> {
    tokio::task::spawn_blocking(move || {
        let emit = |event| {
            // A closed receiver means the consumer is gone, which is not a
            // reason to stop moving bytes.
            let _ = events.send(event);
        };
        match ends {
            Ends::Down {
                source_root,
                only,
                destination,
                read_depth,
            } => {
                let source = ShareSource::new(&share, &source_root, only.is_none(), read_depth);
                let sink = LocalSink::new_without_creation(&destination);
                match only {
                    Some((name, into)) => sync_endpoints(
                        &Only {
                            inner: source,
                            name,
                            destination: into,
                        },
                        &sink,
                        &options,
                        emit,
                    ),
                    None => sync_endpoints(&source, &sink, &options, emit),
                }
            }
            Ends::Up {
                source_root,
                only,
                destination,
            } => {
                let sink = ShareSink::new(&share, &destination);
                let mut source = LocalSource::new(&source_root);
                if only.is_none() {
                    source = source.as_transfer_root();
                }
                match only {
                    Some((name, into)) => sync_endpoints(
                        &Only {
                            inner: source,
                            name,
                            destination: into,
                        },
                        &sink,
                        &options,
                        emit,
                    ),
                    None => sync_endpoints(&source, &sink, &options, emit),
                }
            }
        }
    })
}

/// Run one transfer through xsync's planner with `share` as one endpoint.
///
/// This is the route every pair E22-S1 does not take: NFS, SMB, SFTP, a local
/// folder browsed as a mount, and an xsync export whose own engine could not be
/// reached. A **directory is one row** and the planner does the walk, which is
/// what makes a folder dropped onto a share work at all — before this there was
/// no upload planner and the worker read the directory as a file.
///
/// `read_depth` is E13-S7's, handed to the source as its read concurrency.
/// `progress` is called with the bytes moved so far and the planned total once
/// the scan knows it; the caller throttles.
///
/// # Errors
///
/// The session's own failure, or a refusal before it starts. Per-entry failures
/// do **not** fail the call in xsync's model, but they do here: a row is one
/// transfer and reporting `Done` for a copy that could not publish a file would
/// be a success it did not have.
pub async fn run(
    share: &Arc<dyn ShareBackend>,
    transfer: &Transfer,
    read_depth: usize,
    stop: Arc<AtomicBool>,
    mut progress: impl FnMut(Moved) + Send + 'static,
) -> Result<Settled> {
    let ends = Ends::build(share, transfer, read_depth).await?;
    let share = Arc::clone(share);

    // The worker's flag is an `AtomicBool` with nobody to signal, and the
    // engine wants a token. One task turns the first into the second; unlike
    // E22-S1 nothing has to be killed, because the engine drains what is in
    // flight and leaves a resumable stage rather than a torn file.
    let cancel = CancelToken::new();
    let watching = cancel.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&finished);
    let flag = Arc::clone(&stop);
    let watcher = tokio::spawn(async move {
        while !done.load(Ordering::SeqCst) {
            if flag.load(Ordering::SeqCst) {
                watching.cancel();
                return;
            }
            tokio::time::sleep(CANCEL_POLL).await;
        }
    });

    // The engine calls `emit` on its own thread between files, so it does
    // nothing but push. Unbounded for E22-S1's reason: a bounded channel would
    // make a slow consumer into back-pressure on the transfer.
    let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
    let journal = match &ends {
        Ends::Down { .. } => journal_root(),
        Ends::Up { .. } => None,
    };
    let progress_watch = ProgressWatch::new();
    let session = spawn_session(
        share,
        ends,
        options(cancel, journal, progress_watch.clone()),
        events,
    );

    let mut tally = Tally::new();
    let mut sampler = Progress::new();
    let mut progress_ticks = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            event = incoming.recv() => {
                let Some(event) = event else { break };
                let snapshot = progress_watch.snapshot(false);
                if !tally.absorb(&event, Some(&snapshot)) {
                    continue;
                }
                let moved = tally.moved();
                if sampler.sample(moved.bytes, Instant::now()) {
                    progress(moved);
                }
            }
            _ = progress_ticks.tick() => {
                let snapshot = progress_watch.snapshot(false);
                if tally.refresh(&snapshot) {
                    progress(tally.moved());
                }
            }
        }
    }

    finished.store(true, Ordering::SeqCst);
    watcher.abort();

    let report = session
        .await
        .map_err(|error| EngineError::Io(format!("the transfer thread failed: {error}")))?
        .map_err(|error| EngineError::Io(error.to_string()))?;

    if stop.load(Ordering::SeqCst) || report.cancelled {
        // The user's decision arriving, not a failure. The row's status is
        // already theirs and the caller must not settle it.
        let moved = tally.moved();
        return Ok(Settled {
            bytes: moved.bytes,
            files: moved.files,
            verified: false,
            note: None,
            stopped: true,
        });
    }

    if report.partial_failure() || tally.failed() > 0 {
        let failed = report.failed_entries.max(tally.failed());
        return Err(EngineError::Io(format!(
            "{failed} of {} entries failed — {}",
            failed + report.transferred_files + report.skipped_files,
            tally.first_failure()
        )));
    }

    let settled = tally.settle(&report);
    progress(Moved {
        bytes: settled.bytes,
        files: settled.files,
        planned: tally.moved().planned,
        progress: tally.moved().progress,
    });
    Ok(settled)
}

#[cfg(test)]
mod tests {
    use super::{basename, parent_of};

    #[test]
    fn a_share_path_splits_into_a_parent_and_a_name() {
        assert_eq!(parent_of("films/2016/a.mkv"), "films/2016");
        assert_eq!(parent_of("a.mkv"), "");
        assert_eq!(basename("films/2016/a.mkv"), "a.mkv");
        // A directory row carries a trailing slash from the path field, and the
        // name of a directory is not the empty string.
        assert_eq!(basename("films/2016/"), "2016");
    }
}
