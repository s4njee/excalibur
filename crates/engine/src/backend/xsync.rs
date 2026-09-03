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

use std::sync::Arc;

use async_trait::async_trait;
use xsync_client::{
    attr_presence, Access, Client, Mount, OpenFile, RenameMode, CREATE, READ, TRUNC, WRITE,
};

use super::{
    Attrs, Capacity, Caps, CasOutcome, Entry, EntryKind, Health, Listing, MountInfo, MountStatus,
    ShareBackend, WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// A mounted xsync export.
pub struct XsyncBackend {
    mount: Mount,
    /// Kept so the connection's tasks outlive every request made through it.
    _client: Arc<Client>,
    root_label: String,
}

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
        let mut command = tokio::process::Command::new(target.rsh.unwrap_or("ssh"));
        command
            .arg("-p")
            .arg(target.port.to_string())
            // Never fall back to OpenSSH's own prompt. There is no terminal to
            // answer it on, so the choice is between failing here with a key we
            // can describe and hanging on a question nobody can see.
            .arg("-o")
            .arg("StrictHostKeyChecking=yes")
            .arg("-o")
            // Quoted, because OpenSSH reads this option as a space-separated
            // *list* of files. The default data directory on macOS is
            // `~/Library/Application Support`, so an unquoted path splits into
            // two names that both do not exist, and every connection fails with
            // "No host key is known" for a key that was just recorded.
            .arg(format!(
                "UserKnownHostsFile=\"{}\"",
                target.known_hosts.display()
            ))
            // Same reasoning for credentials: a password or passphrase prompt
            // would go to a terminal that is not there. BatchMode turns that
            // hang into an immediate, reportable failure. Key and agent auth,
            // which is what `xs` over SSH is set up for, are unaffected.
            .arg("-o")
            .arg("BatchMode=yes");
        if let Some(username) = target.username {
            command.arg("-l").arg(username);
        }
        command
            .arg(target.host)
            .arg(format!(
                "PATH=\"$HOME/.local/bin:$PATH\" 'xs' '--server' '{}'",
                root.replace('\'', "'\\''")
            ))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
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
        let client = Client::from_split(stdout, stdin, 0)
            .await
            .map_err(|error| map_error(&error))?;
        Self::mount(client, root).await
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
        Ok(Self {
            mount,
            _client: Arc::new(client),
            root_label: root.to_owned(),
        })
    }

    /// The export root, for the status bar's plain-language restatement.
    #[must_use]
    pub fn root_label(&self) -> &str {
        &self.root_label
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
            capacity: None,
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
        let usage = self
            .mount
            .statfs()
            .await
            .map_err(|error| map_error(&error))?;
        Ok(Capacity {
            // `available` rather than `free`: the root-only reserve is not
            // space this session can use, so showing it would overstate.
            free: usage.available_bytes,
            total: usage.total_bytes,
        })
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

        let mut stage = self
            .mount
            .stage(path.as_bytes(), data.len() as u64, 0o644, b"")
            .await
            .map_err(|error| map_error(&error))?;
        let max = stage.max_write() as usize;
        let mut offset = 0_usize;
        // Chunked to the mount's own limit: a whole file in one message is
        // refused above `max_write`, and an editor's save is not always small.
        while offset < data.len() {
            let end = data.len().min(offset + max);
            stage
                .write(offset as u64, &data[offset..end])
                .await
                .map_err(|error| map_error(&error))?;
            offset = end;
        }
        if data.is_empty() {
            stage.write(0, &[]).await.ok();
        }

        let digest = *blake3::hash(data).as_bytes();
        match stage
            .commit(digest, expect_cookie, None)
            .await
            .map_err(|error| map_error(&error))?
        {
            Ok(attrs) => Ok(CasOutcome::Written(attrs_of(&attrs))),
            Err(current) => Ok(CasOutcome::Changed(attrs_of(&current))),
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
