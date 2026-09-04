//! The SMB backend (E4) — **optional, and GPL-3.0-or-later when enabled**.
//!
//! Compiled only under the `smb` feature, which is off by default. That is not
//! a convenience: this module links `libsmbclient`, which is
//! **GPL-3.0-or-later** — not LGPL, despite what this project's risk register
//! said for a while. Linking it, statically or dynamically, makes the shipped
//! executable a combined work conveyable only under GPLv3 as a whole. Every
//! recipient then gets GPLv3 rights over all of Excalibur, and the Mac App
//! Store is closed to it (GPLv3 §6/§7, the VLC precedent).
//!
//! So the feature is the licence boundary. `cargo build` produces an
//! `MIT OR Apache-2.0` binary with no SMB; `cargo build --features smb`
//! produces a GPLv3 binary with it. See `docs/decisions/0001-smb-client.md`.
//!
//! It also requires `libsmbclient` at build and run time — `brew install samba`
//! or `libsmbclient-dev` — which is the other reason not to make it default.
//!
//! ## What this backend cannot tell you
//!
//! `libsmbclient` exposes no way to read back what was negotiated. Not the
//! dialect, not the signing state, not the encryption cipher: the setters are
//! policy inputs and the getters echo them back. So E4-S2's `security_label`
//! and `protocol_label` cannot report observed facts the way the xsync and NFS
//! backends do. What they report instead is stated below, and it is narrower
//! on purpose.
//!
//! ## Threading
//!
//! `pavao` serialises **every** libsmbclient call in the process behind one
//! static mutex — not per client, not per file. So there is no concurrency to
//! be had here at any level, and `spawn_blocking` is used purely to keep the
//! synchronous C library off the async runtime's threads rather than to
//! overlap anything.

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use async_trait::async_trait;
use pavao::{
    SmbClient, SmbCredentials, SmbDialect, SmbDirentType, SmbEncryptionLevel, SmbMode,
    SmbOpenOptions, SmbOptions, SmbStat,
};

use super::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, MountStatus, ShareBackend,
    WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// The DOS hidden attribute, which is how SMB marks a hidden file — the
/// leading-dot convention is a Unix one and does not travel.
const FILE_ATTRIBUTE_HIDDEN: u16 = 0x02;

/// Everything needed to reach a share.
#[derive(Debug, Clone)]
pub struct SmbTarget<'a> {
    pub host: &'a str,
    /// Ignored unless it is the default: `libsmbclient` offers no way to set
    /// the port, so a non-standard one cannot be reached. Named rather than
    /// silently dropped.
    pub port: Option<u16>,
    pub share: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    /// The Windows domain, or empty.
    pub workgroup: &'a str,
    /// What the user asked for; the server decides what they get.
    pub read_only: bool,
}

/// A mounted SMB share.
pub struct SmbBackend {
    /// Behind an `Arc` so each blocking call can own a handle. There is no
    /// parallelism to gain — the library serialises internally — but the
    /// runtime's worker threads should not be the ones blocking on the network.
    client: Arc<SmbClient>,
    info: MountInfo,
}

impl SmbBackend {
    /// Connect and work out what the share allows.
    ///
    /// # Errors
    ///
    /// Fails when the server cannot be reached, the credentials are refused,
    /// or the share does not exist.
    pub async fn connect(target: &SmbTarget<'_>) -> Result<Self> {
        if target.port.is_some_and(|port| port != 445) {
            return Err(EngineError::Unsupported(
                "libsmbclient always uses port 445 and offers no way to change it".to_owned(),
            ));
        }
        let credentials = SmbCredentials::default()
            .server(format!("smb://{}", target.host))
            .share(format!("/{}", target.share.trim_matches('/')))
            .username(target.username)
            .password(target.password)
            .workgroup(target.workgroup);

        // SMB1 is never negotiated. The floor is 2.0.2 and the ceiling 3.1.1,
        // which is E4-S2's range; the dialect actually chosen cannot be read
        // back, so this bound is the only guarantee available — and it is the
        // one that matters, because it is what excludes SMB1.
        //
        // Encryption is *requested* rather than required: requiring it would
        // refuse SMB 2.x servers outright, and the AC lists 2.1 as negotiable.
        // What that costs is honesty about the label — see `security_label`.
        let options = SmbOptions::default()
            .min_protocol(SmbDialect::Smb202)
            .max_protocol(SmbDialect::Smb311)
            .encryption_level(SmbEncryptionLevel::Request)
            .no_auto_anonymous_login(true)
            .one_share_per_server(true);

        let client = tokio::task::spawn_blocking(move || SmbClient::new(credentials, options))
            .await
            .map_err(|error| EngineError::Io(format!("smb connect task: {error}")))?
            .map_err(|error| map_error(&error))?;
        let client = Arc::new(client);

        let info = probe_mount(&client, target).await?;
        Ok(Self { client, info })
    }

    /// Run one libsmbclient call off the runtime's threads.
    async fn blocking<T, F>(&self, work: F) -> Result<T>
    where
        F: FnOnce(&SmbClient) -> std::result::Result<T, pavao::SmbError> + Send + 'static,
        T: Send + 'static,
    {
        let client = Arc::clone(&self.client);
        tokio::task::spawn_blocking(move || work(&client))
            .await
            .map_err(|error| EngineError::Io(format!("smb task: {error}")))?
            .map_err(|error| map_error(&error))
    }
}

/// A path as libsmbclient wants it: absolute within the share.
fn share_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        format!("/{trimmed}")
    }
}

/// Work out what this mount allows, once, at connect time.
///
/// Returns `Err` rather than a read-only mount when the transport dies during
/// the probe. That distinction is the whole of this function's difficulty: a
/// server that refuses the write has told us something, and a connection that
/// breaks has told us nothing. Reporting "nothing" as "read-only" is how a
/// share silently loses its write affordances after a blip and keeps them off
/// until the next reconnect — which was observed, not imagined: a reconnect
/// landing on a server that was still coming back up produced exactly that.
async fn probe_mount(client: &Arc<SmbClient>, target: &SmbTarget<'_>) -> Result<MountInfo> {
    let root = share_path("");
    let capacity = {
        let client = Arc::clone(client);
        let path = root.clone();
        tokio::task::spawn_blocking(move || client.statvfs(path.as_str()))
            .await
            .ok()
            .and_then(std::result::Result::ok)
            .map(|fs| {
                // Not the POSIX `blocks * frsize`. Against any server without
                // UNIX extensions — which is every Windows box and most NAS —
                // Samba packs `f_frsize` with sectors-per-allocation-unit and
                // `f_bsize` with bytes-per-sector, so the byte size is the
                // product of all three. Getting this wrong reports a 4 TB
                // share as a few gigabytes.
                let unit = fs.frsize.saturating_mul(fs.bsize);
                Capacity {
                    free: fs.bavail.saturating_mul(unit),
                    total: fs.blocks.saturating_mul(unit),
                }
            })
    };

    let (writable, reason) = if target.read_only {
        (false, "Read-only — you asked for it".to_owned())
    } else {
        // There is no share-level answer to ask for. `MaximalAccess` is not
        // exposed by libsmbclient, and the `ST_RDONLY` bit that `statvfs`
        // could carry is never set by Samba's implementation. So the only
        // thing that answers "may this user write here" is writing.
        match probe_write(client).await {
            Ok(()) => (true, String::new()),
            // The mount is gone, not read-only. Failing the connection sends
            // the session manager back round its retry loop, which is the
            // only thing that can actually resolve this.
            Err(error) if matches!(error, EngineError::Disconnected(_)) => return Err(error),
            Err(error) => (
                false,
                format!(
                    "Read-only — {} on `{}`",
                    error.to_string().to_lowercase(),
                    target.share
                ),
            ),
        }
    };

    Ok(MountInfo {
        status: MountStatus::Ready,
        writable,
        reason,
        // Not `SMB3`. The negotiated dialect cannot be read back, and naming a
        // version this code did not observe would be a guess presented as a
        // fact. What is true is the bound that was set.
        protocol_label: "SMB".to_owned(),
        // Likewise. Encryption was *requested*, and whether it was agreed is
        // not observable through this library, so the label says what was
        // asked for rather than claiming a cipher.
        security_label: "smb2+ · encryption requested".to_owned(),
        options: "dialects 2.0.2–3.1.1, SMB1 disabled".to_owned(),
        capacity,
        caps: Caps {
            random_access: true,
            resume_write: true,
            // `pavao`'s rename asks Samba to rename, and Samba on an existing
            // destination unlinks it first and then renames — the same window
            // E6-S3 rejected for SFTP, and here there is no way to avoid it.
            atomic_rename: false,
            symlinks: false,
            watch: false,
        },
    })
}

/// Create a file and remove it, to learn whether this user may write here.
async fn probe_write(client: &Arc<SmbClient>) -> Result<()> {
    let name = format!(
        "/.excalibur-write-probe-{}-{}",
        std::process::id(),
        PROBE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let client = Arc::clone(client);
    tokio::task::spawn_blocking(move || {
        let opened = client.open_with(
            name.as_str(),
            SmbOpenOptions::default()
                .create(true)
                .write(true)
                .exclusive(true),
        );
        match opened {
            Ok(file) => {
                drop(file);
                // Best effort: a probe left behind is untidy, not incorrect.
                let _ = client.unlink(name.as_str());
                Ok(())
            }
            Err(error) => Err(map_error(&error)),
        }
    })
    .await
    .unwrap_or_else(|error| Err(EngineError::Io(format!("smb probe task: {error}"))))
}

/// Distinguishes one write probe from another within this process.
static PROBE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[async_trait]
impl ShareBackend for SmbBackend {
    fn mount_info(&self) -> MountInfo {
        self.info.clone()
    }

    fn caps(&self) -> Caps {
        self.info.caps
    }

    async fn capacity(&self) -> Result<Capacity> {
        let fs = self.blocking(move |client| client.statvfs("/")).await?;
        let unit = fs.frsize.saturating_mul(fs.bsize);
        Ok(Capacity {
            free: fs.bavail.saturating_mul(unit),
            total: fs.blocks.saturating_mul(unit),
        })
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let directory = share_path(path);
        let entries = self
            .blocking(move |client| client.list_dirplus(directory.as_str()))
            .await?;
        let relative = path.trim_matches('/').to_owned();

        let mut rows = Vec::new();
        for entry in entries {
            let name = entry.name.clone();
            if name == "." || name == ".." {
                continue;
            }
            let kind = match entry.get_type() {
                SmbDirentType::Dir => EntryKind::Dir,
                _ => EntryKind::File,
            };
            let attrs = Attrs {
                kind,
                size: entry.size,
                modified_ms: system_time_ms(entry.mtime),
                mode: None,
                link_target: None,
                // SMB has no change cookie the client can see, so edit-in-place
                // falls back to comparing size and mtime.
                change_cookie: None,
            };
            let classified = classify_entry(&name, attrs.kind);
            rows.push(Entry {
                path: if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                },
                // The DOS attribute, not the leading dot: a hidden file on SMB
                // is one the server marks hidden, and the Unix convention does
                // not travel. Both are honoured, because a share written from
                // a Unix client will have dotfiles the server never flagged.
                hidden: entry.attrs & FILE_ATTRIBUTE_HIDDEN != 0 || name.starts_with('.'),
                name,
                attrs,
                kind_label: classified.label,
                kind_category: classified.category,
            });
        }
        Ok(Listing {
            entries: rows,
            mount: self.info.clone(),
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let target = share_path(path);
        let stat = self
            .blocking(move |client| client.stat(target.as_str()))
            .await?;
        Ok(attrs_of(&stat))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let target = share_path(path);
        self.blocking(move |client| {
            let mut file =
                client.open_with(target.as_str(), SmbOpenOptions::default().read(true))?;
            file.seek(SeekFrom::Start(offset))?;
            let mut buffer = vec![0_u8; length as usize];
            let mut filled = 0_usize;
            // Looping so a short read means end of file rather than a short
            // answer to a request the caller expected filled.
            while filled < buffer.len() {
                let read = file.read(&mut buffer[filled..])?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            buffer.truncate(filled);
            Ok(buffer)
        })
        .await
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        if !self.info.writable {
            return Err(EngineError::ReadOnly(self.info.reason.clone()));
        }
        let target = share_path(path);
        let payload = data.to_vec();
        let (create, truncate, offset) = match mode {
            WriteMode::Create => (true, true, 0),
            // Never APPEND: a resumed upload has to land where it stopped, and
            // append puts it at whatever the length happens to be.
            WriteMode::Resume { offset } => (true, false, offset),
        };
        self.blocking(move |client| {
            let mut file = client.open_with(
                target.as_str(),
                SmbOpenOptions::default()
                    .write(true)
                    .create(create)
                    .truncate(truncate),
            )?;
            if offset > 0 {
                file.seek(SeekFrom::Start(offset))?;
            }
            file.write_all(&payload)?;
            // `SmbFile::flush` is a documented no-op — libsmbclient has no
            // flush — so the close on drop is the only durability point, and
            // its error is swallowed by the library. Nothing here can fix
            // that; recorded rather than papered over.
            drop(file);
            Ok(payload.len() as u64)
        })
        .await
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let target = share_path(path);
        self.blocking(move |client| client.mkdir(target.as_str(), SmbMode::from(0o755)))
            .await
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        let source = share_path(from);
        let destination = share_path(to);
        if !replace {
            // Samba's rename clobbers an existing destination and offers no
            // no-clobber flag, so the check is a separate stat and therefore
            // racy. Named rather than hidden.
            let probe = destination.clone();
            if self
                .blocking(move |client| client.stat(probe.as_str()))
                .await
                .is_ok()
            {
                return Err(EngineError::AlreadyExists(format!("{to} already exists")));
            }
        }
        self.blocking(move |client| client.rename(source.as_str(), destination.as_str()))
            .await
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let attrs = self.stat(path).await?;
        if attrs.kind != EntryKind::Dir {
            let target = share_path(path);
            return self
                .blocking(move |client| client.unlink(target.as_str()))
                .await;
        }
        if !recursive {
            let target = share_path(path);
            return self
                .blocking(move |client| client.rmdir(target.as_str()))
                .await;
        }
        // Iteratively, depth first: an `async fn` that calls itself needs
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
                    let target = share_path(&entry.path);
                    self.blocking(move |client| client.unlink(target.as_str()))
                        .await?;
                }
            }
        }
        for directory in directories.into_iter().rev() {
            let target = share_path(&directory);
            self.blocking(move |client| client.rmdir(target.as_str()))
                .await?;
        }
        Ok(())
    }

    async fn ping(&self) -> Result<Health> {
        match self.blocking(move |client| client.stat("/")).await {
            Ok(_) => Ok(Health::Healthy),
            Err(_) => Ok(Health::Failed),
        }
    }
}

/// SMB metadata as the engine describes a file.
fn attrs_of(stat: &SmbStat) -> Attrs {
    // `SmbMode` carries the POSIX type bits Samba synthesises for the entry.
    let kind = if stat.mode.is_dir() {
        EntryKind::Dir
    } else if stat.mode.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    Attrs {
        kind,
        size: stat.size,
        modified_ms: system_time_ms(stat.modified),
        mode: None,
        link_target: None,
        change_cookie: None,
    }
}

/// A `SystemTime` as the engine's milliseconds, or `None` before the epoch.
fn system_time_ms(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
}

/// Translate a libsmbclient error into the engine's taxonomy.
///
/// Almost everything arrives as an `io::Error` carrying the errno the C
/// library set, so the errno is the discriminator rather than the message.
fn map_error(error: &pavao::SmbError) -> EngineError {
    let message = error.to_string();
    // A bad descriptor is what a dropped session looks like from here. The
    // library hands one back when the tree is gone — `STATUS_NETWORK_NAME_DELETED`
    // and a plain socket drop both arrive this way, and a directory listing
    // reports it where a `stat` on the same dead mount reports `ECONNREFUSED`.
    // Calling it an I/O error would put it on a transfer row; it belongs in the
    // banner, because the mount is what broke.
    if matches!(error, pavao::SmbError::BadFileDescriptor) {
        return EngineError::Disconnected(message);
    }
    let pavao::SmbError::Io(io) = error else {
        return EngineError::Io(message);
    };
    match io.kind() {
        std::io::ErrorKind::NotFound => EngineError::NotFound(message),
        std::io::ErrorKind::PermissionDenied => EngineError::PermissionDenied(message),
        std::io::ErrorKind::AlreadyExists => EngineError::AlreadyExists(message),
        std::io::ErrorKind::Unsupported => EngineError::Unsupported(message),
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::TimedOut => EngineError::Disconnected(message),
        // The rest by errno, through `libc` rather than by literal: the
        // numbers differ between macOS and Linux, so a literal would be right
        // on one and wrong on the other.
        _ => match io.raw_os_error() {
            // The share itself is read-only, as opposed to this user not being
            // allowed to write. `ErrorKind` has no variant for it.
            Some(libc::EROFS) => EngineError::ReadOnly(message),
            // `ENETRESET` is what `STATUS_NETWORK_NAME_DELETED` becomes — the
            // server dropped the tree out from under a live mount. `std` does
            // not categorise it, so without this it reads as a plain I/O
            // error and lands on a transfer row instead of the banner.
            Some(
                libc::ENETRESET
                | libc::ENETDOWN
                | libc::ENETUNREACH
                | libc::EHOSTDOWN
                | libc::EHOSTUNREACH
                | libc::ESHUTDOWN,
            ) => EngineError::Disconnected(message),
            _ => EngineError::Io(message),
        },
    }
}
