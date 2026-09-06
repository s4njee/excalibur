//! The xsync protocol v3 backend.
//!
//! This is the adapter the plan called for building *before* extracting a trait
//! on the xsync side: what it needs, and what it has to work around, is the
//! evidence for what `xsync_client::Fs` should eventually contain.
//!
//! The namespace mutations arrived with `xsyncv3.md` E5-S4, so `mkdir`,
//! `rename` and `remove` are real here now and `Caps::atomic_rename` is true:
//! the server renames through `renameat2`/`renamex_np` rather than a
//! check-then-rename this side could only do as a race.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncBufReadExt as _;
use xsync_client::{
    attr_presence, Access, Client, Mount, OpenFile, RenameMode, CREATE, READ, TRUNC, WRITE,
};

use super::{
    Attrs, Capacity, Caps, CasOutcome, Entry, EntryKind, Health, Listing, MountInfo, MountStatus,
    ReadHandle, ShareBackend, WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// A mounted xsync export.
pub struct XsyncBackend {
    mount: Mount,
    /// Kept so the connection's tasks outlive every request made through it.
    _client: Arc<Client>,
    root_label: String,
    /// The last `StatFs` answer and when it was taken.
    ///
    /// `mount_info` is synchronous — the status bar reads the facts that rode
    /// along with a listing it already has (E10-S1) — so capacity cannot be a
    /// round trip there. It is taken at mount and refreshed around a listing,
    /// which is the moment the number is about to be looked at.
    capacity: Mutex<Option<(Capacity, Instant)>>,
}

/// How stale the cached capacity may get before a listing refreshes it.
///
/// E7-S5 allows one `StatFs` per listing, and this does not take that option:
/// `list_dir` is also the walk inside a recursive delete, where a second round
/// trip per directory would double the traffic of a delete to keep a number
/// nobody is reading. Thirty seconds is the story's stated alternative, and it
/// is well inside the time a user spends looking at one directory.
const CAPACITY_TTL: Duration = Duration::from_secs(30);

/// Everything needed to reach an `xs --server` over SSH.
///
/// Grouped into a struct because the host-key file is not optional and should
/// not be easy to leave out: a call site that forgets it would silently fall
/// back to OpenSSH's own trust store and prompt behaviour, which is the thing
/// this story removes.
#[derive(Debug, Clone)]
pub struct SshTarget<'a> {
    /// Host as the user typed it. Also the name the key is trusted under.
    pub host: &'a str,
    /// Port `ssh` should connect to.
    pub port: u16,
    /// Remote username, or `None` for `ssh`'s own default.
    pub username: Option<&'a str>,
    /// The app's `known_hosts` file. See [`crate::hostkeys::KnownHosts`].
    pub known_hosts: &'a std::path::Path,
    /// An alternative to the `ssh` binary, for tests.
    pub rsh: Option<&'a str>,
}

/// The remote command line that starts `xs --server <root>`.
///
/// One string handed to the remote *login shell*, so every argument is
/// single-quoted and an embedded `'` becomes `'\''`. A path that is not quoted
/// this way is a command-injection hole rather than a style preference. The
/// `PATH` prefix is there because a non-interactive `ssh host cmd` reads no
/// login profile and `~/.local/bin` is where `xs` usually is.
///
/// Shared with the transfer session (E22-S1): a transfer opens its *own* `ssh`
/// — one `xs --server` serves one session, and a browse session and a sync
/// session are different protocols chosen at the handshake — and it must send
/// byte-for-byte the same command the browse session does. See
/// `../xsync/docs/library.md`.
#[must_use]
pub fn remote_command(root: &str) -> String {
    format!(
        "PATH=\"$HOME/.local/bin:$PATH\" 'xs' '--server' '{}'",
        root.replace('\'', "'\\''")
    )
}

/// The `ssh` invocation for a session against `root`: program, then arguments.
///
/// Returned as data rather than as a built `Command` because the browse session
/// wants a `tokio::process::Command` and a transfer wants a blocking
/// `std::process::Command` — xsync's engine reads and writes the pipes
/// synchronously. Both must carry the *same* host-key policy, and the way to
/// guarantee that is for there to be one place that decides it.
#[must_use]
pub fn ssh_argv(target: &SshTarget<'_>, root: &str) -> (String, Vec<String>) {
    let mut args = vec![
        "-p".to_owned(),
        target.port.to_string(),
        // Never fall back to OpenSSH's own prompt. There is no terminal to
        // answer it on, so the choice is between failing here with a key we
        // can describe and hanging on a question nobody can see.
        "-o".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        "-o".to_owned(),
        // Quoted, because OpenSSH reads this option as a space-separated
        // *list* of files. The default data directory on macOS is
        // `~/Library/Application Support`, so an unquoted path splits into
        // two names that both do not exist, and every connection fails with
        // "No host key is known" for a key that was just recorded.
        format!("UserKnownHostsFile=\"{}\"", target.known_hosts.display()),
        // Same reasoning for credentials: a password or passphrase prompt
        // would go to a terminal that is not there. BatchMode turns that
        // hang into an immediate, reportable failure. Key and agent auth,
        // which is what `xs` over SSH is set up for, are unaffected.
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
    ];
    if let Some(username) = target.username {
        args.push("-l".to_owned());
        args.push(username.to_owned());
    }
    args.push(target.host.to_owned());
    args.push(remote_command(root));
    (target.rsh.unwrap_or("ssh").to_owned(), args)
}

impl XsyncBackend {
    /// Connect over SSH and mount the export at `root`.
    ///
    /// The `ssh` command line is built here rather than by `xsync-client`
    /// because the host-key policy is Excalibur's to set: this passes the app's
    /// own `known_hosts` and `StrictHostKeyChecking=yes`, so `ssh` enforces the
    /// decision the user made in the trust prompt and nothing else. It is also
    /// the only way to pass a port or a username, which the client's
    /// convenience method has no argument for.
    ///
    /// Call [`crate::hostkeys::verify`] first. Reaching a `Trust::Unknown` host
    /// here produces `Host key verification failed` from `ssh` and nothing
    /// useful for the window to show, which is exactly the failure E3-S4
    /// exists to remove.
    ///
    /// # Errors
    ///
    /// Fails when the remote shell cannot start `xs`, the host key is not in
    /// `known_hosts`, the peer does not speak protocol v3, or the export cannot
    /// be mounted.
    pub async fn connect_ssh(target: &SshTarget<'_>, root: &str) -> Result<Self> {
        let (program, args) = ssh_argv(target, root);
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Piped, not inherited: in the desktop app the inherited stream
            // went nowhere, so `Permission denied (publickey)` reached the
            // user as "connection closed before the request was answered"
            // (E7-S4). What `ssh` says here is the only account of why a
            // session never started.
            .stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| EngineError::Io(format!("could not start ssh: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Io("ssh stdout was not piped".to_owned()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Io("ssh stdin was not piped".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| EngineError::Io("ssh stderr was not piped".to_owned()))?;
        let mut diagnosis = SshStderr::drain(stderr, target.host.to_owned());

        // Both failures below are "the session died before it answered", which
        // is what the client can see and not why it happened. `ssh`'s own
        // words are the why, so they are collected before the error is built.
        let client = match Client::from_split(stdout, stdin, 0).await {
            Ok(client) => client,
            Err(error) => {
                return Err(diagnosis
                    .explain(target, &mut child, map_error(&error))
                    .await)
            }
        };
        match Self::mount(client, root).await {
            // A drop between the handshake and the first request is the same
            // failure a moment later, and deserves the same explanation.
            Err(EngineError::Disconnected(message)) => Err(diagnosis
                .explain(target, &mut child, EngineError::Disconnected(message))
                .await),
            // On the way out the drain is dropped rather than aborted:
            // dropping a `JoinHandle` detaches the task, which is what keeps
            // `ssh`'s later stderr reaching the log for the life of the
            // session. It still ends on its own when the pipe closes.
            other => other,
        }
    }

    /// Run `xs --server` locally and mount the directory at `root`.
    ///
    /// The same protocol over a pipe instead of a socket. It is how the backend
    /// is exercised without a second machine, and it is a real deployment for a
    /// share on the same host.
    ///
    /// # Errors
    ///
    /// As [`Self::connect_ssh`], plus failure to spawn `xs` itself.
    pub async fn connect_local(program: &str, root: &str, read_only: bool) -> Result<Self> {
        let mut command = tokio::process::Command::new(program);
        command.arg("--server").arg(root);
        if read_only {
            command.arg("--read-only");
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|error| EngineError::Io(format!("could not start {program}: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Io("xs stdout was not piped".to_owned()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Io("xs stdin was not piped".to_owned()))?;
        let client = Client::from_split(stdout, stdin, 0)
            .await
            .map_err(|error| map_error(&error))?;
        Self::mount(client, root).await
    }

    async fn mount(client: Client, root: &str) -> Result<Self> {
        // The empty export name is the one `xs --server` serves; named exports
        // arrive with the daemon.
        let mount = client
            .mount(b"", Access::ReadWrite)
            .await
            .map_err(|error| map_error(&error))?;
        let backend = Self {
            mount,
            _client: Arc::new(client),
            root_label: root.to_owned(),
            capacity: Mutex::new(None),
        };
        // Seeded here so the very first listing carries a capacity: the status
        // bar reads it from the listing's mount facts, and a mount that has
        // never been listed twice would otherwise show nothing (E7-S5).
        // Ignored on failure — a share that cannot answer `StatFs` is still a
        // share, and the bar simply has no number to show.
        let _ = backend.refresh_capacity().await;
        Ok(backend)
    }

    /// Ask the server how much space there is, and remember the answer.
    async fn refresh_capacity(&self) -> Result<Capacity> {
        let usage = self
            .mount
            .statfs()
            .await
            .map_err(|error| map_error(&error))?;
        let capacity = Capacity {
            // `available` rather than `free`: the root-only reserve is not
            // space this session can use, so showing it would overstate.
            free: usage.available_bytes,
            total: usage.total_bytes,
        };
        *self.capacity.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((capacity, Instant::now()));
        Ok(capacity)
    }

    /// Refresh the capacity if it has gone stale, keeping the old answer if not.
    ///
    /// A failure is deliberately dropped: a listing that worked must not fail
    /// because the number beside it could not be updated.
    async fn refresh_capacity_if_stale(&self) {
        let fresh = self
            .capacity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|(_, taken)| taken.elapsed() < CAPACITY_TTL);
        if !fresh {
            let _ = self.refresh_capacity().await;
        }
    }

    /// The export root, for the status bar's plain-language restatement.
    #[must_use]
    pub fn root_label(&self) -> &str {
        &self.root_label
    }

    /// Open a stage for a whole-file replacement, never resuming one.
    ///
    /// `StageOpen` with an empty resume token adopts whatever stage the
    /// server already holds for the destination. That is the right default
    /// for an upload picking up after a disconnect (E13-S6) and the wrong one
    /// here: this path always writes the entire file from offset 0, so any
    /// bytes already staged are from an earlier, abandoned save and would
    /// survive beyond the new content's length — the commit then fails its
    /// digest. A stage found with bytes in it is aborted and a clean one
    /// opened.
    async fn fresh_stage(&self, path: &str, size: u64) -> Result<xsync_client::Stage> {
        let stage = self
            .mount
            .stage(path.as_bytes(), size, 0o644, b"")
            .await
            .map_err(|error| map_error(&error))?;
        if stage.staged_bytes() == 0 {
            return Ok(stage);
        }
        stage.abort().await.map_err(|error| map_error(&error))?;
        let stage = self
            .mount
            .stage(path.as_bytes(), size, 0o644, b"")
            .await
            .map_err(|error| map_error(&error))?;
        if stage.staged_bytes() != 0 {
            // Aborted and still there: something else is uploading here.
            stage.abort().await.ok();
            return Err(EngineError::Io(format!(
                "a stage for {path} is already in progress and cannot be replaced"
            )));
        }
        Ok(stage)
    }

    /// Abandon a stage by its token. Best effort: the failure being reported
    /// is the caller's, and a stage that could not be aborted is swept by
    /// the server's retention, or by the next `fresh_stage` here.
    async fn abort_stage(&self, path: &str, token: &[u8]) {
        if let Ok(stage) = self.mount.stage(path.as_bytes(), 0, 0o644, token).await {
            stage.abort().await.ok();
        }
    }

    /// Remove a directory and everything under it, depth first.
    ///
    /// The walk is here rather than on the server because that is what lets a
    /// caller report progress and stop: a server-side recursive delete is one
    /// request that either finishes or half-finishes with nothing to show for
    /// it. `xsyncv3.md` E5-S4 makes the same argument for keeping the verbs
    /// single-purpose.
    ///
    /// Iterative rather than recursive: an `async fn` that calls itself needs
    /// boxing, and a deep tree would otherwise be bounded by the stack rather
    /// than by the share.
    async fn remove_tree(&self, root: &str) -> Result<()> {
        // Directories in the order found, so unwinding in reverse removes
        // children before parents.
        let mut directories = vec![root.to_owned()];
        let mut index = 0;
        while index < directories.len() {
            let directory = directories[index].clone();
            index += 1;
            for entry in self.list_dir(&directory).await?.entries {
                if entry.attrs.kind == EntryKind::Dir {
                    directories.push(entry.path);
                } else {
                    self.mount
                        .unlink(entry.path.as_bytes())
                        .await
                        .map_err(|error| map_error(&error))?;
                }
            }
        }
        for directory in directories.into_iter().rev() {
            self.mount
                .rmdir(directory.as_bytes())
                .await
                .map_err(|error| map_error(&error))?;
        }
        Ok(())
    }

    /// Open one file with exactly the access asked for.
    ///
    /// A write-class open is refused here as well as by the server, so a UI bug
    /// that offers a write on a read-only mount costs a local error rather than
    /// a round trip.
    async fn open(&self, path: &str, flags: u32) -> Result<OpenFile> {
        const WRITE_CLASS: u32 = WRITE | CREATE | TRUNC;
        if flags & WRITE_CLASS != 0 && !self.mount.info().writable {
            return Err(EngineError::ReadOnly(self.mount.info().reason.clone()));
        }
        self.mount
            .open(path.trim_matches('/').as_bytes(), flags)
            .await
            .map_err(|error| map_error(&error))
    }
}

/// A share-relative path as the wire wants it: no leading or trailing slash.
///
/// The engine's paths are already share-relative, but a caller that built one
/// by joining can leave a stray separator, and the server would then see a
/// different name from the one the listing reported.
fn trimmed(path: &str) -> &str {
    path.trim_matches('/')
}

/// A 16-byte cookie from the engine's opaque bytes, if it is one.
fn cookie_bytes(raw: &[u8]) -> Option<[u8; 16]> {
    raw.try_into().ok()
}

/// The trait's portable path, for the cases the staged one cannot serve.
async fn default_cas(
    backend: &XsyncBackend,
    path: &str,
    expected: Option<&Attrs>,
    data: &[u8],
) -> Result<CasOutcome> {
    if let Some(expected) = expected {
        let current = backend.stat(path).await?;
        if !current.unchanged_from(expected) {
            return Ok(CasOutcome::Changed(current));
        }
    }
    backend.write_range(path, WriteMode::Create, data).await?;
    Ok(CasOutcome::Written(backend.stat(path).await?))
}

/// How many lines of `ssh`'s stderr are kept, and how long each may be.
///
/// Bounded because the drain runs for the whole life of the session: a remote
/// that chatters on stderr must not grow this without limit. The *last* lines
/// are the ones that say why a session failed, so a full buffer drops the
/// oldest.
const STDERR_LINES: usize = 20;
/// See [`STDERR_LINES`].
const STDERR_LINE_BYTES: usize = 512;

/// How long to wait for `ssh`'s last words before reporting the failure.
const STDERR_GRACE: Duration = Duration::from_millis(500);

/// `ssh`'s stderr, captured rather than inherited.
///
/// The drain is a task because the pipe has to keep being read while the
/// session runs: a full pipe would block `ssh` itself, which would hang the
/// connection rather than logging it. It ends at EOF — which is when `ssh`
/// exits — so it cannot outlive the process it is reading and nothing has to
/// remember to stop it.
struct SshStderr {
    /// The last [`STDERR_LINES`] lines, oldest first.
    lines: Arc<Mutex<Vec<String>>>,
    drain: tokio::task::JoinHandle<()>,
}

impl SshStderr {
    /// Start draining `stderr`, logging each line as it arrives.
    fn drain(stderr: tokio::process::ChildStderr, host: String) -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let drain = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let line = clamp(line.trim());
                if line.is_empty() {
                    continue;
                }
                // Logged as it arrives, because once the session is up this is
                // the only account of why it later dropped (E7-S4). Nothing
                // here is secret: `ssh` never echoes a password, and BatchMode
                // means it never asks for one.
                eprintln!("ssh[{host}]: {line}");
                let mut kept = sink.lock().unwrap_or_else(PoisonError::into_inner);
                if kept.len() == STDERR_LINES {
                    kept.remove(0);
                }
                kept.push(line);
            }
        });
        Self { lines, drain }
    }

    /// What `ssh` wrote, once it has had a moment to write its last.
    ///
    /// The client noticing a closed pipe and `ssh` writing its reason are two
    /// different descriptors, so which lands first is a race. Waiting for the
    /// drain to end — instant for a process that has already exited — is what
    /// puts the reason in the error rather than only in the log.
    async fn tail(&mut self) -> String {
        let _ = tokio::time::timeout(STDERR_GRACE, &mut self.drain).await;
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .join("\n")
    }

    /// Turn "the session died" into what actually happened.
    async fn explain(
        &mut self,
        target: &SshTarget<'_>,
        child: &mut tokio::process::Child,
        fallback: EngineError,
    ) -> EngineError {
        let tail = self.tail().await;
        // `ssh` exits 255 for everything it fails at itself, so the status
        // only distinguishes the *remote command's* failures — 127 from the
        // login shell being the one that says `xs` was not found. Absent when
        // the process is somehow still running, which the text still covers.
        let status = tokio::time::timeout(STDERR_GRACE, child.wait())
            .await
            .ok()
            .and_then(std::result::Result::ok);
        ssh_failure(target, &tail, status, fallback)
    }
}

/// At most [`STDERR_LINE_BYTES`] of a line, cut on a character boundary.
fn clamp(line: &str) -> String {
    if line.len() <= STDERR_LINE_BYTES {
        return line.to_owned();
    }
    let mut end = STDERR_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// The first captured line containing any of `needles`, matched case-blind.
fn line_with<'a>(stderr: &'a str, needles: &[&str]) -> Option<&'a str> {
    stderr.lines().find(|line| {
        let lowered = line.to_ascii_lowercase();
        needles.iter().any(|needle| lowered.contains(needle))
    })
}

/// One of `ssh`'s lines, made to stand alone in a message.
///
/// Drops `ssh`'s own `ssh: ` prefix, which would read twice over inside an
/// error that already says what it is, and names the host when the line does
/// not — "Permission denied, please try again" is otherwise about nothing.
fn naming(host: &str, line: &str) -> String {
    let line = line.strip_prefix("ssh: ").unwrap_or(line);
    if line.contains(host) {
        line.to_owned()
    } else {
        format!("{host}: {line}")
    }
}

/// The `SHA256:` fingerprint `ssh` printed, when it printed one.
fn offered_fingerprint(stderr: &str) -> Option<String> {
    stderr
        .split_whitespace()
        .find(|word| word.starts_with("SHA256:"))
        .map(|word| word.trim_end_matches('.').to_owned())
}

/// The key type `ssh` named, e.g. `ED25519`.
fn offered_algorithm(stderr: &str) -> Option<String> {
    const ALGORITHMS: [&str; 4] = ["ED25519", "ECDSA", "RSA", "DSA"];
    let upper = stderr.to_ascii_uppercase();
    ALGORITHMS
        .into_iter()
        .find(|algorithm| upper.contains(algorithm))
        .map(str::to_owned)
}

/// The fingerprint this app has on file for the host, if it can read one.
///
/// Worth the file read on this path only: when `ssh` refuses a key the app
/// believes it has trusted, the two stores disagree, and the message is much
/// more useful for saying which key was expected.
fn recorded_fingerprint(target: &SshTarget<'_>) -> Option<String> {
    crate::hostkeys::KnownHosts::new(target.known_hosts)
        .stored(target.host, target.port)
        .ok()?
        .into_iter()
        .find(|stored| !stored.revoked)
        .map(|stored| stored.key.fingerprint())
}

/// What `ssh` said, as an error the window can act on.
///
/// Matching OpenSSH's English is unlovely, and it is the only signal there is:
/// `ssh` exits 255 for a refused key, an unreachable host and a rejected
/// password alike. Anything unrecognised stays [`EngineError::Disconnected`] —
/// carrying the captured text, so a phrase this does not know is still
/// readable rather than lost, which is the whole failure E7-S4 is about.
fn ssh_failure(
    target: &SshTarget<'_>,
    stderr: &str,
    status: Option<std::process::ExitStatus>,
    fallback: EngineError,
) -> EngineError {
    let host = target.host;

    // First, because a changed key is the one failure that must never be
    // reported as something a retry or a new password could fix.
    if let Some(line) = line_with(
        stderr,
        &[
            "remote host identification has changed",
            "host key verification failed",
        ],
    ) {
        return EngineError::HostKeyChanged {
            host: host.to_owned(),
            port: target.port,
            algorithm: offered_algorithm(stderr).unwrap_or_else(|| "host".to_owned()),
            fingerprint: offered_fingerprint(stderr)
                .unwrap_or_else(|| format!("a key ssh would not accept ({})", line.trim())),
            expected: recorded_fingerprint(target)
                .unwrap_or_else(|| "the key this app recorded".to_owned()),
        };
    }
    if let Some(line) = line_with(
        stderr,
        &[
            "permission denied",
            "too many authentication failures",
            "no supported authentication methods",
        ],
    ) {
        // Not transient, so the session manager stops rather than re-offering
        // a rejected credential until the account locks (E15-S2).
        return EngineError::AuthenticationFailed(naming(host, line));
    }
    if let Some(line) = line_with(
        stderr,
        &[
            "could not resolve hostname",
            "nodename nor servname",
            "name or service not known",
            "no address associated with hostname",
        ],
    ) {
        return EngineError::HostNotFound(naming(host, line));
    }
    // 127 is what a login shell exits with when the command is not there, and
    // the wording differs by shell (`bash: xs: command not found`,
    // `sh: xs: not found`), so both the status and the text are consulted.
    let missing = line_with(
        stderr,
        &[
            "command not found",
            "not found",
            "no such file or directory",
        ],
    );
    if missing.is_some() || status.is_some_and(|status| status.code() == Some(127)) {
        // The line as the remote shell wrote it, not host-prefixed: the
        // message already says which host it is about.
        let said = missing.map_or("the remote shell could not start it", str::trim);
        return EngineError::NotFound(format!(
            "xs on {host}: {said} — install xs on the server or put it on PATH"
        ));
    }
    if let Some(line) = line_with(
        stderr,
        &[
            "connection refused",
            "connection timed out",
            "operation timed out",
            "connection reset",
            "connection closed by",
            "no route to host",
            "network is unreachable",
            "broken pipe",
            "kex_exchange_identification",
        ],
    ) {
        // Genuinely a transport drop, so it stays retryable.
        return EngineError::Disconnected(naming(host, line));
    }

    match fallback {
        EngineError::Disconnected(message) if !stderr.trim().is_empty() => {
            EngineError::Disconnected(format!("{message}; ssh said: {}", summary(stderr)))
        }
        other => other,
    }
}

/// The last few captured lines, for a message that has to fit in a banner.
fn summary(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(|line| line.strip_prefix("ssh: ").unwrap_or(line))
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..].join(" / ")
}

/// Translate a client error into the engine's taxonomy.
///
/// The mapping is by `io::ErrorKind` rather than by protocol code, because the
/// client already does that translation and doing it twice would be two places
/// to keep in step.
fn map_error(error: &xsync_client::Error) -> EngineError {
    use std::io::ErrorKind;
    let message = error.to_string();
    match error.kind() {
        ErrorKind::NotFound => EngineError::NotFound(message),
        ErrorKind::PermissionDenied => {
            // The client folds EACCES and EROFS together as "denied"; only the
            // second means the UI should have disabled the affordance, so the
            // protocol code is what decides here.
            if matches!(
                error,
                xsync_client::Error::Server {
                    code: xsync_client::ErrorCode::ReadOnly,
                    ..
                }
            ) {
                EngineError::ReadOnly(message)
            } else {
                EngineError::PermissionDenied(message)
            }
        }
        ErrorKind::AlreadyExists => EngineError::AlreadyExists(message),
        ErrorKind::BrokenPipe => EngineError::Disconnected(message),
        ErrorKind::Unsupported => EngineError::Unsupported(message),
        _ => EngineError::Io(message),
    }
}

const fn kind_of(kind: u8) -> EntryKind {
    match kind {
        1 => EntryKind::File,
        2 => EntryKind::Dir,
        3 => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

/// The mask a listing asks for: enough to fill every column, and no more.
///
/// Each optional block costs the server work per entry, so asking for
/// everything would make a 10,000-entry directory pay for fields no column
/// shows.
const LISTING_MASK: u32 = attr_presence::SYMLINK_TARGET;

fn attrs_of(attrs: &xsync_client::Attrs) -> Attrs {
    Attrs {
        kind: kind_of(attrs.kind),
        size: attrs.size,
        // The wire carries nanoseconds; the UI works in milliseconds.
        modified_ms: Some(attrs.mtime_ns / 1_000_000),
        mode: Some(attrs.mode),
        link_target: attrs
            .symlink_target
            .as_ref()
            .map(|target| String::from_utf8_lossy(target).into_owned()),
        // The protocol's own opaque value, which is what makes this backend's
        // compare-and-swap exact rather than a size-and-mtime guess.
        change_cookie: Some(attrs.change_cookie.to_vec()),
    }
}

#[async_trait]
impl ShareBackend for XsyncBackend {
    fn mount_info(&self) -> MountInfo {
        let info = self.mount.info();
        MountInfo {
            status: MountStatus::Ready,
            writable: info.writable,
            reason: info.reason.clone(),
            protocol_label: "xsync v3".to_owned(),
            // The transport's own security label needs `SecurityInfo`, which
            // arrives with the TLS transport. Over SSH it is OpenSSH's business
            // and saying "ssh" is the honest amount to claim.
            security_label: "ssh".to_owned(),
            options: info.options.clone(),
            // Whatever the last `StatFs` said. The status bar shows the mount
            // facts that came with a listing, so this is where a capacity has
            // to be for it to appear at all (E7-S5).
            capacity: self
                .capacity
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .map(|(capacity, _)| capacity),
            caps: self.caps(),
        }
    }

    fn caps(&self) -> Caps {
        let info = self.mount.info();
        Caps {
            random_access: true,
            resume_write: true,
            // No rename verb in v3 yet, so a temp-then-rename upload cannot
            // promote atomically here (`xsyncv3.md` E5-S4).
            atomic_rename: true,
            symlinks: info.supports & xsync_client::supports::SYMLINKS != 0,
            watch: info.supports & xsync_client::supports::NOTIFY != 0,
        }
    }

    async fn capacity(&self) -> Result<Capacity> {
        // Always a round trip: a caller that asks for capacity directly is
        // asking the server, not the cache. It refreshes the cache on the way
        // past, so the next listing carries what this call learned.
        self.refresh_capacity().await
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let relative = path.trim_matches('/');
        let directory = self
            .mount
            .open(relative.as_bytes(), READ | xsync_client::DIRECTORY)
            .await
            .map_err(|error| map_error(&error))?;
        let listed = directory
            .read_dir_all(LISTING_MASK)
            .await
            .map_err(|error| map_error(&error));
        // Closed whether or not the listing worked: a handle leaked per failed
        // listing would exhaust the session's table.
        let closed = directory.close().await;
        let listed = listed?;
        closed.map_err(|error| map_error(&error))?;

        let entries = listed
            .into_iter()
            .map(|entry| {
                let name = String::from_utf8_lossy(&entry.name).into_owned();
                let attrs = attrs_of(&entry.attrs);
                let kind = classify_entry(&name, attrs.kind);
                Entry {
                    kind_label: kind.label,
                    kind_category: kind.category,
                    hidden: name.starts_with('.'),
                    path: if relative.is_empty() {
                        name.clone()
                    } else {
                        format!("{relative}/{name}")
                    },
                    name,
                    attrs,
                }
            })
            .collect();

        // After the listing, so a failed one does not pay for it, and before
        // the mount facts are snapshotted, so the capacity that rides along is
        // the fresh one.
        self.refresh_capacity_if_stale().await;
        Ok(Listing {
            entries,
            mount: self.mount_info(),
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let attrs = self
            .mount
            .stat(path.trim_matches('/').as_bytes(), false, LISTING_MASK)
            .await
            .map_err(|error| map_error(&error))?;
        Ok(attrs_of(&attrs))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let file = self.open(path, READ).await?;
        // Verified: the server hashes what it sends and the client checks it
        // before handing the bytes over, which is the whole reason the digest
        // is on the wire.
        let chunk = file
            .read_verified(offset, length)
            .await
            .map_err(|error| map_error(&error));
        let closed = file.close().await;
        let chunk = chunk?;
        closed.map_err(|error| map_error(&error))?;
        Ok(chunk.data)
    }

    /// One open for a whole download, read many times at once (E13-S7).
    ///
    /// This is the method `read_range` cannot be: it opened, read and closed
    /// per megabyte, three serial round trips for one chunk of payload, which
    /// on a real link cost three quarters of the throughput.
    async fn open_read(&self, path: &str) -> Result<Option<Arc<dyn ReadHandle>>> {
        let file = self.open(path, READ).await?;
        Ok(Some(Arc::new(XsyncReads {
            // The mount's own ceiling, not a number chosen here: a read above
            // it is refused by the client before it reaches the wire.
            max_read: self.mount.info().max_read,
            file: std::sync::Mutex::new(Some(Arc::new(file))),
        })))
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        // `Create` truncates, as its name says; `Resume` must not, or resuming
        // an interrupted upload would discard what had already landed. Both
        // create the file when it is missing, which is what a caller writing to
        // a new path means.
        let (flags, offset) = match mode {
            WriteMode::Create => (WRITE | CREATE | TRUNC, 0),
            WriteMode::Resume { offset } => (WRITE | CREATE, offset),
        };
        let file = self.open(path, flags).await?;
        let written = file
            .write(offset, data)
            .await
            .map_err(|error| map_error(&error));
        let flushed = file.flush().await;
        let closed = file.close().await;
        let written = written?;
        flushed.map_err(|error| map_error(&error))?;
        closed.map_err(|error| map_error(&error))?;
        Ok(u64::from(written.bytes))
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        // One level, like `mkdir(2)`. Creating missing parents is the caller's
        // loop, so a caller that asked for `a/b/c` and got `a` and `b` as well
        // can tell what it made.
        self.mount
            .mkdir(trimmed(path).as_bytes(), 0o777)
            .await
            .map(|_| ())
            .map_err(|error| map_error(&error))
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        // `replace` chooses the mode rather than being emulated: the server
        // does it atomically through `renameat2`/`renamex_np`, and a
        // check-then-rename here would be a race across a network.
        let mode = if replace {
            RenameMode::Replace
        } else {
            RenameMode::NoReplace
        };
        self.mount
            .rename(trimmed(from).as_bytes(), trimmed(to).as_bytes(), mode)
            .await
            .map(|_| ())
            .map_err(|error| map_error(&error))
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let path = trimmed(path);
        // Which verb depends on what the thing is, and the server refuses the
        // wrong one rather than guessing — `Unlink` on a directory is an error
        // even when it is empty.
        let attrs = self.stat(path).await?;
        if attrs.kind != EntryKind::Dir {
            return self
                .mount
                .unlink(path.as_bytes())
                .await
                .map_err(|error| map_error(&error));
        }
        if !recursive {
            return self
                .mount
                .rmdir(path.as_bytes())
                .await
                .map_err(|error| map_error(&error));
        }
        self.remove_tree(path).await
    }

    /// Replace a file atomically, refusing if it moved (`xsyncv3.md` E4-S7).
    ///
    /// Overrides the trait's portable default, which stats, compares and then
    /// writes — three steps another writer can slip between. Here the cookie
    /// is checked immediately before the publishing rename, on the server, so
    /// there is no window at all. The bytes go through a stage, so a refused
    /// commit leaves the destination untouched rather than half-written.
    async fn write_all_if_unchanged(
        &self,
        path: &str,
        expected: Option<&Attrs>,
        data: &[u8],
    ) -> Result<CasOutcome> {
        let path = trimmed(path);
        if !self.mount.info().writable {
            return Err(EngineError::ReadOnly(self.mount.info().reason.clone()));
        }
        // A cookie the caller does not have is a caller that cannot use this
        // path; fall back rather than publishing unconditionally behind its
        // back.
        let expect_cookie = match expected {
            None => None,
            Some(attrs) => match attrs.change_cookie.as_deref().and_then(cookie_bytes) {
                Some(cookie) => Some(cookie),
                None => return default_cas(self, path, expected, data).await,
            },
        };

        let mut stage = self.fresh_stage(path, data.len() as u64).await?;
        let max = stage.max_write() as usize;
        let mut offset = 0_usize;
        // Chunked to the mount's own limit: a whole file in one message is
        // refused above `max_write`, and an editor's save is not always small.
        while offset < data.len() {
            let end = data.len().min(offset + max);
            if let Err(error) = stage.write(offset as u64, &data[offset..end]).await {
                // Abandoned, not left for a resume: a half-written stage for
                // a whole-file replacement is a hazard, not progress.
                stage.abort().await.ok();
                return Err(map_error(&error));
            }
            offset = end;
        }
        if data.is_empty() {
            stage.write(0, &[]).await.ok();
        }

        let digest = *blake3::hash(data).as_bytes();
        // `commit` consumes the handle, so the token is what lets a refused
        // stage be found again and aborted.
        let token = stage.resume_token().to_vec();
        match stage.commit(digest, expect_cookie, None).await {
            Ok(Ok(attrs)) => Ok(CasOutcome::Written(attrs_of(&attrs))),
            // A refusal leaves the stage open on the server with the bytes it
            // did not publish. Until 2026-09-05 this walked away from it, and
            // the *next* save of the same file resumed that stage — `StageOpen`
            // with an empty token adopts any stage for the destination — so a
            // shorter edit landed on top of the longer refused one and failed
            // its own digest. `Stage` has no `Drop` that aborts, so it is done
            // here, explicitly.
            Ok(Err(current)) => {
                self.abort_stage(path, &token).await;
                Ok(CasOutcome::Changed(attrs_of(&current)))
            }
            Err(error) => {
                self.abort_stage(path, &token).await;
                Err(map_error(&error))
            }
        }
    }

    async fn ping(&self) -> Result<Health> {
        // `StatFs` is the cheapest round trip that proves the session still
        // answers; there is no dedicated ping verb, and a keepalive is not
        // routed back to a caller.
        match self.mount.statfs().await {
            Ok(_) => Ok(Health::Healthy),
            Err(_) => Ok(Health::Failed),
        }
    }
}

/// One `OpenFile`, read concurrently until the transfer is done (E13-S7).
///
/// The `Mutex` guards the *handle*, not the reads: it is held long enough to
/// clone an `Arc` and never across an await, so any number of reads are in
/// flight at once. That is the point — the connection multiplexes by request
/// id, so depth costs nothing but memory and the link stays full between round
/// trips.
///
/// The `Option` is what makes `close` reportable. `OpenFile::close` consumes
/// itself, so closing has to take the handle out of somewhere; taking it also
/// makes a read after a close an error rather than a use of something the
/// server has forgotten.
struct XsyncReads {
    file: std::sync::Mutex<Option<Arc<OpenFile>>>,
    max_read: u32,
}

impl XsyncReads {
    /// The handle, or why there is none.
    fn file(&self) -> Result<Arc<OpenFile>> {
        self.file
            .lock()
            .map_err(|_| EngineError::Io("the read handle's lock is poisoned".to_owned()))?
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| EngineError::Io("read after close".to_owned()))
    }
}

#[async_trait]
impl ReadHandle for XsyncReads {
    async fn read_at(&self, offset: u64, length: u32) -> Result<Vec<u8>> {
        let file = self.file()?;
        // Verified, exactly as `read_range` is: the digest is on the wire so
        // that it is checked, and a transfer is the last place to stop
        // checking it. It costs one BLAKE3 pass over bytes already in memory.
        let chunk = file
            .read_verified(offset, length)
            .await
            .map_err(|error| map_error(&error))?;
        Ok(chunk.data)
    }

    fn max_read(&self) -> u32 {
        self.max_read
    }

    async fn close(&self) -> Result<()> {
        let taken = self
            .file
            .lock()
            .map_err(|_| EngineError::Io("the read handle's lock is poisoned".to_owned()))?
            .take();
        let Some(file) = taken else {
            // Already closed. Saying so as an error would make a tidy-up path
            // that runs twice look like a failure.
            return Ok(());
        };
        match Arc::try_unwrap(file) {
            // The ordinary case: every read has finished, so the close can be
            // awaited and its failure reported.
            Ok(file) => file.close().await.map_err(|error| map_error(&error)),
            // A read is still in flight. Dropping the last reference sends the
            // `Close` one-way, which releases the server's handle without
            // waiting for an answer nobody is left to read.
            Err(_shared) => Ok(()),
        }
    }
}
