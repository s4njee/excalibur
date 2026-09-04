//! The `NFSv3` backend (E5).
//!
//! `nfs3_client` is a wire codec, not a client: it maps RFC 1813 procedures
//! one-to-one and stops there. No DNS, no connection pool, no retry, no path
//! resolution, no attribute translation. All of that is this module, which is
//! why it is longer than the SFTP backend despite doing less.
//!
//! Three consequences shape everything below.
//!
//! **Every call takes `&mut self`.** One connection answers one request at a
//! time — not by convention but structurally, since the socket is borrowed
//! mutably across send-then-receive and the transaction id is a plain counter
//! with no in-flight map. E5-S4 wants four reads in flight, so concurrency has
//! to come from *several connections*, and that is what [`Pool`] is for.
//!
//! **The source port must not be privileged.** The crate binds one in
//! 300..=1023 by default, which is what most servers want and what this program
//! must never do: it is not root, the bind fails with `EACCES`, and E5-S2 is
//! explicit that the answer is to explain the export option rather than to
//! reach for a privileged port. So the default is turned off, always.
//!
//! **The server's host must be an IP literal.** The crate parses it as one, so
//! a name resolves here first.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nfs3_client::nfs3_types::nfs3::{
    self, cookie3, cookieverf3, filename3, ftype3, nfsstat3, sattr3, Nfs3Option, Nfs3Result,
    ACCESS3_DELETE, ACCESS3_EXTEND, ACCESS3_MODIFY,
};
use nfs3_client::nfs3_types::rpc::{auth_unix, opaque_auth};
use nfs3_client::nfs3_types::xdr_codec::Opaque;
use nfs3_client::tokio::{TokioConnector, TokioIo};
use nfs3_client::{Nfs3Connection, Nfs3ConnectionBuilder};
use tokio::sync::Semaphore;

use super::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, MountStatus, ShareBackend,
    WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// One NFS connection, which is one TCP socket.
type Conn = Nfs3Connection<TokioIo<tokio::net::TcpStream>>;

/// The standard NFS port. `mountd` has no standard port and is normally found
/// through the portmapper.
pub const DEFAULT_NFS_PORT: u16 = 2049;

/// How many sockets a mount opens.
///
/// E5-S4 asks for at least four reads in flight, and a connection is the only
/// unit of concurrency available.
const POOL_SIZE: usize = 4;

/// The ceiling on a single READ or WRITE, before the server's own limit.
///
/// A megabyte is what E5-S4 caps at, but the codec cannot reassemble a reply
/// split across RPC record fragments — it errors and leaves unread bytes on the
/// socket, poisoning it. Servers send single fragments for replies of ordinary
/// size, so staying well under the cap is the cheap way to not find out.
const MAX_TRANSFER: u32 = 512 * 1024;

/// Everything needed to reach an export.
#[derive(Debug, Clone)]
pub struct NfsTarget<'a> {
    /// Host as the user typed it. Resolved here, because the codec wants an IP.
    pub host: &'a str,
    /// The NFS port. `mountd`'s is found through the portmapper unless given.
    pub port: u16,
    /// `mountd`'s port, when the portmapper cannot be trusted to name it —
    /// notably behind a port mapping, where it reports the inside port.
    pub mount_port: Option<u16>,
    /// The export path, as the server exports it.
    pub export: &'a str,
    /// The `AUTH_SYS` identity. E5-S2's "Identity" field.
    pub identity: Identity,
    /// What the user asked for; the server decides what they get.
    pub read_only: bool,
}

/// Who the client claims to be.
///
/// NFS has no password: `AUTH_SYS` is an assertion the server chooses to believe,
/// which is why the dialog shows this as an identity rather than as a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
    /// Supplementary groups. The protocol allows sixteen.
    pub gids: Vec<u32>,
}

impl Default for Identity {
    fn default() -> Self {
        // The current user, which is what a kernel mount would send. Falling
        // back to 0 would silently ask to be root, and an export with
        // `root_squash` would then map us to `nobody` for reasons no message
        // would explain.
        Self {
            uid: current_uid(),
            gid: current_gid(),
            gids: Vec::new(),
        }
    }
}

/// This process's user and group id.
///
/// `std` exposes no `getuid`, and calling it directly would need `unsafe`,
/// which this workspace denies for the sake of one integer. A file this
/// process creates is owned by its effective uid and gid, so making one and
/// asking is the same question answered safely. Done once and remembered.
#[cfg(unix)]
fn current_ids() -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    static IDS: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *IDS.get_or_init(|| {
        let probe = std::env::temp_dir().join(format!(".excalibur-id-{}", std::process::id()));
        let ids = std::fs::File::create(&probe)
            .and_then(|file| file.metadata())
            .map_or((65534, 65534), |meta| (meta.uid(), meta.gid()));
        let _ = std::fs::remove_file(&probe);
        ids
    })
}

#[cfg(not(unix))]
fn current_ids() -> (u32, u32) {
    (65534, 65534)
}

fn current_uid() -> u32 {
    current_ids().0
}

fn current_gid() -> u32 {
    current_ids().1
}

/// What it takes to open one more connection to the same export.
#[derive(Debug, Clone)]
struct Dial {
    /// Already resolved: the codec parses this as an IP literal.
    ip: String,
    port: u16,
    mount_port: Option<u16>,
    export: String,
    identity: Identity,
}

impl Dial {
    /// Open, MOUNT, and hand back a connection with its root handle.
    async fn connect(&self) -> Result<Conn> {
        let credential = opaque_auth::auth_unix(&auth_unix {
            stamp: 0,
            machinename: Opaque::owned(b"excalibur".to_vec()),
            uid: self.identity.uid,
            gid: self.identity.gid,
            gids: self.identity.gids.clone(),
        });
        let mut builder = Nfs3ConnectionBuilder::new(TokioConnector, &self.ip, &self.export)
            .credential(credential)
            // Never a privileged port. This process is not root, so the bind
            // fails outright; E5-S2 requires explaining the export option
            // instead of reaching for a port we cannot have.
            .connect_from_privileged_port(false)
            .nfs3_port(self.port);
        if let Some(port) = self.mount_port {
            // Given when the portmapper cannot be believed — behind a port
            // mapping it reports the port inside, which is not reachable.
            builder = builder.mount_port(port);
        }
        builder.mount().await.map_err(|error| map_connect(&error))
    }
}

/// Several connections to one export, because one cannot answer two requests.
struct Pool {
    idle: Mutex<Vec<Conn>>,
    permits: Arc<Semaphore>,
    dial: Dial,
}

impl Pool {
    /// Open the pool, and hand back the root handle alongside it.
    async fn open(dial: Dial) -> Result<(Arc<Self>, nfs3::nfs_fh3)> {
        // One connection first, so a bad host, a refused mount or a squashed
        // identity is reported once rather than four times.
        let first = dial.connect().await?;
        let root = first.root_nfs_fh3();
        Ok((
            Arc::new(Self {
                idle: Mutex::new(vec![first]),
                permits: Arc::new(Semaphore::new(POOL_SIZE)),
                dial,
            }),
            root,
        ))
    }

    /// Borrow a connection, opening one if the pool has none spare.
    ///
    /// The pool fills lazily: a mount that only ever lists a directory never
    /// opens the other three sockets.
    async fn acquire(self: &Arc<Self>) -> Result<Lease> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| EngineError::Disconnected("the mount is closing".to_owned()))?;
        let existing = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop();
        let conn = match existing {
            Some(conn) => conn,
            None => self.dial.connect().await?,
        };
        Ok(Lease {
            pool: Arc::clone(self),
            conn: Some(conn),
            _permit: permit,
        })
    }
}

/// A borrowed connection, returned to the pool when dropped.
struct Lease {
    pool: Arc<Pool>,
    conn: Option<Conn>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Lease {
    fn get(&mut self) -> &mut Conn {
        self.conn
            .as_mut()
            .expect("a lease always holds a connection")
    }

    /// Drop the connection instead of returning it.
    ///
    /// A transport error leaves unread bytes on the socket, so the next request
    /// would read the tail of the last reply. `acquire` opens a fresh one.
    fn discard(mut self) {
        self.conn = None;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(conn);
        }
    }
}

/// A mounted `NFSv3` export.
pub struct NfsBackend {
    pool: Arc<Pool>,
    root: nfs3::nfs_fh3,
    info: MountInfo,
    /// The largest READ this server will answer, after our own cap.
    read_chunk: u32,
    /// The largest WRITE it will accept.
    write_chunk: u32,
}

impl NfsBackend {
    /// Resolve, mount, and work out what the export allows.
    ///
    /// # Errors
    ///
    /// Fails when the host cannot be resolved or reached, the export is not
    /// exported to this client, or the server demands a privileged source port.
    pub async fn connect(target: &NfsTarget<'_>) -> Result<Self> {
        // The codec parses the host as an IP literal, so a name is resolved
        // here rather than producing "invalid IP address syntax" from inside.
        let ip = resolve(target.host, target.port).await?;
        let dial = Dial {
            ip,
            port: target.port,
            mount_port: target.mount_port,
            export: target.export.to_owned(),
            identity: target.identity.clone(),
        };
        let (pool, root) = Pool::open(dial).await?;

        let (read_chunk, write_chunk, capacity) = limits(&pool, &root).await;
        let info = probe_mount(&pool, &root, target, capacity).await;
        Ok(Self {
            pool,
            root,
            info,
            read_chunk,
            write_chunk,
        })
    }

    /// Walk from the export root to a path, one LOOKUP per component.
    ///
    /// NFS is handle-based and has no notion of a path, so this is the
    /// translation. It is a round trip per component; a cache belongs here
    /// eventually, and correctness comes first.
    async fn resolve_handle(&self, path: &str) -> Result<nfs3::nfs_fh3> {
        let mut handle = self.root.clone();
        for component in path.split('/').filter(|part| !part.is_empty()) {
            if component == "." || component == ".." {
                // Refused rather than followed: a path that climbs out of the
                // export is a bug or an attempt, and neither should be served.
                return Err(EngineError::NotFound(format!(
                    "{path} contains a relative component"
                )));
            }
            let mut lease = self.pool.acquire().await?;
            let reply = lease
                .get()
                .lookup(&nfs3::LOOKUP3args {
                    what: nfs3::diropargs3 {
                        dir: handle,
                        name: filename3(Opaque::borrowed(component.as_bytes())),
                    },
                })
                .await;
            handle = match reply {
                Ok(result) => unwrap_nfs(result, path)?.object,
                Err(error) => {
                    lease.discard();
                    return Err(map_rpc(&error));
                }
            };
        }
        Ok(handle)
    }

    /// The parent handle and the final name, for the mutations.
    async fn split_parent(&self, path: &str) -> Result<(nfs3::nfs_fh3, String)> {
        let trimmed = path.trim_matches('/');
        let (parent, name) = match trimmed.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", trimmed),
        };
        if name.is_empty() {
            return Err(EngineError::NotFound(format!("{path} names nothing")));
        }
        Ok((self.resolve_handle(parent).await?, name.to_owned()))
    }
}

/// Turn a host — name or literal — into the IP the codec insists on.
async fn resolve(host: &str, port: u16) -> Result<String> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Ok(host.to_owned());
    }
    let mut addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| EngineError::Disconnected(format!("{host}: {error}")))?;
    // IPv4 first: the crate's own connector binds an IPv4 socket on one of its
    // paths, so an IPv6 answer can fail later for reasons that read as
    // unrelated.
    let mut fallback = None;
    for address in addresses.by_ref() {
        if address.is_ipv4() {
            return Ok(address.ip().to_string());
        }
        fallback.get_or_insert(address);
    }
    fallback
        .map(|address| address.ip().to_string())
        .ok_or_else(|| EngineError::Disconnected(format!("{host} resolves to nothing")))
}

/// The server's transfer limits and its capacity, in one pass.
async fn limits(pool: &Arc<Pool>, root: &nfs3::nfs_fh3) -> (u32, u32, Option<Capacity>) {
    let Ok(mut lease) = pool.acquire().await else {
        return (MAX_TRANSFER, MAX_TRANSFER, None);
    };
    let info = lease
        .get()
        .fsinfo(&nfs3::FSINFO3args {
            fsroot: root.clone(),
        })
        .await
        .ok()
        .and_then(|result| match result {
            Nfs3Result::Ok(ok) => Some(ok),
            Nfs3Result::Err(_) => None,
        });
    let stat = lease
        .get()
        .fsstat(&nfs3::FSSTAT3args {
            fsroot: root.clone(),
        })
        .await
        .ok()
        .and_then(|result| match result {
            Nfs3Result::Ok(ok) => Some(ok),
            Nfs3Result::Err(_) => None,
        });

    // `rtmax`/`wtmax` are the server's hard ceilings; ours is lower on purpose.
    // A server reporting 0 means "no limit stated", which is not a reason to
    // send nothing.
    let read = info
        .as_ref()
        .map_or(MAX_TRANSFER, |i| nonzero(i.rtmax).min(MAX_TRANSFER));
    let write = info
        .as_ref()
        .map_or(MAX_TRANSFER, |i| nonzero(i.wtmax).min(MAX_TRANSFER));
    let capacity = stat.map(|s| Capacity {
        // `abytes`, not `fbytes`: the root reserve is not space this user can
        // write into, and showing it would overstate what they can do. Same
        // choice as the xsync and SFTP backends.
        free: s.abytes,
        total: s.tbytes,
    });
    (read, write, capacity)
}

fn nonzero(value: u32) -> u32 {
    if value == 0 {
        MAX_TRANSFER
    } else {
        value
    }
}

/// Work out what this mount allows, and say which term failed.
///
/// `NFSv3` offers no single "is this writable" answer, and the three ways it can
/// be no are worth telling apart: the user asked for read-only, the export is
/// exported read-only, or this identity is squashed to `nobody`. E5-S3 wants
/// the last one named, because "permission denied" sends a user hunting
/// through their own account rather than the export line.
async fn probe_mount(
    pool: &Arc<Pool>,
    root: &nfs3::nfs_fh3,
    target: &NfsTarget<'_>,
    capacity: Option<Capacity>,
) -> MountInfo {
    let (writable, reason) = if target.read_only {
        (false, "Read-only — you asked for it".to_owned())
    } else {
        match probe_write(pool, root).await {
            Ok(()) => (true, String::new()),
            Err(status) => (false, explain(status, target)),
        }
    };

    MountInfo {
        status: MountStatus::Ready,
        writable,
        reason,
        protocol_label: "NFSv3".to_owned(),
        // AUTH_SYS is an assertion the server chooses to believe, and calling
        // it anything stronger would overstate it. Kerberos flavours are
        // E5-S5, and this label becomes `krb5p` and friends when they land.
        security_label: "sys".to_owned(),
        options: String::new(),
        capacity,
        caps: Caps {
            random_access: true,
            resume_write: true,
            // NFS RENAME is POSIX rename: it replaces atomically on the
            // server. Better than the SFTP backend can manage without an
            // extension, and the transfer worker may rely on it.
            atomic_rename: true,
            symlinks: true,
            watch: false,
        },
    }
}

/// Distinguishes one write probe from another within this process.
static PROBE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Create a file and remove it, to learn whether this identity may write.
///
/// The ACCESS bits are consulted first because they are one round trip and
/// usually decisive, but they are advisory — a server may grant them and still
/// refuse the write. Only the create answers the question the Upload button
/// will ask.
async fn probe_write(pool: &Arc<Pool>, root: &nfs3::nfs_fh3) -> std::result::Result<(), nfsstat3> {
    let mut lease = pool.acquire().await.map_err(|_| nfsstat3::NFS3ERR_IO)?;
    let granted = lease
        .get()
        .access(&nfs3::ACCESS3args {
            object: root.clone(),
            access: ACCESS3_MODIFY | ACCESS3_EXTEND | ACCESS3_DELETE,
        })
        .await;
    if let Ok(Nfs3Result::Ok(ok)) = granted {
        if ok.access & (ACCESS3_MODIFY | ACCESS3_EXTEND) == 0 {
            return Err(nfsstat3::NFS3ERR_ACCES);
        }
    }

    // Unique per probe, not per process: two mounts of the same export in one
    // process would otherwise pick the same name, and the `GUARDED` create
    // below would fail with `EXIST` for whichever lost — reporting a writable
    // export as read-only.
    let name = format!(
        ".excalibur-write-probe-{}-{}",
        std::process::id(),
        PROBE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let created = lease
        .get()
        .create(&nfs3::CREATE3args {
            where_: nfs3::diropargs3 {
                dir: root.clone(),
                name: filename3(Opaque::borrowed(name.as_bytes())),
            },
            // GUARDED so an existing file of the same name is an error rather
            // than something this probe would truncate.
            how: nfs3::createhow3::GUARDED(sattr3 {
                mode: Nfs3Option::Some(0o600),
                ..sattr3::default()
            }),
        })
        .await;
    match created {
        Ok(Nfs3Result::Ok(_)) => {
            // Best effort: a probe left behind is untidy, not incorrect.
            let _ = lease
                .get()
                .remove(&nfs3::REMOVE3args {
                    object: nfs3::diropargs3 {
                        dir: root.clone(),
                        name: filename3(Opaque::borrowed(name.as_bytes())),
                    },
                })
                .await;
            Ok(())
        }
        Ok(Nfs3Result::Err((status, _))) => Err(status),
        Err(_) => {
            lease.discard();
            Err(nfsstat3::NFS3ERR_IO)
        }
    }
}

/// Turn a refusal into the sentence the status bar shows.
fn explain(status: nfsstat3, target: &NfsTarget<'_>) -> String {
    match status {
        nfsstat3::NFS3ERR_ROFS => {
            format!("Read-only — `{}` is exported read-only", target.export)
        }
        nfsstat3::NFS3ERR_ACCES | nfsstat3::NFS3ERR_PERM => {
            // The identity is the usual cause, and root is the usual identity
            // to be refused, because `root_squash` is the default nearly
            // everywhere. Naming the option is what makes this actionable.
            if target.identity.uid == 0 {
                format!(
                    "Read-only — `{}` maps root to `nobody` (`root_squash`); \
                     connect as a real user or export with `no_root_squash`",
                    target.export
                )
            } else {
                format!(
                    "Read-only — uid {} may not write to `{}`",
                    target.identity.uid, target.export
                )
            }
        }
        other => format!("Read-only — the server refused a write ({other})"),
    }
}

#[async_trait]
impl ShareBackend for NfsBackend {
    fn mount_info(&self) -> MountInfo {
        self.info.clone()
    }

    fn caps(&self) -> Caps {
        self.info.caps
    }

    async fn capacity(&self) -> Result<Capacity> {
        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .fsstat(&nfs3::FSSTAT3args {
                fsroot: self.root.clone(),
            })
            .await;
        match reply {
            Ok(result) => {
                let ok = unwrap_nfs(result, "fsstat")?;
                Ok(Capacity {
                    free: ok.abytes,
                    total: ok.tbytes,
                })
            }
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        let directory = self.resolve_handle(path).await?;
        let relative = path.trim_matches('/').to_owned();
        let mut rows = Vec::new();

        // The codec does not page for us: a server answers with as many
        // entries as fit and expects to be asked again from the last cookie.
        let mut cookie: cookie3 = 0;
        let mut verifier = cookieverf3::default();
        loop {
            let mut lease = self.pool.acquire().await?;
            let reply = lease
                .get()
                .readdirplus(&nfs3::READDIRPLUS3args {
                    dir: directory.clone(),
                    cookie,
                    cookieverf: verifier,
                    dircount: 32 * 1024,
                    maxcount: 128 * 1024,
                })
                .await;
            let ok = match reply {
                Ok(result) => unwrap_nfs(result, path)?,
                Err(error) => {
                    lease.discard();
                    return Err(map_rpc(&error));
                }
            };
            drop(lease);

            verifier = ok.cookieverf;
            let eof = ok.reply.eof;
            let entries = ok.reply.entries.into_inner();
            // A server may answer with nothing and `eof: false` when the size
            // hints were too small. Advancing off the last entry would panic,
            // and not advancing would spin here forever.
            let Some(last) = entries.last() else {
                if eof {
                    break;
                }
                return Err(EngineError::Io(format!(
                    "{path}: the server returned no entries and no end marker"
                )));
            };
            cookie = last.cookie;

            for entry in entries {
                // Names are bytes on the wire, not UTF-8. One that is not
                // valid is shown rather than hidden — a file the user can see
                // in another client but not here is worse than a lossy name.
                let name = String::from_utf8_lossy(entry.name.as_ref()).into_owned();
                if name == "." || name == ".." {
                    continue;
                }
                let attrs = match &entry.name_attributes {
                    Nfs3Option::Some(fattr) => attrs_of(fattr),
                    // READDIRPLUS attributes are optional. Rather than a
                    // GETATTR per entry, the row is shown with what is known;
                    // the kind still comes from the name.
                    Nfs3Option::None => Attrs {
                        kind: EntryKind::Other,
                        size: 0,
                        modified_ms: None,
                        mode: None,
                        link_target: None,
                        change_cookie: None,
                    },
                };
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
            if eof {
                break;
            }
        }

        Ok(Listing {
            entries: rows,
            mount: self.info.clone(),
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        let handle = self.resolve_handle(path).await?;
        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .getattr(&nfs3::GETATTR3args { object: handle })
            .await;
        match reply {
            Ok(result) => Ok(attrs_of(&unwrap_nfs(result, path)?.obj_attributes)),
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        let handle = self.resolve_handle(path).await?;
        let mut chunks = Vec::new();
        let mut start = 0_u32;
        while start < length {
            chunks.push((start, self.read_chunk.min(length - start)));
            start = start.saturating_add(self.read_chunk);
        }

        // One connection answers one request at a time, so the concurrency
        // here is the pool: four sockets, four reads outstanding.
        let mut done: Vec<Option<Vec<u8>>> = vec![None; chunks.len()];
        let mut running: tokio::task::JoinSet<(usize, Result<Vec<u8>>)> =
            tokio::task::JoinSet::new();
        let mut next = 0_usize;
        loop {
            while running.len() < POOL_SIZE && next < chunks.len() {
                let (start, want) = chunks[next];
                let index = next;
                let pool = Arc::clone(&self.pool);
                let handle = handle.clone();
                let at = offset + u64::from(start);
                running.spawn(async move { (index, read_one(&pool, handle, at, want).await) });
                next += 1;
            }
            let Some(joined) = running.join_next().await else {
                break;
            };
            let (index, bytes) =
                joined.map_err(|error| EngineError::Io(format!("read task: {error}")))?;
            done[index] = Some(bytes?);
        }

        // Up to the first short chunk only: past that is end of file, and a
        // later chunk holding bytes would place them at the wrong offset.
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

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        if !self.info.writable {
            return Err(EngineError::ReadOnly(self.info.reason.clone()));
        }
        let (handle, offset) = match mode {
            WriteMode::Create => {
                let (parent, name) = self.split_parent(path).await?;
                (self.create_file(&parent, &name).await?, 0)
            }
            WriteMode::Resume { offset } => (self.resolve_handle(path).await?, offset),
        };

        // UNSTABLE writes followed by one COMMIT, which is what E5-S4 asks for
        // and what makes NFS writes worth doing at all — FILE_SYNC would make
        // the server wait on its disk for every chunk.
        //
        // The price is the write verifier: if it changes, the server rebooted
        // and everything not committed is gone. Detected and reported rather
        // than silently losing the tail.
        let mut written = 0_usize;
        let mut verifier: Option<nfs3::writeverf3> = None;
        while written < data.len() {
            let end = data.len().min(written + self.write_chunk as usize);
            let slice = &data[written..end];
            let mut lease = self.pool.acquire().await?;
            let reply = lease
                .get()
                .write(&nfs3::WRITE3args {
                    file: handle.clone(),
                    offset: offset + written as u64,
                    count: u32::try_from(slice.len()).unwrap_or(u32::MAX),
                    stable: nfs3::stable_how::UNSTABLE,
                    data: Opaque::borrowed(slice),
                })
                .await;
            let ok = match reply {
                Ok(result) => unwrap_nfs(result, path)?,
                Err(error) => {
                    lease.discard();
                    return Err(map_rpc(&error));
                }
            };
            drop(lease);
            check_verifier(&mut verifier, ok.verf, path)?;
            // A server may accept less than was offered; the rest is re-sent
            // rather than assumed written.
            written += (ok.count as usize).min(slice.len()).max(1);
        }

        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .commit(&nfs3::COMMIT3args {
                file: handle,
                offset,
                count: u32::try_from(written).unwrap_or(u32::MAX),
            })
            .await;
        match reply {
            Ok(result) => {
                let ok = unwrap_nfs(result, path)?;
                check_verifier(&mut verifier, ok.verf, path)?;
            }
            Err(error) => {
                lease.discard();
                return Err(map_rpc(&error));
            }
        }
        Ok(written as u64)
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let (parent, name) = self.split_parent(path).await?;
        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .mkdir(&nfs3::MKDIR3args {
                where_: nfs3::diropargs3 {
                    dir: parent,
                    name: filename3(Opaque::borrowed(name.as_bytes())),
                },
                attributes: sattr3 {
                    mode: Nfs3Option::Some(0o755),
                    ..sattr3::default()
                },
            })
            .await;
        match reply {
            Ok(result) => unwrap_nfs(result, path).map(|_| ()),
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        let (from_parent, from_name) = self.split_parent(from).await?;
        let (to_parent, to_name) = self.split_parent(to).await?;

        if !replace {
            // NFS RENAME is POSIX rename, which replaces silently. There is no
            // no-clobber flag in v3, so the check is a separate LOOKUP and is
            // therefore racy — unlike xsync, which decides it server-side. The
            // window is named rather than hidden.
            if self.resolve_handle(to).await.is_ok() {
                return Err(EngineError::AlreadyExists(format!("{to} already exists")));
            }
        }

        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .rename(&nfs3::RENAME3args {
                from: nfs3::diropargs3 {
                    dir: from_parent,
                    name: filename3(Opaque::borrowed(from_name.as_bytes())),
                },
                to: nfs3::diropargs3 {
                    dir: to_parent,
                    name: filename3(Opaque::borrowed(to_name.as_bytes())),
                },
            })
            .await;
        match reply {
            Ok(result) => unwrap_nfs(result, from).map(|_| ()),
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let attrs = self.stat(path).await?;
        if attrs.kind != EntryKind::Dir {
            return self.unlink(path, false).await;
        }
        if !recursive {
            return self.unlink(path, true).await;
        }
        // Iteratively, depth first: an `async fn` that calls itself needs
        // boxing, and a deep tree would be bounded by the stack rather than by
        // the export.
        let mut directories = vec![path.trim_matches('/').to_owned()];
        let mut index = 0;
        while index < directories.len() {
            let directory = directories[index].clone();
            index += 1;
            for entry in self.list_dir(&directory).await?.entries {
                if entry.attrs.kind == EntryKind::Dir {
                    directories.push(entry.path);
                } else {
                    self.unlink(&entry.path, false).await?;
                }
            }
        }
        for directory in directories.into_iter().rev() {
            self.unlink(&directory, true).await?;
        }
        Ok(())
    }

    async fn ping(&self) -> Result<Health> {
        let Ok(mut lease) = self.pool.acquire().await else {
            return Ok(Health::Failed);
        };
        let reply = lease
            .get()
            .getattr(&nfs3::GETATTR3args {
                object: self.root.clone(),
            })
            .await;
        if reply.is_ok() {
            Ok(Health::Healthy)
        } else {
            lease.discard();
            Ok(Health::Failed)
        }
    }
}

impl NfsBackend {
    /// Create (or truncate) a file and hand back its handle.
    async fn create_file(&self, parent: &nfs3::nfs_fh3, name: &str) -> Result<nfs3::nfs_fh3> {
        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .create(&nfs3::CREATE3args {
                where_: nfs3::diropargs3 {
                    dir: parent.clone(),
                    name: filename3(Opaque::borrowed(name.as_bytes())),
                },
                // UNCHECKED with an explicit size, so an existing file is
                // truncated rather than appended to — `WriteMode::Create` means
                // the caller is writing the whole thing.
                how: nfs3::createhow3::UNCHECKED(sattr3 {
                    mode: Nfs3Option::Some(0o644),
                    size: Nfs3Option::Some(0),
                    ..sattr3::default()
                }),
            })
            .await;
        let ok = match reply {
            Ok(result) => unwrap_nfs(result, name)?,
            Err(error) => {
                lease.discard();
                return Err(map_rpc(&error));
            }
        };
        drop(lease);
        match ok.obj {
            Nfs3Option::Some(handle) => Ok(handle),
            // The server may create without returning a handle, which is legal.
            Nfs3Option::None => self.resolve_handle_in(parent, name).await,
        }
    }

    /// LOOKUP one name inside a known directory.
    async fn resolve_handle_in(&self, parent: &nfs3::nfs_fh3, name: &str) -> Result<nfs3::nfs_fh3> {
        let mut lease = self.pool.acquire().await?;
        let reply = lease
            .get()
            .lookup(&nfs3::LOOKUP3args {
                what: nfs3::diropargs3 {
                    dir: parent.clone(),
                    name: filename3(Opaque::borrowed(name.as_bytes())),
                },
            })
            .await;
        match reply {
            Ok(result) => Ok(unwrap_nfs(result, name)?.object),
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }

    /// REMOVE a file or RMDIR a directory.
    async fn unlink(&self, path: &str, directory: bool) -> Result<()> {
        let (parent, name) = self.split_parent(path).await?;
        let args = nfs3::diropargs3 {
            dir: parent,
            name: filename3(Opaque::borrowed(name.as_bytes())),
        };
        let mut lease = self.pool.acquire().await?;
        let reply = if directory {
            lease
                .get()
                .rmdir(&nfs3::RMDIR3args { object: args })
                .await
                .map(|result| unwrap_nfs(result, path).map(|_| ()))
        } else {
            lease
                .get()
                .remove(&nfs3::REMOVE3args { object: args })
                .await
                .map(|result| unwrap_nfs(result, path).map(|_| ()))
        };
        match reply {
            Ok(inner) => inner,
            Err(error) => {
                lease.discard();
                Err(map_rpc(&error))
            }
        }
    }
}

/// One READ, on one borrowed connection.
async fn read_one(
    pool: &Arc<Pool>,
    handle: nfs3::nfs_fh3,
    offset: u64,
    want: u32,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(want as usize);
    while bytes.len() < want as usize {
        let filled = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let mut lease = pool.acquire().await?;
        let reply = lease
            .get()
            .read(&nfs3::READ3args {
                file: handle.clone(),
                offset: offset + u64::from(filled),
                count: want - filled,
            })
            .await;
        let ok = match reply {
            Ok(result) => unwrap_nfs(result, "read")?,
            Err(error) => {
                lease.discard();
                return Err(map_rpc(&error));
            }
        };
        drop(lease);
        let eof = ok.eof;
        let data = ok.data.into_owned();
        if data.is_empty() {
            break;
        }
        bytes.extend_from_slice(&data);
        if eof {
            break;
        }
    }
    Ok(bytes)
}

/// A changed write verifier means the server restarted mid-write.
fn check_verifier(
    seen: &mut Option<nfs3::writeverf3>,
    reported: nfs3::writeverf3,
    path: &str,
) -> Result<()> {
    match seen {
        None => {
            *seen = Some(reported);
            Ok(())
        }
        Some(first) if *first == reported => Ok(()),
        Some(_) => Err(EngineError::Io(format!(
            "{path}: the server restarted during the write, so what was sent \
             before it is gone; the transfer has to start again"
        ))),
    }
}

/// NFS attributes as the engine describes a file.
fn attrs_of(fattr: &nfs3::fattr3) -> Attrs {
    let kind = match fattr.type_ {
        ftype3::NF3REG => EntryKind::File,
        ftype3::NF3DIR => EntryKind::Dir,
        ftype3::NF3LNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    Attrs {
        kind,
        size: fattr.size,
        modified_ms: Some(i64::from(fattr.mtime.seconds) * 1000),
        mode: Some(fattr.mode),
        link_target: None,
        // NFSv3 has no change cookie, so edit-in-place falls back to comparing
        // size and mtime. Weaker than xsync's, and honest about it.
        change_cookie: None,
    }
}

/// The success arm of a per-call result, or the engine's error.
///
/// NFS reports failures inside the reply rather than as transport errors, so
/// this is where a status becomes an error. The crate's own `unwrap` panics on
/// the error arm, which is why it is never used here.
fn unwrap_nfs<T, E>(result: Nfs3Result<T, E>, what: &str) -> Result<T> {
    match result {
        Nfs3Result::Ok(value) => Ok(value),
        Nfs3Result::Err((status, _)) => Err(map_status(status, what)),
    }
}

/// Translate an NFS status into the engine's taxonomy.
fn map_status(status: nfsstat3, what: &str) -> EngineError {
    match status {
        nfsstat3::NFS3ERR_NOENT => EngineError::NotFound(what.to_owned()),
        nfsstat3::NFS3ERR_EXIST => EngineError::AlreadyExists(what.to_owned()),
        nfsstat3::NFS3ERR_ACCES | nfsstat3::NFS3ERR_PERM => {
            EngineError::PermissionDenied(format!("{what}: {status}"))
        }
        nfsstat3::NFS3ERR_ROFS => EngineError::ReadOnly(format!("{what}: {status}")),
        nfsstat3::NFS3ERR_NOTSUPP => EngineError::Unsupported(format!("{what}: {status}")),
        // Everything else — NOTEMPTY, NOSPC, DQUOT, STALE, JUKEBOX and the
        // rest — carries its own name, which is more use than a category.
        other => EngineError::Io(format!("{what}: {other}")),
    }
}

/// Translate a transport error.
fn map_rpc(error: &nfs3_client::RpcError) -> EngineError {
    EngineError::Disconnected(error.to_string())
}

/// Translate a failure to mount at all.
fn map_connect(error: &nfs3_client::ConnectError) -> EngineError {
    use nfs3_client::{ConnectError, MountError};
    match error {
        // The commonest cause by far, and the one worth a sentence: the export
        // does not carry `insecure`, so the server wants a source port below
        // 1024 that this process cannot bind. E5-S2 forbids trying.
        ConnectError::Mount(MountError::Denied(status)) => EngineError::PermissionDenied(format!(
            "the server refused the mount ({status}). If the export is meant to \
             be reachable from a userspace client, it needs the `insecure` \
             option — this program will not bind a privileged port"
        )),
        other => EngineError::Disconnected(other.to_string()),
    }
}
