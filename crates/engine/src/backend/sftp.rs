//! The SFTP backend (E6).
//!
//! SFTP over SSH, with the same host-key trust store the xsync backend uses.
//! That sharing is deliberate and load-bearing: a user who has accepted a
//! host's key for one protocol should not be asked again for another, and —
//! more importantly — a key that *changed* must be refused whichever protocol
//! notices it first.
//!
//! Unlike the xsync backend, which spawns `ssh`, this speaks SSH itself
//! through `russh`. That means the host key arrives during the handshake
//! rather than from a separate `ssh-keyscan`, so there is no scan-then-connect
//! window at all here — the key being checked *is* the key of the connection
//! being made.
//!
//! This drives `russh-sftp`'s [`RawSftpSession`] rather than its friendlier
//! `SftpSession`. The high-level type wraps each open file in an `AsyncRead`,
//! which forces a seek-then-read pair per range and serialises everything
//! behind one cursor. The raw session carries the offset in the request, which
//! is what SFTP actually does, and that buys three things the friendly API
//! cannot express: reads pipelined across the round trip, the server's
//! extension map, and `posix-rename@openssh.com`.

use std::sync::Arc;

use async_trait::async_trait;
use russh::client::{self, Handler};
use russh::keys::PublicKeyBase64;
use russh_sftp::client::rawsession::Limits;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::task::JoinSet;

use super::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, MountStatus, ShareBackend,
    WriteMode,
};
use crate::error::{EngineError, Result};
use crate::hostkeys::{self, HostKey, KnownHosts, Trust};
use crate::kind::classify_entry;

/// `ST_RDONLY` in the statvfs mount flags, which is how a server says the
/// filesystem itself refuses writes.
const ST_RDONLY: u64 = 0x1;

/// OpenSSH's atomic replacing rename. Its payload is two strings, the same
/// shape as `hardlink@openssh.com`.
const POSIX_RENAME: &str = "posix-rename@openssh.com";

/// One read request's worth of bytes.
const READ_CHUNK: u32 = 256 * 1024;

/// How many reads are in flight at once.
///
/// SFTP is a request-response protocol over one channel, so a sequential
/// reader spends a round trip per chunk and tops out at `chunk / RTT`
/// regardless of the link. Eight outstanding requests is what OpenSSH's own
/// client uses, and it is enough to keep a long fat pipe busy.
const READ_PIPELINE: usize = 8;

/// The two-string payload `posix-rename@openssh.com` takes.
#[derive(serde::Serialize)]
struct PosixRename {
    oldpath: String,
    newpath: String,
}

/// How a caller proves who it is.
#[derive(Debug, Clone)]
pub enum Credential {
    /// Whatever the running `ssh-agent` holds.
    Agent,
    /// A private key file, with an optional passphrase.
    PrivateKey {
        /// Path to the key.
        path: String,
        /// Passphrase, when it is encrypted.
        passphrase: Option<String>,
    },
    /// A password, offered by the `password` method and then by
    /// `keyboard-interactive` — a server with `PasswordAuthentication no` and
    /// `KbdInteractiveAuthentication yes` accepts only the second, and that is
    /// a common default.
    Password(String),
}

/// Everything needed to reach an SFTP server.
#[derive(Debug, Clone)]
pub struct SftpTarget<'a> {
    /// Host as the user typed it. Also the name the key is trusted under.
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    /// The directory the mount is rooted at.
    pub root: &'a str,
    /// What the user asked for; the server decides what they get.
    pub read_only: bool,
}

/// A mounted SFTP directory.
pub struct SftpBackend {
    /// Behind an `Arc` so pipelined reads can be spawned against it.
    sftp: Arc<RawSftpSession>,
    /// Kept so the SSH session outlives every request through it.
    _session: client::Handle<TrustGate>,
    root: String,
    info: MountInfo,
    /// The largest read this server will answer, which `limits@openssh.com`
    /// may cap below [`READ_CHUNK`].
    read_chunk: u32,
    /// The largest write it will accept.
    write_chunk: usize,
}

/// The host-key check, run during the handshake.
///
/// Records its verdict rather than only returning a bool, because `russh`'s
/// callback can say yes or no and nothing else — and "no" needs to reach the
/// user as *why*, which is the whole point of E3-S4.
struct TrustGate {
    known: Arc<KnownHosts>,
    host: String,
    port: u16,
    verdict: Arc<std::sync::Mutex<Option<Trust>>>,
}

impl Handler for TrustGate {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // A certificate needs a CA to validate against, which this does
            // not have. Refusing is the honest answer rather than trusting it.
            return Ok(false);
        };
        let offered = HostKey {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            key: key.public_key_base64(),
        };
        // The same decision the xsync path reaches, through the same function.
        let trust = match hostkeys::check(
            &self.known,
            &self.host,
            self.port,
            std::slice::from_ref(&offered),
        ) {
            Ok(trust) => trust,
            // A trust store that cannot be read is not a reason to trust
            // the key in front of it.
            Err(_) => Trust::Unknown { offered },
        };
        let trusted = matches!(trust, Trust::Trusted);
        *self
            .verdict
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(trust);
        Ok(trusted)
    }
}

impl SftpBackend {
    /// Connect, authenticate, and open the SFTP subsystem.
    ///
    /// # Errors
    ///
    /// Fails when the host cannot be reached, its key is not trusted — with
    /// the same typed errors E3-S4 defined, so the window shows a trust prompt
    /// rather than a generic failure — authentication is rejected, or the root
    /// is not a directory.
    pub async fn connect(
        target: &SftpTarget<'_>,
        credential: &Credential,
        known: Arc<KnownHosts>,
    ) -> Result<Self> {
        let verdict = Arc::new(std::sync::Mutex::new(None));
        let gate = TrustGate {
            known,
            host: target.host.to_owned(),
            port: target.port,
            verdict: Arc::clone(&verdict),
        };
        let config = Arc::new(client::Config::default());
        let mut session = match client::connect(config, (target.host, target.port), gate).await {
            Ok(session) => session,
            Err(error) => {
                // A refused key surfaces here as a handshake failure, so
                // the recorded verdict is what turns it into something the
                // window can act on.
                return Err(host_key_error(&verdict).unwrap_or_else(|| {
                    EngineError::Disconnected(format!("{}:{}: {error}", target.host, target.port))
                }));
            }
        };

        authenticate(&mut session, target.username, credential).await?;

        let channel = session
            .channel_open_session()
            .await
            .map_err(|error| EngineError::Disconnected(format!("open a channel: {error}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| EngineError::Unsupported(format!("no sftp subsystem: {error}")))?;

        let mut sftp = RawSftpSession::new(channel.into_stream());
        // `RawSftpSession` does not handshake on construction the way the
        // high-level session does, and every request before this would be
        // answered by a server that has not agreed a version.
        let version = sftp.init().await.map_err(|error| map_error(&error))?;
        let advertises =
            |name: &str, value: &str| version.extensions.get(name).is_some_and(|v| v == value);
        let posix_rename = advertises(POSIX_RENAME, "1");

        // Ask for the server's ceilings before sending anything large. Without
        // this the client's own guard is unset and an oversized read is
        // refused by the server instead, mid-transfer.
        let (mut read_chunk, mut write_chunk) = (READ_CHUNK, READ_CHUNK as usize);
        if advertises("limits@openssh.com", "1") {
            if let Ok(limits) = sftp.limits().await {
                let limits = Limits::from(limits);
                if let Some(read) = limits.read_len {
                    read_chunk = read_chunk.min(u32::try_from(read).unwrap_or(u32::MAX));
                }
                if let Some(write) = limits.write_len {
                    write_chunk = write_chunk.min(usize::try_from(write).unwrap_or(usize::MAX));
                }
                sftp.set_limits(limits);
            }
        }
        let sftp = Arc::new(sftp);

        let root = normalise_root(&sftp, target.root).await?;
        let info = probe_mount(&sftp, &root, target.read_only, posix_rename).await?;
        Ok(Self {
            sftp,
            _session: session,
            root,
            info,
            read_chunk: read_chunk.max(1),
            write_chunk: write_chunk.max(1),
        })
    }

    /// A path on the server, from one relative to the mount.
    fn absolute(&self, path: &str) -> String {
        let relative = path.trim_matches('/');
        if relative.is_empty() {
            self.root.clone()
        } else {
            format!("{}/{relative}", self.root.trim_end_matches('/'))
        }
    }

    /// Open a file and hand back the server's handle for it.
    async fn open_handle(&self, path: &str, flags: OpenFlags) -> Result<String> {
        self.sftp
            .open(self.absolute(path), flags, FileAttributes::default())
            .await
            .map(|handle| handle.handle)
            .map_err(|error| map_error(&error))
    }

    /// Every entry in a directory, across as many `readdir` calls as it takes.
    ///
    /// A server answers with as many names as fit in a packet and expects to
    /// be asked again; stopping after the first reply silently truncates any
    /// directory bigger than that.
    async fn read_dir_raw(&self, path: &str) -> Result<Vec<russh_sftp::protocol::File>> {
        let handle = self
            .sftp
            .opendir(self.absolute(path))
            .await
            .map_err(|error| map_error(&error))?
            .handle;
        let mut files = Vec::new();
        let outcome = loop {
            match self.sftp.readdir(handle.clone()).await {
                Ok(name) => files.extend(name.files),
                Err(error) if is_eof(&error) => break Ok(()),
                Err(error) => break Err(map_error(&error)),
            }
        };
        let _ = self.sftp.close(handle).await;
        outcome.map(|()| files)
    }

    /// Read one range, split across several requests in flight at once.
    async fn read_pipelined(&self, handle: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut start = 0_u32;
        while start < length {
            chunks.push((start, self.read_chunk.min(length - start)));
            start = start.saturating_add(self.read_chunk);
        }

        let mut done: Vec<Option<Vec<u8>>> = vec![None; chunks.len()];
        let mut running: JoinSet<(usize, Result<Vec<u8>>)> = JoinSet::new();
        let mut next = 0_usize;
        loop {
            while running.len() < READ_PIPELINE && next < chunks.len() {
                let (start, want) = chunks[next];
                let index = next;
                let sftp = Arc::clone(&self.sftp);
                let handle = handle.to_owned();
                let at = offset + u64::from(start);
                running.spawn(async move { (index, read_chunk(&sftp, &handle, at, want).await) });
                next += 1;
            }
            let Some(joined) = running.join_next().await else {
                break;
            };
            let (index, bytes) =
                joined.map_err(|error| EngineError::Io(format!("read task: {error}")))?;
            done[index] = Some(bytes?);
        }

        // Concatenate only up to the first chunk that came back short. Past
        // that point is end of file, and a later chunk that happens to hold
        // bytes would put them at the wrong offset.
        let mut bytes = Vec::new();
        for (index, (_, want)) in chunks.iter().enumerate() {
            let chunk = done[index].take().unwrap_or_default();
            let short = chunk.len() < *want as usize;
            bytes.extend_from_slice(&chunk);
            if short {
                break;
            }
        }
        Ok(bytes)
    }
}

/// One chunk, re-requesting until it is filled or the file ends.
async fn read_chunk(
    sftp: &RawSftpSession,
    handle: &str,
    offset: u64,
    want: u32,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(want as usize);
    while bytes.len() < want as usize {
        let filled = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        match sftp
            .read(handle.to_owned(), offset + u64::from(filled), want - filled)
            .await
        {
            // A server may answer with less than was asked for even mid-file,
            // so a short reply is a reason to ask again, not to stop.
            Ok(data) if data.data.is_empty() => break,
            Ok(data) => bytes.extend_from_slice(&data.data),
            Err(error) if is_eof(&error) => break,
            Err(error) => return Err(map_error(&error)),
        }
    }
    Ok(bytes)
}

/// Whether an error is the server saying "that is the end of the file".
///
/// `Eof` is a status like any other on the wire, so it arrives as an error
/// and has to be told apart from one.
fn is_eof(error: &russh_sftp::client::error::Error) -> bool {
    matches!(
        error,
        russh_sftp::client::error::Error::Status(status)
            if status.status_code == StatusCode::Eof
    )
}
/// The typed error for a refused host key, if that is what happened.
fn host_key_error(verdict: &Arc<std::sync::Mutex<Option<Trust>>>) -> Option<EngineError> {
    let trust = verdict
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    match trust {
        Trust::Trusted => None,
        Trust::Unknown { offered } => Some(EngineError::HostKeyUnknown {
            host: offered.host.clone(),
            port: offered.port,
            algorithm: offered.algorithm.clone(),
            fingerprint: offered.fingerprint(),
        }),
        Trust::Changed { offered, stored } => Some(EngineError::HostKeyChanged {
            host: offered.host.clone(),
            port: offered.port,
            algorithm: offered.algorithm.clone(),
            fingerprint: offered.fingerprint(),
            expected: stored.fingerprint(),
        }),
        Trust::Revoked { offered } => Some(EngineError::HostKeyChanged {
            host: offered.host.clone(),
            port: offered.port,
            algorithm: offered.algorithm.clone(),
            fingerprint: offered.fingerprint(),
            expected: "a key you revoked".to_owned(),
        }),
    }
}

/// Try the credential the caller gave, and say plainly when it is rejected.
async fn authenticate(
    session: &mut client::Handle<TrustGate>,
    username: &str,
    credential: &Credential,
) -> Result<()> {
    let accepted = match credential {
        Credential::Agent => {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|error| {
                    EngineError::AuthenticationFailed(format!("no ssh-agent: {error}"))
                })?;
            let identities = agent.request_identities().await.map_err(|error| {
                EngineError::AuthenticationFailed(format!("agent has no keys: {error}"))
            })?;
            let mut accepted = false;
            for identity in identities {
                // Each identity in turn: an agent commonly holds several and
                // only one of them is for this host. A certificate needs a CA
                // to validate against, which this does not have.
                let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = identity else {
                    continue;
                };
                match session
                    .authenticate_publickey_with(username, key, None, &mut agent)
                    .await
                {
                    Ok(result) if result.success() => {
                        accepted = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return Err(EngineError::AuthenticationFailed(format!("agent: {error}")))
                    }
                }
            }
            accepted
        }
        Credential::PrivateKey { path, passphrase } => {
            let key = russh::keys::load_secret_key(path, passphrase.as_deref())
                .map_err(|error| EngineError::AuthenticationFailed(format!("{path}: {error}")))?;
            // `None` maps an RSA key to the legacy SHA-1 signature, which
            // modern servers refuse, so the server's own preference is asked
            // for rather than assumed.
            let hash = session
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            session
                .authenticate_publickey(username, key)
                .await
                .map_err(|error| EngineError::AuthenticationFailed(error.to_string()))?
                .success()
        }
        Credential::Password(password) => {
            if session
                .authenticate_password(username, password)
                .await
                .map_err(|error| EngineError::AuthenticationFailed(error.to_string()))?
                .success()
            {
                true
            } else {
                keyboard_interactive(session, username, password).await?
            }
        }
    };
    if accepted {
        Ok(())
    } else {
        // Deliberately not retried here. E15-S2's guard stops the session
        // manager looping on this, and a client that retried internally would
        // defeat that from underneath.
        Err(EngineError::AuthenticationFailed(format!(
            "the server rejected authentication for {username}"
        )))
    }
}

/// Answer a `keyboard-interactive` exchange with the one password we hold.
///
/// A server may ask any number of times and for any number of prompts. Every
/// prompt gets the same answer, which is what an `ssh` client does when it is
/// not attached to a terminal — this backend has no way to ask a person, since
/// the password reached Rust once from the connection dialog and E10's rule is
/// that it never travels back out to be prompted for again.
async fn keyboard_interactive(
    session: &mut client::Handle<TrustGate>,
    username: &str,
    password: &str,
) -> Result<bool> {
    use russh::client::KeyboardInteractiveAuthResponse as Response;

    let mut response = session
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|error| EngineError::AuthenticationFailed(error.to_string()))?;
    // Bounded: a server that keeps asking would otherwise hold this loop open
    // for as long as it liked.
    for _ in 0..16 {
        match response {
            Response::Success => return Ok(true),
            Response::Failure { .. } => return Ok(false),
            Response::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_owned()).collect();
                response = session
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|error| EngineError::AuthenticationFailed(error.to_string()))?;
            }
        }
    }
    Ok(false)
}

/// Resolve the mount root to an absolute path the server agrees with.
async fn normalise_root(sftp: &RawSftpSession, root: &str) -> Result<String> {
    let requested = if root.is_empty() { "." } else { root };
    let resolved = sftp
        .realpath(requested)
        .await
        .map_err(|error| map_error(&error))?
        .files
        .into_iter()
        .next()
        .ok_or_else(|| EngineError::NotFound(format!("{requested} does not resolve")))?
        .filename;
    let attrs = sftp
        .stat(resolved.clone())
        .await
        .map_err(|error| map_error(&error))?
        .attrs;
    if !attrs.is_dir() {
        return Err(EngineError::Unsupported(format!(
            "{resolved} is not a directory"
        )));
    }
    Ok(resolved)
}

/// Work out what this mount allows, once, at connect time.
///
/// Three terms, and the reason string names whichever one failed — a disabled
/// Upload button with no explanation is the thing E9 exists to prevent.
/// Returns `Err` rather than a read-only mount when the transport dies during
/// the probe: a server that refuses the write has told us something, and a
/// connection that broke has told us nothing. Reporting the second as the
/// first silently strips a share's write affordances until the next reconnect.
async fn probe_mount(
    sftp: &RawSftpSession,
    root: &str,
    read_only_requested: bool,
    posix_rename: bool,
) -> Result<MountInfo> {
    let statvfs = sftp.statvfs(root.to_owned()).await.ok();
    let capacity = statvfs.as_ref().map(|fs| Capacity {
        free: fs.blocks_avail.saturating_mul(fs.fragment_size),
        total: fs.blocks.saturating_mul(fs.fragment_size),
    });

    let filesystem_read_only = statvfs.as_ref().is_some_and(|fs| fs.flags & ST_RDONLY != 0);

    let (writable, reason) = if read_only_requested {
        (false, "Read-only — you asked for it".to_owned())
    } else if filesystem_read_only {
        (false, "Read-only — filesystem is read-only".to_owned())
    } else {
        // The statvfs flag is about the filesystem; permission is about this
        // user. A create-probe is the only thing that answers the second, and
        // it is the question that actually decides whether a write works.
        match probe_write(sftp, root).await {
            Ok(()) => (true, String::new()),
            // The mount is gone, not read-only. Failing the connection sends
            // the session manager back round its retry loop.
            Err(error) if matches!(error, EngineError::Disconnected(_)) => return Err(error),
            Err(error) => (
                false,
                format!(
                    "Read-only — {} on `{root}`",
                    error.to_string().to_lowercase()
                ),
            ),
        }
    };

    Ok(MountInfo {
        status: MountStatus::Ready,
        writable,
        reason,
        protocol_label: "SFTP".to_owned(),
        // The negotiated cipher is not exposed by russh's public API, so this
        // says what it knows rather than inventing a name. E6-S2 wants the
        // KEX/cipher here and it needs an upstream accessor.
        security_label: "ssh".to_owned(),
        options: String::new(),
        capacity,
        caps: Caps {
            random_access: true,
            resume_write: true,
            // True only where the server offers `posix-rename@openssh.com`.
            // Plain SFTPv3 rename refuses an existing destination, so a
            // replace there takes more than one step and the transfer worker
            // must use its safe path instead.
            atomic_rename: posix_rename,
            symlinks: true,
            watch: false,
        },
    })
}

/// Create and remove a file, to learn whether this user may write here.
async fn probe_write(sftp: &RawSftpSession, root: &str) -> Result<()> {
    let probe = format!("{}/.excalibur-write-probe", root.trim_end_matches('/'));
    match sftp
        .open(
            probe.clone(),
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
            FileAttributes::default(),
        )
        .await
    {
        Ok(handle) => {
            let _ = sftp.close(handle.handle).await;
            // Best effort: a probe left behind is untidy, not incorrect, and
            // failing the mount because cleanup failed would be worse.
            let _ = sftp.remove(probe).await;
            Ok(())
        }
        Err(error) => Err(map_error(&error)),
    }
}

#[async_trait]
impl ShareBackend for SftpBackend {
    fn mount_info(&self) -> MountInfo {
        self.info.clone()
    }

    fn caps(&self) -> Caps {
        self.info.caps
    }

    async fn capacity(&self) -> Result<Capacity> {
        let statvfs = self
            .sftp
            .statvfs(self.root.clone())
            .await
            .map_err(|error| map_error(&error))?;
        Ok(Capacity {
            free: statvfs.blocks_avail.saturating_mul(statvfs.fragment_size),
            total: statvfs.blocks.saturating_mul(statvfs.fragment_size),
        })
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let files = self.read_dir_raw(path).await?;
        let relative = path.trim_matches('/').to_owned();

        let mut rows = Vec::new();
        for file in files {
            let name = file.filename;
            if name == "." || name == ".." {
                continue;
            }
            let attrs = attrs_of(&file.attrs);
            let kind = classify_entry(&name, attrs.kind);
            rows.push(Entry {
                path: if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                },
                hidden: name.starts_with('.'),
                name,
                attrs,
                kind_label: kind.label,
                kind_category: kind.category,
            });
        }
        Ok(Listing {
            entries: rows,
            mount: self.info.clone(),
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let attrs = self
            .sftp
            .stat(self.absolute(path))
            .await
            .map_err(|error| map_error(&error))?
            .attrs;
        Ok(attrs_of(&attrs))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let handle = self.open_handle(path, OpenFlags::READ).await?;
        // The offset travels in each request, so several are in flight at
        // once and no lock is needed: there is no shared cursor to protect.
        let bytes = self.read_pipelined(&handle, offset, length).await;
        let _ = self.sftp.close(handle).await;
        bytes
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        if !self.info.writable {
            return Err(EngineError::ReadOnly(self.info.reason.clone()));
        }
        // Positional, never APPEND: a resumed upload has to land at the offset
        // it stopped at, and APPEND would put it at whatever the length
        // happens to be — which is the same thing only when nothing else has
        // touched the file.
        let (flags, offset) = match mode {
            WriteMode::Create => (
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
                0,
            ),
            WriteMode::Resume { offset } => (OpenFlags::CREATE | OpenFlags::WRITE, offset),
        };
        let handle = self.open_handle(path, flags).await?;
        // Sequential, unlike reads: a pipelined write that failed halfway
        // would leave holes, and the transfer worker resumes from a byte
        // count that has to mean "everything before this is on disk".
        let mut written = 0_usize;
        let outcome = loop {
            if written >= data.len() {
                break Ok(());
            }
            let end = data.len().min(written + self.write_chunk);
            match self
                .sftp
                .write(
                    handle.clone(),
                    offset + written as u64,
                    data[written..end].to_vec(),
                )
                .await
            {
                Ok(_) => written = end,
                Err(error) => break Err(map_error(&error)),
            }
        };
        let _ = self.sftp.close(handle).await;
        outcome.map(|()| written as u64)
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        self.sftp
            .mkdir(self.absolute(path), FileAttributes::default())
            .await
            .map(|_| ())
            .map_err(|error| map_error(&error))
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        let from = self.absolute(from);
        let to = self.absolute(to);
        if !replace {
            // SFTPv3's own rename refuses an existing destination, which is
            // exactly this case, so nothing extra is needed.
            return self
                .sftp
                .rename(from, to)
                .await
                .map(|_| ())
                .map_err(|error| map_error(&error));
        }
        if self.info.caps.atomic_rename {
            // One request, and the server does it with `rename(2)`: the
            // destination is never absent, not even for an instant.
            return self.posix_rename(&from, &to).await;
        }
        if self.sftp.rename(from.clone(), to.clone()).await.is_ok() {
            return Ok(());
        }

        // The obvious fallback — remove the destination, then rename onto it —
        // has a window in which the destination does not exist at all, so a
        // crash or a dropped connection there destroys the file the user was
        // replacing. The sibling Kestrel engine does exactly that.
        //
        // Standing the old one aside instead means the destination is absent
        // only for the instant between two renames, and a failure can put it
        // back.
        let stashed = format!("{to}.excalibur-replacing");
        let had_destination = self.sftp.rename(to.clone(), stashed.clone()).await.is_ok();
        match self.sftp.rename(from, to.clone()).await {
            Ok(_) => {
                if had_destination {
                    // Best effort: the replacement is already in place, and
                    // failing now would report a failure that did not happen.
                    let _ = self.sftp.remove(stashed).await;
                }
                Ok(())
            }
            Err(error) => {
                if had_destination {
                    let _ = self.sftp.rename(stashed, to).await;
                }
                Err(map_error(&error))
            }
        }
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let absolute = self.absolute(path);
        let attrs = self
            .sftp
            .stat(absolute.clone())
            .await
            .map_err(|error| map_error(&error))?
            .attrs;
        if !attrs.is_dir() {
            return self
                .sftp
                .remove(absolute)
                .await
                .map(|_| ())
                .map_err(|error| map_error(&error));
        }
        if !recursive {
            return self
                .sftp
                .rmdir(absolute)
                .await
                .map(|_| ())
                .map_err(|error| map_error(&error));
        }
        // Depth first, and iteratively: an `async fn` that calls itself needs
        // boxing, and a deep tree would be bounded by the stack rather than by
        // the share.
        let mut directories = vec![path.trim_matches('/').to_owned()];
        let mut index = 0;
        while index < directories.len() {
            let directory = directories[index].clone();
            index += 1;
            for entry in self.list_dir(&directory).await?.entries {
                if entry.attrs.kind == EntryKind::Dir {
                    directories.push(entry.path);
                } else {
                    self.sftp
                        .remove(self.absolute(&entry.path))
                        .await
                        .map_err(|error| map_error(&error))?;
                }
            }
        }
        for directory in directories.into_iter().rev() {
            self.sftp
                .rmdir(self.absolute(&directory))
                .await
                .map_err(|error| map_error(&error))?;
        }
        Ok(())
    }

    async fn ping(&self) -> Result<Health> {
        match self.sftp.stat(self.root.clone()).await {
            Ok(_) => Ok(Health::Healthy),
            Err(_) => Ok(Health::Failed),
        }
    }
}

impl SftpBackend {
    /// `posix-rename@openssh.com`: replace the destination in one step.
    async fn posix_rename(&self, from: &str, to: &str) -> Result<()> {
        let payload = russh_sftp::ser::to_bytes(&PosixRename {
            oldpath: from.to_owned(),
            newpath: to.to_owned(),
        })
        .map_err(|error| EngineError::Io(format!("encode posix-rename: {error}")))?;
        match self.sftp.extended(POSIX_RENAME, payload.to_vec()).await {
            Ok(russh_sftp::protocol::Packet::Status(status))
                if status.status_code == StatusCode::Ok =>
            {
                Ok(())
            }
            Ok(russh_sftp::protocol::Packet::Status(status)) => {
                Err(map_error(&russh_sftp::client::error::Error::Status(status)))
            }
            Ok(_) => Err(EngineError::Io(
                "the server answered posix-rename with something else".to_owned(),
            )),
            Err(error) => Err(map_error(&error)),
        }
    }
}

/// SFTP metadata as the engine describes a file.
fn attrs_of(metadata: &FileAttributes) -> Attrs {
    let kind = if metadata.is_dir() {
        EntryKind::Dir
    } else if metadata.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_regular() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Attrs {
        kind,
        size: metadata.size.unwrap_or(0),
        modified_ms: metadata.mtime.map(|seconds| i64::from(seconds) * 1000),
        mode: metadata.permissions,
        link_target: None,
        // SFTPv3 has nothing like it, so edit-in-place falls back to comparing
        // size and mtime here. Weaker than xsync's cookie, and honest about it.
        change_cookie: None,
    }
}

/// Translate an SFTP error into the engine's taxonomy.
///
/// By status code rather than by message: the text is the server's and varies
/// between implementations, while the code is the protocol's.
fn map_error(error: &russh_sftp::client::error::Error) -> EngineError {
    let message = error.to_string();
    match error {
        russh_sftp::client::error::Error::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => EngineError::NotFound(message),
            StatusCode::PermissionDenied => EngineError::PermissionDenied(message),
            StatusCode::OpUnsupported => EngineError::Unsupported(message),
            StatusCode::Failure => {
                // SFTPv3 collapses most of the interesting cases into
                // `Failure`, so the message is the only discriminator left.
                if message.contains("exists") {
                    EngineError::AlreadyExists(message)
                } else {
                    EngineError::Io(message)
                }
            }
            _ => EngineError::Io(message),
        },
        _ => EngineError::Io(message),
    }
}
