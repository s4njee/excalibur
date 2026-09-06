//! Local↔xsync transfers, driven by xsync's own engine (E22-S1).
//!
//! Everything else in this module moves bytes with [`super::worker`]'s
//! `download`/`upload`: a chunk loop over [`crate::backend::ShareBackend`].
//! When both ends of a transfer are a folder on this machine and an xsync
//! export, that loop is the wrong thing to run. xsync already has a planner, a
//! size-ladder scheduler, a persistent BLAKE3 hash cache, adaptive
//! compression, a durable resume journal and a batched small-file pass, and
//! all of it is faster than a chunk loop can be. So for that one pair the
//! bytes go through *its* engine and this file is the adapter.
//!
//! Three things shape the adapter, and each of them is a constraint from the
//! other side rather than a preference:
//!
//! - **The session is Excalibur's to spawn.** `run_client_push` and
//!   `run_client_pull` take a reader and a writer, not a host. Excalibur owns
//!   its `known_hosts`, its `StrictHostKeyChecking=yes` and its `BatchMode`,
//!   and it is not handing that decision to a library. So this spawns
//!   `ssh … xs --server <root>` through the *same* builder the browse session
//!   uses ([`crate::backend::xsync::ssh_argv`]) and hands over the pipes.
//!
//! - **A browse session is not a sync session.** One `xs --server` serves one
//!   session and the client's handshake role fixes which protocol it speaks —
//!   v3 filesystem for browsing, v1/v2 sync for a transfer. So a transfer
//!   opens a second `ssh` beside the mount's, and the mount is untouched.
//!
//! - **The engine is synchronous and its callback is not `Send`.** Both entry
//!   points block for the whole session and call `emit` on the calling thread
//!   between frames. Anything slow in there is time the transfer is not
//!   running, so the session runs on a blocking thread and `emit` does nothing
//!   but push onto a channel that a task on the runtime drains into the queue.
//!
//! What is deliberately *not* here: `delete`. This is a copy, not a sync, and
//! `LocalSyncOptions::delete` would let a transfer remove destination files
//! nobody asked about. It is set false and not exposed, per E22-S1.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use xsync_core::local::{LocalEvent, LocalSyncOptions, LocalSyncReport, TransferMethod};
use xsync_core::server::{run_client_pull, run_client_push};

use super::rate::Progress;
use super::{Direction, Transfer};
use crate::backend::xsync::{ssh_argv, SshTarget};
use crate::error::{EngineError, Result};

/// Bytes buffered in front of the pipe to the remote.
///
/// `xs` uses exactly this. The engine flushes only where the protocol requires
/// it and relies on the caller's buffer for everything else, so a default
/// 8 KiB writer turns a batch of a hundred thousand small files into a write
/// syscall per file.
const WRITE_BUFFER: usize = 1024 * 1024;

/// How often the cancel flag is looked at while a session runs.
///
/// Polled rather than signalled because [`super::worker::Worker::stop`] sets an
/// `AtomicBool` and has no channel to wake anyone with. A tenth of a second is
/// below the threshold where a person notices a button not responding, and it
/// costs one atomic load.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// How to reach one xsync export for a transfer.
///
/// Owned rather than borrowed like [`SshTarget`], because it crosses a task
/// boundary and outlives the lock the session manager handed it out under.
#[derive(Debug, Clone)]
pub struct XsyncRoute {
    /// Where the `xs --server` lives.
    pub remote: XsyncRemote,
    /// The export root, exactly as the remote spells it.
    ///
    /// A transfer's own `--server` root is a path *under* this one: the queue
    /// carries mount-relative paths, and the export root is what they are
    /// relative to.
    pub export_root: String,
    /// Whether this mount may be written.
    ///
    /// Excalibur's seam rule — one derived value gates every write — and here
    /// it is the *only* gate. `xs --server --read-only` sets `MountInfo.access`
    /// for the v3 filesystem session and nothing else; the v1/v2 sink never
    /// consults it, so a push to a read-only export still writes. That is
    /// recorded in `../xsync/docs/library.md` and it is why this check is not
    /// a belt-and-braces one.
    pub writable: bool,
    /// Why not, when `writable` is false. Shown verbatim on the failed row.
    pub reason: String,
}

/// Where an `xs --server` is, and how to start it.
#[derive(Debug, Clone)]
pub enum XsyncRemote {
    /// Over SSH, with the app's host-key policy.
    Ssh(SshEndpoint),
    /// `<program> --server <root>` on this machine.
    ///
    /// The same protocol over a pipe instead of a socket. It is how the route
    /// is exercised without a second machine, and it is a real deployment for
    /// an export on this host.
    Local {
        /// The `xs` binary.
        program: String,
    },
}

/// An owned [`SshTarget`].
#[derive(Debug, Clone)]
pub struct SshEndpoint {
    /// Host as the user typed it. Also the name the key is trusted under.
    pub host: String,
    /// Port `ssh` should connect to.
    pub port: u16,
    /// Remote username, or `None` for `ssh`'s own default.
    pub username: Option<String>,
    /// The app's `known_hosts`.
    pub known_hosts: PathBuf,
    /// An alternative to the `ssh` binary, for tests.
    pub rsh: Option<String>,
}

impl SshEndpoint {
    fn borrow(&self) -> SshTarget<'_> {
        SshTarget {
            host: &self.host,
            port: self.port,
            username: self.username.as_deref(),
            known_hosts: &self.known_hosts,
            rsh: self.rsh.as_deref(),
        }
    }
}

/// What a finished session leaves on the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    /// Bytes the session accounts for: published plus already-present.
    pub bytes: u64,
    /// Whether every published file went through a verified path.
    pub verified: bool,
    /// A short phrase for the row when there is more to say than a size.
    pub note: Option<String>,
    /// Set when the user stopped it; the caller must not settle the row.
    pub stopped: bool,
}

/// Where the two ends of a transfer are, once the route has been read.
struct Plan {
    /// The root the remote is given as `xs --server <root>`.
    remote_root: String,
    /// The local root the engine reads from or writes into.
    local_root: PathBuf,
    /// rsync's rule, applied to the side being read.
    trailing_slash: bool,
    /// A directory staged so the item lands under a name that is not its own.
    staging: Option<Staging>,
}

/// Join a mount-relative path onto an export root.
///
/// Rejects anything that could climb out of the export. The paths in the queue
/// come from listings this app made, so a `..` here is a bug or an attack and
/// neither should be resolved into a path outside the share the user mounted.
fn under(export_root: &str, relative: &str) -> Result<String> {
    let relative = relative.trim_matches('/');
    if relative
        .split('/')
        .any(|part| part == ".." || part == "." || part.is_empty() && !relative.is_empty())
    {
        return Err(EngineError::Io(format!(
            "{relative} is not a path inside this share"
        )));
    }
    let root = export_root.trim_end_matches('/');
    if relative.is_empty() {
        return Ok(root.to_owned());
    }
    Ok(format!("{root}/{relative}"))
}

/// The parent of a `/`-separated path, or the root itself when it has none.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(cut) => &path[..cut],
        None => "",
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A directory built so one item can be transferred under a different name.
///
/// The engine names what it publishes after the *source's* basename, and for a
/// single file nothing in `LocalSyncOptions` changes that: the wire path of a
/// one-file scan is the file's own name. A directory can be renamed for free —
/// its `--server` root *is* the name — but a file cannot, so "Keep both" on a
/// file is done here.
///
/// The trick is one directory containing one hard link under the wanted name.
/// A hard link because it costs nothing and shares the bytes; it is made beside
/// the source so it cannot cross a device. The directory is removed when the
/// session ends, and the link with it — the file it points at is untouched.
struct Staging {
    directory: PathBuf,
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Best effort. A staging directory that outlives its transfer is a
        // hard link and an empty directory, not lost data, and failing a
        // transfer that worked because its scaffolding could not be swept
        // would be the worse trade.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Staging {
    /// Link `source` into a fresh directory under `name`.
    fn link(source: &Path, name: &str, id: &str) -> Result<Self> {
        let parent = source.parent().unwrap_or(Path::new("."));
        let directory = parent.join(format!(".excalibur-{id}"));
        // A leftover from a killed run would make `create_dir` fail and would
        // hold a link to whatever that run was moving.
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory)
            .map_err(|error| EngineError::Io(format!("stage {}: {error}", directory.display())))?;
        let staged = Self { directory };
        let link = staged.directory.join(name);
        // A hard link is refused for a directory, across a device, and on a
        // filesystem that has none. Copying is the honest fallback: it costs
        // the bytes once more on this machine, which is still less than
        // sending the file under the wrong name would cost.
        if std::fs::hard_link(source, &link).is_err() {
            std::fs::copy(source, &link)
                .map_err(|error| EngineError::Io(format!("stage {}: {error}", link.display())))?;
        }
        Ok(staged)
    }
}

impl Plan {
    /// Work out the two roots for one transfer.
    ///
    /// `remote_is_dir` is what the share says about the source, and is only
    /// consulted on a pull — on a push the local filesystem is asked directly.
    ///
    /// The shape is symmetric, and it is the shape xsync's trailing-slash rule
    /// makes natural:
    ///
    /// - a **directory** is the `--server` root itself on the remote side and
    ///   the named directory on the local side, with the trailing slash on, so
    ///   its *contents* land in a root that already carries the wanted name.
    ///   Renaming a directory therefore costs nothing.
    /// - a **file** hangs off its parent: the container is the root and the
    ///   file's own basename is its wire path. Renaming a file needs
    ///   [`Staging`], because that basename is not ours to choose.
    fn build(route: &XsyncRoute, transfer: &Transfer, remote_is_dir: bool) -> Result<Self> {
        match transfer.direction {
            // Local → xsync. The remote path is the destination.
            Direction::Up => {
                let source = PathBuf::from(&transfer.source_path);
                let remote = under(&route.export_root, &transfer.dest_path)?;
                let is_dir = source.is_dir();
                if is_dir {
                    return Ok(Self {
                        remote_root: remote,
                        local_root: source,
                        trailing_slash: true,
                        staging: None,
                    });
                }
                let wanted = basename(&transfer.dest_path);
                let natural = source
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let staging = if wanted == natural {
                    None
                } else {
                    Some(Staging::link(&source, wanted, &transfer.id)?)
                };
                let local_root = match &staging {
                    // The staging directory's *contents* — one link, under the
                    // name the row promised — go into the destination folder.
                    Some(staged) => staged.directory.clone(),
                    None => source,
                };
                Ok(Self {
                    remote_root: parent_of(&remote).to_owned(),
                    local_root,
                    trailing_slash: staging.is_some(),
                    staging,
                })
            }
            // xsync → local. The remote path is the source.
            Direction::Down => {
                let remote = under(&route.export_root, &transfer.source_path)?;
                let destination = PathBuf::from(&transfer.dest_path);
                if remote_is_dir {
                    return Ok(Self {
                        remote_root: remote,
                        local_root: destination,
                        trailing_slash: true,
                        staging: None,
                    });
                }
                // A one-file scan on the far side already reports the file's
                // basename as its wire path, so the destination root is the
                // folder that will hold it. `src_trailing_slash` must be true:
                // false would prefix the basename again and publish
                // `x.txt/x.txt`.
                let parent = destination
                    .parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                Ok(Self {
                    remote_root: remote,
                    local_root: parent,
                    trailing_slash: true,
                    staging: None,
                })
            }
        }
    }
}

/// The options every Local↔xsync transfer runs under.
///
/// `delete: false` because a copy is not a sync. E22-S1 makes it permanent and
/// unexposed rather than a default somebody could flip, and `max_delete:
/// Some(0)` means a future change that turned `delete` on by accident would
/// still authorise no removals.
///
/// `paranoid` stays **off**. It is not the difference between verified and
/// unverified — every byte is digest-checked before publication without it.
/// What it adds is a whole-file digest compared end to end after the write,
/// which for a file over 8 MiB is the one guarantee the per-chunk check does
/// not give, and it costs a full extra read of every published file on each
/// side. That is a preference, not a default, and there is nowhere for it to
/// live until E22-S3.
///
/// `checksum` is the one that needs explaining, because E22-S1 asks for it on
/// and it is **off**.
///
/// It does not do what its name suggests on this route. It decides how a file
/// is *classified* — BLAKE3 content rather than size and mtime — so it makes
/// "this file is already there" a claim about the bytes. It verifies nothing:
/// checking what crossed the wire is unconditional and no option turns it off
/// (see [`Tally::settle`]), so the row's `verified` neither depends on it nor
/// is weakened by it being off. On a pull the field is not read at all — that
/// route always sends `checksum: true` so the remote source computes content
/// identity either way.
///
/// What it costs, on the xsync working tree this builds against today, is a
/// failed push. Turning it on makes `run_client_push` replace each scanned
/// entry's `fingerprint.identity` with a content identity
/// (`server.rs`, "Scan local source root"), and `LocalSource::read_range` then
/// compares a freshly taken *metadata* fingerprint against that mutated one
/// and refuses the read: `source file '…' changed during read after 1
/// attempts`. It fires for any file large enough to take the chunked path — a
/// 9 MiB file reproduces it every time, a 4 KiB one never does, because the
/// batched small-file pass does not read that way. The same flag on the *pull*
/// side is fine: the remote's source does not mutate the entry it reads with.
///
/// So this is off until that is fixed on the xsync side, and the loss is
/// stated rather than hidden: on a push, "unchanged" is a claim about size and
/// mtime, not about content. A file edited back to the same length within the
/// same mtime granularity would be skipped. Nothing about verification changes.
fn options() -> LocalSyncOptions {
    LocalSyncOptions {
        checksum: false,
        delete: false,
        dry_run: false,
        max_delete: Some(0),
        ..LocalSyncOptions::default()
    }
}

/// How many bytes may wait to reach the child before the engine is made to wait.
///
/// Bounded by **bytes and not by messages**, and that distinction is the whole
/// of it. A push flushes megabyte buffers and a pull writes acknowledgement
/// frames of a few dozen bytes; a queue of sixteen slots is 16 MiB of headroom
/// for one and 700 bytes for the other, so a message count generous enough for
/// a push starves a pull and one sized for a pull is unbounded for a push. That
/// was not a hypothesis — a 10,000-file *pull* deadlocked at the same 1,600
/// files a push did, with a sixteen-slot queue full of tiny acks.
///
/// Sixty-four mebibytes is far past anything the protocol leaves outstanding
/// and small enough that a peer which has genuinely stopped reading stalls this
/// transfer rather than the machine.
const PENDING_BYTES: usize = 64 * 1024 * 1024;

/// What the engine has written and the child has not yet taken.
#[derive(Default)]
struct Outgoing {
    chunks: std::collections::VecDeque<Vec<u8>>,
    bytes: usize,
    /// Set when the engine drops the writer: the pump drains and then closes.
    closed: bool,
    /// Set when the child's pipe failed, so the writer stops rather than
    /// filling a queue nobody is reading.
    broken: bool,
}

/// A writer that hands the child's pipe to a thread.
///
/// **This is what stops a transfer of many small files from deadlocking.** The
/// protocol acknowledges every frame, and a peer blocks once its own replies
/// fill the pipe back to us; an endpoint that is itself blocked writing never
/// reads them, and the two wait for each other forever. xsync sizes its
/// unacknowledged window — `MAX_PIPELINED_FRAMES`, ~84 KiB of pending replies —
/// to sit inside **OpenSSH's 2 MiB channel window**, and its own comment says
/// that margin "comes from the SSH transport rather than from a pipe". A pipe
/// holds 64 KiB, below the window, so the guarantee does not hold there.
///
/// It is not theoretical, and it is not one-directional. Pushing 10,000 files
/// into a local `xs --server` stops dead at about 1,600 files with every
/// process at 0% CPU; pulling the same tree stops at the same place, for the
/// mirrored reason — there the child is the one blocked writing data while we
/// are blocked writing acknowledgements it is not reading.
///
/// Decoupling the write direction removes the cycle. The engine's writes go
/// into a byte-bounded queue and return, so the engine always gets back to its
/// read loop and drains the peer; the pump thread is the one that blocks on the
/// full pipe, and it unblocks as soon as the peer reads again. Ordering is
/// preserved — one queue, one thread — so the peer sees exactly the byte stream
/// the engine wrote, a moment later.
///
/// Applied to the SSH route as well, not only the local one. OpenSSH's window
/// is an assumption about how the peer is configured rather than a guarantee,
/// and a memcpy per megabyte is not measurable beside BLAKE3 and zstd.
struct Detached {
    outgoing: Arc<(Mutex<Outgoing>, std::sync::Condvar, std::sync::Condvar)>,
    pump: Option<std::thread::JoinHandle<()>>,
}

impl Detached {
    fn new(stdin: std::process::ChildStdin) -> Self {
        let outgoing = Arc::new((
            Mutex::new(Outgoing::default()),
            // Signalled when the pump has taken bytes out.
            std::sync::Condvar::new(),
            // Signalled when the engine has put bytes in, or closed.
            std::sync::Condvar::new(),
        ));
        let theirs = Arc::clone(&outgoing);
        let pump = std::thread::spawn(move || {
            let mut stdin = stdin;
            let (queue, drained, filled) = &*theirs;
            loop {
                let chunk = {
                    let mut held = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    while held.chunks.is_empty() && !held.closed {
                        held = filled
                            .wait(held)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    match held.chunks.pop_front() {
                        Some(chunk) => {
                            held.bytes -= chunk.len();
                            drained.notify_all();
                            chunk
                        }
                        // Closed and drained: dropping `stdin` here is the EOF
                        // the remote waits for before it exits.
                        None => return,
                    }
                };
                if std::io::Write::write_all(&mut stdin, &chunk).is_err() {
                    let mut held = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    held.broken = true;
                    held.chunks.clear();
                    held.bytes = 0;
                    drained.notify_all();
                    return;
                }
            }
        });
        Self {
            outgoing,
            pump: Some(pump),
        }
    }
}

impl std::io::Write for Detached {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (queue, drained, filled) = &*self.outgoing;
        let mut held = queue.lock().unwrap_or_else(PoisonError::into_inner);
        // Never wait on an empty queue, however large the write: a single
        // buffer bigger than the budget must still get through.
        while held.bytes >= PENDING_BYTES && !held.broken && !held.chunks.is_empty() {
            held = drained
                .wait(held)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if held.broken {
            // The same failure an unbuffered write would have reported.
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the remote stopped reading",
            ));
        }
        held.bytes += bytes.len();
        held.chunks.push_back(bytes.to_vec());
        filled.notify_all();
        Ok(bytes.len())
    }

    /// A no-op, and deliberately.
    ///
    /// The engine flushes where the protocol requires a frame to be on the wire
    /// before it waits for the answer. Blocking here until the pump had drained
    /// would reintroduce exactly the wait this type exists to remove — and it
    /// is not needed, because the engine's next move after a flush is to read,
    /// and what it is reading cannot arrive until the peer has had the bytes.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Detached {
    fn drop(&mut self) {
        {
            let (queue, _, filled) = &*self.outgoing;
            let mut held = queue.lock().unwrap_or_else(PoisonError::into_inner);
            held.closed = true;
            filled.notify_all();
        }
        // Joined, so the last bytes are written and the child's stdin is closed
        // before the caller reaps it.
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

/// The child process for one session, and the pipes to it.
struct Remote {
    child: Arc<Mutex<Option<std::process::Child>>>,
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
}

impl Remote {
    /// Start one `xs --server <root>` and take its three pipes.
    fn spawn(route: &XsyncRoute, root: &str) -> Result<Self> {
        let mut command = match &route.remote {
            XsyncRemote::Ssh(endpoint) => {
                let (program, args) = ssh_argv(&endpoint.borrow(), root);
                let mut command = std::process::Command::new(program);
                command.args(args);
                command
            }
            XsyncRemote::Local { program } => {
                let mut command = std::process::Command::new(program);
                command.arg("--server").arg(root);
                command
            }
        };
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Piped, not inherited, and drained on a thread below: `ssh`'s own
            // account of a refused key is the only explanation there is for a
            // session that never started, and an unread stderr pipe can block
            // the child once it fills.
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| EngineError::Io(format!("could not start the transfer: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Io("the transfer's stdin was not piped".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Io("the transfer's stdout was not piped".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EngineError::Io("the transfer's stderr was not piped".to_owned()))?;
        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            stdin,
            stdout,
            stderr,
        })
    }
}

/// Kill the child of a session that is still running.
///
/// The only cancellation xsync offers: nothing below `V3Message::Cancel` can be
/// interrupted and neither entry point takes a token (xsync's story 4.74). The
/// session then fails with a transport error on both ends, which is what the
/// caller reads as "the user stopped it" rather than as a failure.
///
/// The lock is held only for the kill itself, never across the `wait` — the
/// session thread takes the child *out* of the mutex before reaping it, so a
/// cancel arriving a moment late finds nothing and does nothing.
fn kill(child: &Mutex<Option<std::process::Child>>) {
    let mut held = child.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(child) = held.as_mut() {
        let _ = child.kill();
    }
}

/// Everything one session's events add up to.
#[derive(Default)]
struct Tally {
    /// Bytes of files the engine has finished with, published or skipped.
    settled: u64,
    /// Bytes in flight, per path, from `Progress`.
    in_flight: HashMap<String, u64>,
    transferred: usize,
    skipped: usize,
    failed: usize,
    /// The first per-entry failure, which is the one worth showing.
    first_failure: Option<String>,
    /// False as soon as one published file did not take a verified path.
    every_file_verified: bool,
    /// Set once `Planned` has said how big the job is.
    planned_bytes: Option<u64>,
}

impl Tally {
    fn new() -> Self {
        Self {
            every_file_verified: true,
            ..Self::default()
        }
    }

    fn moved(&self) -> u64 {
        self.settled + self.in_flight.values().sum::<u64>()
    }

    /// Fold one event in. Returns true when the row should be redrawn.
    fn absorb(&mut self, event: &LocalEvent) -> bool {
        match event {
            LocalEvent::Planned { bytes, .. } => {
                self.planned_bytes = Some(*bytes);
                true
            }
            LocalEvent::Progress {
                path, completed, ..
            } => {
                self.in_flight.insert(path.clone(), *completed);
                true
            }
            LocalEvent::Transferred {
                path,
                bytes,
                method,
                ..
            } => {
                self.in_flight.remove(path);
                self.settled += bytes;
                self.transferred += 1;
                // Over a wire there is no clone path at all, so anything that
                // is not a byte copy is a route this adapter has not seen and
                // cannot vouch for.
                if *method != TransferMethod::ByteCopy {
                    self.every_file_verified = false;
                }
                true
            }
            LocalEvent::Skipped { path, bytes, .. } => {
                self.in_flight.remove(path);
                self.settled += bytes;
                self.skipped += 1;
                true
            }
            LocalEvent::Failed { path, message, .. } => {
                self.in_flight.remove(path);
                self.failed += 1;
                if self.first_failure.is_none() {
                    self.first_failure = Some(if path.is_empty() {
                        message.clone()
                    } else {
                        format!("{path}: {message}")
                    });
                }
                true
            }
            _ => false,
        }
    }

    /// What the row should say once the session is over.
    ///
    /// `verified` is a claim about what crossed the wire, and it is one the
    /// engine earns without being asked: the source computes a BLAKE3 digest of
    /// every file as it reads it, ships it in the same frame as the data, and
    /// the sink compares before it renames the staged file into place. Nothing
    /// is published that did not match. For a file over 8 MiB the comparison is
    /// per 8 MiB chunk plus a proof that the ranges cover the file, rather than
    /// one digest over the whole of it — which is why `--paranoid` exists, and
    /// why this word means "every byte was checked against the source's digest"
    /// rather than "one digest was taken end to end".
    ///
    /// It is deliberately **not** derived from `LocalSyncOptions::checksum`.
    /// That flag decides which files are *selected*; it verifies nothing.
    fn settle(&self, report: &LocalSyncReport) -> Settled {
        // The report and the `Finished` event are two views of one run, so the
        // report is the one read here and the counts above are only what the
        // row showed on the way.
        let bytes = self.settled.max(report.transferred_bytes);
        let note = if report.transferred_files == 0 && self.skipped > 0 {
            // Every file was already there. See [`options`] for what "already
            // there" is a claim about — with `checksum` off it is size and
            // mtime, which is what the planner had to work with.
            Some(if self.skipped == 1 {
                "unchanged".to_owned()
            } else {
                format!("{} files unchanged", self.skipped)
            })
        } else if self.skipped > 0 {
            Some(format!("{} unchanged", self.skipped))
        } else {
            None
        };
        Settled {
            bytes,
            // Nothing published is nothing verified. A row that said `verified`
            // for a run that moved no bytes would be claiming this app checked
            // something it never read.
            verified: self.every_file_verified && report.transferred_files > 0,
            note,
            stopped: false,
        }
    }
}

/// What one finished session hands back: the engine's answer, and the child's.
type Session = (
    std::result::Result<LocalSyncReport, xsync_core::server::ServerError>,
    Option<std::io::Result<std::process::ExitStatus>>,
);

/// Run the blocking session on its own thread and reap the child after it.
///
/// Everything the engine touches lives inside this closure: it owns both pipe
/// halves for the length of the session and drops them on the way out, and
/// that close is the EOF the remote waits for before it exits.
fn spawn_session(
    direction: Direction,
    local_root: PathBuf,
    remote_root: String,
    trailing: bool,
    pipes: (std::process::ChildStdout, std::process::ChildStdin),
    child: Arc<Mutex<Option<std::process::Child>>>,
    events: tokio::sync::mpsc::UnboundedSender<LocalEvent>,
) -> tokio::task::JoinHandle<Session> {
    let (stdout, stdin) = pipes;
    tokio::task::spawn_blocking(move || {
        let options = options();
        let reader = BufReader::new(stdout);
        // A megabyte in front of the pipe, and the pipe itself behind a thread.
        // The buffer is what stops a batch of small files becoming a write
        // syscall per file; the thread is what stops that batch deadlocking.
        let writer = BufWriter::with_capacity(WRITE_BUFFER, Detached::new(stdin));
        let emit = |event: LocalEvent| {
            // A closed receiver means the consumer is gone, which is not a
            // reason to stop moving bytes.
            let _ = events.send(event);
        };
        let outcome = match direction {
            Direction::Up => run_client_push(
                &local_root,
                trailing,
                // The same string the remote was given as `--server`: it is
                // half of the resume journal's job identity, and a different
                // spelling silently gives an interrupted transfer a new
                // identity and no resume.
                &remote_root,
                false,
                &options,
                reader,
                writer,
                emit,
            ),
            Direction::Down => run_client_pull(
                &remote_root,
                trailing,
                &local_root,
                false,
                &options,
                reader,
                writer,
                emit,
            ),
        };
        // Reaped after the call, not before: waiting while still holding a copy
        // of the child's stdin deadlocks. Taken out of the mutex first, so a
        // cancel arriving now cannot block on a `wait` that is itself waiting
        // for the process it wants to kill.
        let reaped = child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .map(|mut child| child.wait());
        (outcome, reaped)
    })
}

/// Run one Local↔xsync transfer to completion.
///
/// `remote_is_dir` is what the mount says about the xsync side of the pair;
/// it decides whether the transfer's root is the item itself or its container.
/// `progress` is called with the bytes moved so far, at whatever rate the
/// engine reports them — the caller throttles.
///
/// # Errors
///
/// The session's own failure, or a refusal before it starts: a destination
/// mount that is not writable, or a path that would climb out of the share.
/// Per-entry failures do **not** fail the call — a run that could not publish
/// one file of a thousand still returns `Ok`, with the failure in
/// [`Settled::note`]'s place on the row. That is xsync's contract (its exit
/// status 23) and reading `Ok` as "everything landed" would report a success
/// the transfer did not have.
pub async fn run(
    route: &XsyncRoute,
    transfer: &Transfer,
    remote_is_dir: bool,
    stop: Arc<AtomicBool>,
    mut progress: impl FnMut(u64, Option<u64>) + Send + 'static,
) -> Result<Settled> {
    // The seam rule, and here the only gate there is: `--read-only` on the
    // remote does not stop a v1/v2 push.
    if transfer.direction == Direction::Up && !route.writable {
        return Err(EngineError::Io(format!(
            "this share is read-only: {}",
            route.reason
        )));
    }

    let plan = Plan::build(route, transfer, remote_is_dir)?;
    let remote = Remote::spawn(route, &plan.remote_root)?;
    let Remote {
        child,
        stdin,
        stdout,
        mut stderr,
    } = remote;

    // Drained on its own thread: an unread pipe blocks the child once it fills,
    // and this text is the only account of a session that never started.
    let diagnostics = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    // The callback runs on the session thread inside the transfer loop, so it
    // does nothing but push. Unbounded because the alternative — a bounded
    // channel — makes a slow consumer into back-pressure on the transfer,
    // which is exactly what the contract says not to do.
    let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel::<LocalEvent>();

    let session = spawn_session(
        transfer.direction,
        plan.local_root.clone(),
        plan.remote_root.clone(),
        plan.trailing_slash,
        (stdout, stdin),
        Arc::clone(&child),
        events,
    );

    // Watch for a cancel while the session runs. The flag is the worker's, and
    // killing the child is the only way to act on it.
    let watching = Arc::clone(&stop);
    let doomed = Arc::clone(&child);
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&finished);
    let watcher = tokio::spawn(async move {
        while !done.load(Ordering::SeqCst) {
            if watching.load(Ordering::SeqCst) {
                kill(&doomed);
                return;
            }
            tokio::time::sleep(CANCEL_POLL).await;
        }
    });

    // Drain the events into the row while the session runs.
    let mut tally = Tally::new();
    let mut sampler = Progress::new();
    while let Some(event) = incoming.recv().await {
        if !tally.absorb(&event) {
            continue;
        }
        let moved = tally.moved();
        if sampler.sample(moved, Instant::now()) {
            progress(moved, tally.planned_bytes);
        }
    }

    finished.store(true, Ordering::SeqCst);
    watcher.abort();

    let (outcome, reaped) = session
        .await
        .map_err(|error| EngineError::Io(format!("the transfer thread failed: {error}")))?;
    let diagnostics = diagnostics.join().unwrap_or_default();

    // A killed child fails the session with a transport error on both ends.
    // That is the user's decision arriving, not a failure, and it outranks
    // whatever the engine was about to say.
    if stop.load(Ordering::SeqCst) {
        return Ok(Settled {
            bytes: tally.moved(),
            verified: false,
            note: None,
            stopped: true,
        });
    }

    let report = judge((outcome, reaped), &tally, &diagnostics)?;

    // Whatever the throttle last published, the row ends on the real number.
    let settled = tally.settle(&report);
    progress(settled.bytes, tally.planned_bytes);
    drop(plan.staging);
    Ok(settled)
}

/// Decide whether a finished session actually succeeded.
///
/// Three ways it can not have, and all three end on the row as a failure:
/// the engine itself failed; the child exited non-zero, which is where a
/// refused key and a missing `xs` show up; or the run finished with per-entry
/// failures. That last one comes back as `Ok` — it is what `xs` maps to exit
/// status 23 — and a consumer that read `Ok` as "everything landed" would
/// report a success it did not have.
fn judge(session: Session, tally: &Tally, diagnostics: &str) -> Result<LocalSyncReport> {
    let (outcome, reaped) = session;
    let report = outcome.map_err(|error| explain(&error.to_string(), diagnostics))?;
    match reaped {
        Some(Ok(status)) if !status.success() => {
            return Err(explain(
                &format!("the remote exited with {status}"),
                diagnostics,
            ))
        }
        Some(Err(error)) => {
            return Err(EngineError::Io(format!(
                "could not reap the remote: {error}"
            )))
        }
        _ => {}
    }
    if report.partial_failure() || tally.failed > 0 {
        let failed = report.failed_entries.max(tally.failed);
        let detail = tally
            .first_failure
            .clone()
            .unwrap_or_else(|| "the reason was not reported".to_owned());
        return Err(EngineError::Io(format!(
            "{failed} of {} entries failed — {detail}",
            failed + report.transferred_files + report.skipped_files,
        )));
    }
    Ok(report)
}

/// Fold what the child said into the failure the row will show.
///
/// `ssh` writes its authentication and host-key refusals to stderr and the
/// engine only ever sees "the peer did not speak". Without this, a rejected key
/// reaches the user as a protocol error.
fn explain(message: &str, diagnostics: &str) -> EngineError {
    let said = diagnostics
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("Warning: Permanently added"));
    match said {
        Some(line) => EngineError::Io(format!("{message} — {line}")),
        None => EngineError::Io(message.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{basename, parent_of, under, Staging};

    #[test]
    fn a_relative_path_joins_onto_the_export_root() {
        assert_eq!(
            under("/srv/strife", "films/a.mkv").unwrap(),
            "/srv/strife/films/a.mkv"
        );
        assert_eq!(under("/srv/strife/", "films").unwrap(), "/srv/strife/films");
        // The root of the share is the export root itself.
        assert_eq!(under("/srv/strife", "").unwrap(), "/srv/strife");
        assert_eq!(under("/srv/strife", "/").unwrap(), "/srv/strife");
    }

    #[test]
    fn a_path_cannot_climb_out_of_the_share() {
        // The queue's paths come from listings this app made, so this is a bug
        // or an attack; resolving it would hand a transfer a root outside the
        // share the user mounted.
        assert!(under("/srv/strife", "../etc").is_err());
        assert!(under("/srv/strife", "films/../../etc").is_err());
        assert!(under("/srv/strife", "./films").is_err());
    }

    #[test]
    fn the_parent_of_a_top_level_path_is_the_root() {
        assert_eq!(parent_of("/srv/strife/a.mkv"), "/srv/strife");
        assert_eq!(parent_of("/a.mkv"), "/");
        assert_eq!(parent_of("a.mkv"), "");
        assert_eq!(basename("/srv/strife/a.mkv"), "a.mkv");
    }

    #[test]
    fn staging_links_the_file_under_the_wanted_name_and_sweeps_itself() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("report.pdf");
        std::fs::write(&source, b"contents").unwrap();

        let directory = {
            let staged = Staging::link(&source, "report 2.pdf", "abc").unwrap();
            let link = staged.directory.join("report 2.pdf");
            assert_eq!(std::fs::read(&link).unwrap(), b"contents");
            // One directory, one entry: the push publishes exactly one file.
            assert_eq!(std::fs::read_dir(&staged.directory).unwrap().count(), 1);
            staged.directory.clone()
        };
        assert!(
            !directory.exists(),
            "the staging directory outlived the transfer"
        );
        // The link is gone; the file it shared bytes with is not.
        assert_eq!(std::fs::read(&source).unwrap(), b"contents");
    }
}
