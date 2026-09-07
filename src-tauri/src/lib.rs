//! The Tauri shell: commands, DTOs and the event bridge. Nothing else.
//!
//! Two boundaries this file exists to keep:
//!
//! - **File bytes never cross IPC.** Commands carry paths and metadata. A
//!   preview or a transfer streams through the engine and, later, an asset
//!   protocol handler — never through a command's return value.
//! - **The engine knows nothing about Tauri.** Everything here is a thin
//!   translation, so the engine stays testable without a window.

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::nfs::{self, Identity, NfsBackend, NfsTarget};
use excalibur_engine::backend::sftp::{Credential, SftpBackend, SftpTarget};
#[cfg(feature = "smb")]
use excalibur_engine::backend::smb::{SmbBackend, SmbTarget};
use excalibur_engine::backend::xsync::{SshTarget, XsyncBackend};
use excalibur_engine::backend::{Access, ConnectionSpec, EntryKind, Protocol, ShareBackend};
use excalibur_engine::drag::{self, Selection};
use excalibur_engine::transfer::conflict::{self, Conflict};
use excalibur_engine::transfer::plan;
use excalibur_engine::transfer::queue::{QueueEvent, TransferQueue};
use excalibur_engine::transfer::worker::{BackendSource, Worker};
use excalibur_engine::transfer::xsync_engine::{SshEndpoint, XsyncRemote, XsyncRoute};
use excalibur_engine::{
    hostkeys, Connector, Direction, EditSession, EditStore, EngineError, KeyringStore, KnownHosts,
    Listing, MountInfo, Result, SaveOutcome, Secret, SecretStore, SessionManager, Transfer,
    TransferStatus, Trust,
};
use tauri::http;
use tauri::{Emitter, Manager};

/// Builds the backend a `ConnectionSpec` names.
///
/// This is where the protocol enum turns into a client. `connect()` is
/// deliberately not on `ShareBackend`: each protocol needs different things to
/// get started — a keychain entry, a host key, a port — and the shape of that
/// should come from the second real backend rather than be guessed before the
/// first.
struct ProtocolConnector {
    known_hosts: KnownHosts,
    /// Where a stored password comes from. Read here, never in the window.
    secrets: Arc<dyn SecretStore>,
}

#[async_trait]
impl Connector for ProtocolConnector {
    // One arm per protocol, each of them the credential handling and the
    // target struct that protocol needs, read top to bottom. It went over the
    // lint's hundred lines when SMB landed and E21's local arm is not what
    // would pay for splitting it: the arms share nothing, so factoring them
    // out buys five one-line functions and a longer path to the code.
    #[allow(clippy::too_many_lines)]
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        match spec.protocol {
            Protocol::Xsync => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
                // Decided before `ssh` runs, so an unknown or changed key
                // reaches the window as something it can render rather than as
                // OpenSSH's own prompt, which has no terminal to appear on.
                match hostkeys::verify(&hostkeys::Keyscan, &self.known_hosts, &spec.host, port)
                    .await?
                {
                    Trust::Trusted => {}
                    Trust::Unknown { offered } => {
                        return Err(EngineError::HostKeyUnknown {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                        })
                    }
                    Trust::Changed { offered, stored } => {
                        return Err(EngineError::HostKeyChanged {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                            expected: stored.fingerprint(),
                        })
                    }
                    Trust::Revoked { offered } => {
                        return Err(EngineError::HostKeyChanged {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                            expected: "a key you revoked".to_owned(),
                        })
                    }
                }
                let target = SshTarget {
                    host: &spec.host,
                    port,
                    username: spec.username.as_deref(),
                    known_hosts: self.known_hosts.path(),
                    rsh: None,
                };
                let backend = XsyncBackend::connect_ssh(&target, &spec.share).await?;
                Ok(Arc::new(backend))
            }
            // `share` is the folder and `host` is empty: a folder on this
            // machine has no host to name, and inventing one would put a
            // reachability question on a bookmark that cannot fail (E21-S2).
            Protocol::Local => Ok(Arc::new(
                LocalBackend::open(Path::new(&spec.share), spec.mount_preference == Access::Ro)
                    .await?,
            )),
            Protocol::Sftp => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
                let username = spec.username.clone().unwrap_or_else(whoami);
                // The credential is read from the keychain here rather than
                // being passed in from the window: that is what keeps E3-S3's
                // rule true — the window collects a password once and never
                // sees it again.
                let credential = match self.secrets.get(&spec.id) {
                    Ok(Some(secret)) => Credential::Password(secret.expose().to_owned()),
                    // No stored password means the agent, which is what most
                    // SSH users actually have, and needs nothing typed.
                    _ => Credential::Agent,
                };
                let target = SftpTarget {
                    host: &spec.host,
                    port,
                    username: &username,
                    root: &spec.share,
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend =
                    SftpBackend::connect(&target, &credential, Arc::new(self.known_hosts.clone()))
                        .await?;
                Ok(Arc::new(backend))
            }
            Protocol::Nfs => {
                let target = NfsTarget {
                    host: &spec.host,
                    port: spec.port.unwrap_or(nfs::DEFAULT_NFS_PORT),
                    // The portmapper names `mountd`'s port, which is right
                    // except behind a port mapping, where it reports the port
                    // inside. Nothing in the dialog offers that yet.
                    mount_port: None,
                    export: &spec.share,
                    // NFS has no password. The dialog calls this field
                    // "Identity" and it carries `uid:gid`, because AUTH_SYS is
                    // an assertion rather than a login — there is no secret to
                    // read from the keychain here, and nothing to read it with.
                    identity: parse_identity(spec.username.as_deref()),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = NfsBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
            // Named rather than caught by a wildcard, so that adding a
            // protocol to the enum breaks this match instead of silently
            // routing it here. Two hangs in the sibling xsync repo came from
            // exactly that shape of arm.
            #[cfg(feature = "smb")]
            Protocol::Smb => {
                let username = spec.username.clone().unwrap_or_else(whoami);
                let password = match self.secrets.get(&spec.id) {
                    Ok(Some(secret)) => secret.expose().to_owned(),
                    _ => String::new(),
                };
                let target = SmbTarget {
                    host: &spec.host,
                    port: spec.port,
                    share: &spec.share,
                    username: &username,
                    password: &password,
                    // The dialog's Domain field, which only SMB has.
                    workgroup: spec.domain.as_deref().unwrap_or(""),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = SmbBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
            // Built only with the `smb` feature, because linking libsmbclient
            // makes the binary GPL-3.0-or-later. Without it, a spec naming SMB
            // is answered honestly rather than silently mocked.
            #[cfg(not(feature = "smb"))]
            Protocol::Smb => Err(excalibur_engine::EngineError::Unsupported(
                "this build was compiled without SMB support".to_owned(),
            )),
        }
    }
}

/// `uid`, `uid:gid`, or nothing at all.
///
/// Anything unparseable falls back to this user rather than to root: claiming
/// uid 0 is how `root_squash` maps you to `nobody`, and the resulting refusal
/// would have nothing to do with what the user typed.
fn parse_identity(field: Option<&str>) -> Identity {
    let Some(text) = field.map(str::trim).filter(|text| !text.is_empty()) else {
        return Identity::default();
    };
    let (uid, gid) = match text.split_once(':') {
        Some((uid, gid)) => (uid.trim().parse().ok(), gid.trim().parse().ok()),
        None => (text.parse().ok(), None),
    };
    let default = Identity::default();
    Identity {
        uid: uid.unwrap_or(default.uid),
        gid: gid.unwrap_or(default.gid),
        gids: Vec::new(),
    }
}

/// The worker's window onto the session manager.
///
/// A thin adapter rather than the worker taking a `SessionManager` directly:
/// the worker should not know how a mount is chosen, only that there is one,
/// which is what lets a transfer survive a reconnect that swapped the backend
/// underneath it.
///
/// It asks for the *app's* mount rather than a window's (E23-S1). The queue is
/// app-wide and a transfer outlives the window that queued it — closing that
/// window must not strand the bytes — so there is no window here to ask. The
/// honest fix is a transfer that names its own endpoint, which is E22-S0's
/// work; until then this is what "the active mount" always meant, and with one
/// window it is exactly what it meant before.
struct AppMount {
    sessions: Arc<SessionManager>,
    /// The app's accepted SSH host keys.
    ///
    /// Held here, not just on the connector, because a transfer through
    /// xsync's engine spawns its *own* `ssh` (E22-S1) and it must enforce the
    /// same trust decision the browse session did. A second `ssh` that fell
    /// back to OpenSSH's own store would be a second, different policy.
    known_hosts: KnownHosts,
}

#[async_trait]
impl BackendSource for AppMount {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        self.sessions.recent_backend().await
    }

    /// The share a row names, rather than the one the app last touched.
    ///
    /// With two windows open, "the active mount" is not a thing the worker can
    /// ask for (E23-S1): a drag from one window into another (E24-S2) enqueues a
    /// transfer on a share that is by definition *not* the one most recently
    /// activated, and resolving it late would read from whichever window the
    /// user happened to click in last. Rows queued before this field existed
    /// still fall back, so a restored queue keeps working.
    async fn backend_for(&self, mount: Option<&str>) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = mount else {
            return self.sessions.recent_backend().await;
        };
        self.sessions
            .backend(id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))
    }

    /// The recipe for a second `ssh` to an xsync export (E22-S1).
    ///
    /// `None` for every other protocol, and that `None` is the routing rule:
    /// the worker asks this first and falls back to its own chunk loop when
    /// there is no answer. So NFS, SMB, SFTP and a local folder are unchanged
    /// by this story without a single mention of them here.
    ///
    /// A mount that is not `Ready` is deliberately still answered. The
    /// transfer opens its own process and does not use the browse session at
    /// all, so a mount that is reconnecting can still move bytes — and the
    /// failure, if the host really is gone, is `ssh`'s own and reads better
    /// than "not connected".
    async fn xsync_route(&self, mount: Option<&str>) -> Option<XsyncRoute> {
        let id = match mount {
            Some(id) => id.to_owned(),
            None => self.sessions.recent().await?,
        };
        let spec = self.sessions.spec(&id).await?;
        if spec.protocol != Protocol::Xsync {
            return None;
        }
        // The mount's own answer, not what the user asked for at connect time:
        // `writable` is the one derived value every write is gated on, and on
        // this route it is the *only* gate — `xs --server --read-only` does not
        // stop a v1/v2 push.
        let mount_info = self.sessions.mount_info(&id).await;
        Some(XsyncRoute {
            remote: XsyncRemote::Ssh(SshEndpoint {
                host: spec.host.clone(),
                port: spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT),
                username: spec.username.clone(),
                // The app's own trust store, so the transfer's `ssh` enforces
                // exactly the decision the user made in the trust prompt — the
                // same file, the same `StrictHostKeyChecking=yes`, the same
                // `BatchMode`. `hostkeys::verify` already ran when the mount
                // was made; a key that changed since then is refused by `ssh`
                // here rather than silently accepted.
                known_hosts: self.known_hosts.path().to_path_buf(),
                rsh: None,
            }),
            export_root: spec.share.clone(),
            writable: mount_info.as_ref().is_some_and(|info| info.writable),
            reason: mount_info.map_or_else(
                || "the mount has not reported its access yet".to_owned(),
                |info| info.reason,
            ),
        })
    }
}

/// What the shell holds: the mounts, and everything that acts on them.
struct AppState {
    sessions: Arc<SessionManager>,
    /// The OS credential store. Written to from the window, never read back to
    /// it: see the commands below.
    secrets: Arc<dyn SecretStore>,
    /// Accepted SSH host keys. Shared with the connector, which is what
    /// enforces them.
    known_hosts: KnownHosts,
    /// Files checked out for editing.
    edits: Arc<EditStore>,
    /// Everything moving, or waiting to.
    transfers: Arc<TransferQueue>,
    /// Moves it. Held so a new enqueue can wake it.
    worker: Arc<Worker>,
}

impl AppState {
    /// The backend a command should use: the **calling window's** mount, or
    /// nothing.
    ///
    /// The window is not a detail of the call, it is the subject of it
    /// (E23-S1): with two browser windows open there is no such thing as "the
    /// active mount", and a command that guessed would list one window's share
    /// into the other's list. Tauri injects the caller's `Window`, so no
    /// command has to be trusted to pass the right label.
    ///
    /// There is deliberately no fallback (E1-S6). A mock share stood here so
    /// the window had something to draw on first run, and what it drew was a
    /// share that does not exist — with verbs that acted on it, so Download
    /// queued a row for a file nobody has. "Nothing is mounted" is a fact the
    /// window can render as an empty state; a fake listing is not something it
    /// can tell apart from a real one.
    async fn backend(&self, window: &tauri::Window) -> Result<Arc<dyn ShareBackend>> {
        self.sessions.active_backend(window.label()).await
    }

    /// A named mount and its backend, defaulting to the calling window's.
    ///
    /// The id comes back with the backend because every queue row now records
    /// which share it moves bytes through, and resolving the id here — once —
    /// is what stops a caller stamping a row with one mount and planning it
    /// against another.
    ///
    /// `mount` is `None` for every caller that means "this window": Download,
    /// Upload, the conflict check behind both. It is `Some` only for a drop
    /// carrying another window's selection (E24-S2), where the source share is
    /// named in the payload rather than implied by who is asking.
    async fn mount_and_backend(
        &self,
        window: &tauri::Window,
        mount: Option<&str>,
    ) -> Result<(String, Arc<dyn ShareBackend>)> {
        let Some(id) = mount
            .map(str::to_owned)
            .or(self.sessions.active(window.label()).await)
        else {
            return Err(EngineError::Disconnected("no share is mounted".to_owned()));
        };
        let backend = self
            .sessions
            .backend(&id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))?;
        Ok((id, backend))
    }
}

/// Tell every window on this window's mount that its listing changed.
///
/// The other half of "two windows on one share stay in step": a rename in one
/// window is a stale row in the other, and neither of them is polling. The
/// windows feed this into the 250 ms coalescer they already had, so twenty
/// deletions are one relist rather than twenty (E8-S9).
///
/// The window that did the mutation is included rather than excluded. It
/// refreshes locally too, and the coalescer collapses the pair — leaving it out
/// would buy nothing and would make the event mean something subtly different
/// from "this listing changed".
async fn announce_listing(sessions: &SessionManager, window: &tauri::Window) {
    let Some(id) = sessions.active(window.label()).await else {
        return;
    };
    announce_listing_on(sessions, window, &id).await;
}

/// The same, for a mutation on a share the calling window is not showing.
///
/// A drop can change a share that no window in this app has in front of it —
/// a move onto a sidebar row for a mount another window holds (E24-S2) — and
/// telling the *caller's* windows would refresh the wrong lists while leaving
/// the changed one stale.
async fn announce_listing_on(sessions: &SessionManager, window: &tauri::Window, id: &str) {
    for label in sessions.windows_on(id).await {
        let _ = window
            .app_handle()
            .emit_to(label.as_str(), LISTING_CHANGED, id);
    }
}

/// Fired at every window on a mount whose contents this app just changed.
const LISTING_CHANGED: &str = "listing://changed";

/// Errors as the frontend sees them: a message and where it belongs on screen.
///
/// The engine's error type does not cross IPC, so a new variant there cannot
/// silently become an untyped string here.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    message: String,
    surface: excalibur_engine::error::Surface,
    transient: bool,
    /// Set for the two host-key failures.
    ///
    /// The message already says all of this in a sentence, but a fingerprint is
    /// something the user compares character by character against what the
    /// server operator told them. That needs its own element, in a monospace
    /// font, not a substring of a paragraph — so it travels as data.
    #[serde(skip_serializing_if = "Option::is_none")]
    host_key: Option<HostKeyProblem>,
}

/// The host-key detail behind a [`CommandError`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HostKeyProblem {
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
    /// The previously accepted fingerprint, when the key changed. `None` means
    /// the host is simply unknown, which is a question rather than an alarm.
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
}

impl From<excalibur_engine::EngineError> for CommandError {
    fn from(error: excalibur_engine::EngineError) -> Self {
        let host_key = match &error {
            EngineError::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint,
            } => Some(HostKeyProblem {
                host: host.clone(),
                port: *port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                expected: None,
            }),
            EngineError::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint,
                expected,
            } => Some(HostKeyProblem {
                host: host.clone(),
                port: *port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                expected: Some(expected.clone()),
            }),
            _ => None,
        };
        Self {
            message: error.to_string(),
            surface: error.surface(),
            transient: error.transient(),
            host_key,
        }
    }
}

/// One directory, with the mount facts that must not drift from it.
#[tauri::command]
async fn list_dir(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<Listing, CommandError> {
    state
        .backend(&window)
        .await?
        .list_dir(&path)
        .await
        .map_err(Into::into)
}

/// The mount facts alone, for a status bar refresh that does not need a
/// listing.
#[tauri::command]
async fn mount_info(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<MountInfo, CommandError> {
    Ok(state.backend(&window).await?.mount_info())
}

/// Try a connection and report what it found, without keeping it.
#[tauri::command]
async fn test_connection(
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<MountInfo, CommandError> {
    state.sessions.test(&spec).await.map_err(Into::into)
}

/// Runs a blocking call off the UI thread.
///
/// Tauri runs a non-`async` command on the main thread, and every keychain call
/// below is blocking — on macOS it can put up a system prompt and sit there
/// until the user answers. That would freeze the window, so these commands are
/// `async` and hand the actual call to the blocking pool.
///
/// # Panics
///
/// Panics if the closure panics, which propagates the original panic rather
/// than turning it into an error the window would have to invent a message for.
async fn blocking<T, F>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .expect("keychain worker panicked")
}

/// Whether this machine has a credential store that actually keeps things.
///
/// A Linux desktop with no Secret Service has a keyring API that accepts calls
/// and stores nothing, so the window asks before offering to save.
#[tauri::command]
async fn keychain_available(state: tauri::State<'_, AppState>) -> std::result::Result<bool, ()> {
    let secrets = Arc::clone(&state.secrets);
    Ok(blocking(move || secrets.available()).await)
}

/// Which protocols this build can actually connect.
///
/// Compiled rather than listed, because SMB is behind a cargo feature: one
/// frontend bundle serves a build with it and a build without, so a static
/// list in the window would be wrong for one of them. Letting a user fill in a
/// form that cannot connect is worse than telling them up front.
#[tauri::command]
async fn supported_protocols() -> std::result::Result<Vec<&'static str>, ()> {
    // `local` is unconditional: it is the standard library, so there is no
    // build of this binary that cannot open a folder on the machine it is
    // running on (E21-S2).
    let mut protocols = vec!["xsync", "sftp", "nfs", "local"];
    if cfg!(feature = "smb") {
        protocols.push("smb");
    }
    Ok(protocols)
}

/// How many transfers may run at once (E17-S1).
///
/// The preference lives in the window, the semaphore lives in the engine, so
/// the value has to travel. Raising takes effect at once; lowering takes
/// effect as running transfers finish, because recalling a permit already held
/// would mean stopping work the user did not ask to stop.
#[tauri::command]
async fn set_transfer_concurrency(
    limit: usize,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.worker.set_concurrency(limit);
    Ok(())
}

/// Save a credential for a bookmark.
///
/// There is deliberately **no command to read one back**. The window collects a
/// password once, when the user types it, and never sees it again; the engine
/// fetches it from the keychain when it connects. Adding a getter here would
/// undo that with one line, so its absence is the design.
#[tauri::command]
async fn save_secret(
    state: tauri::State<'_, AppState>,
    account: String,
    secret: String,
) -> std::result::Result<(), CommandError> {
    let secrets = Arc::clone(&state.secrets);
    let secret = Secret::new(secret);
    blocking(move || secrets.set(&account, &secret))
        .await
        .map_err(Into::into)
}

/// Forget a credential: the box was unticked, or the bookmark was removed.
#[tauri::command]
async fn forget_secret(
    state: tauri::State<'_, AppState>,
    account: String,
) -> std::result::Result<(), CommandError> {
    let secrets = Arc::clone(&state.secrets);
    blocking(move || secrets.delete(&account))
        .await
        .map_err(Into::into)
}

/// Accept a host's SSH key, after the user compared its fingerprint.
///
/// Takes the host and port rather than the fingerprint the window was shown:
/// re-scanning means the key that gets recorded is one the host is offering
/// now, so a stale prompt left open on screen cannot be used to trust a key
/// the host has already stopped presenting.
#[tauri::command]
async fn trust_host_key(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
) -> std::result::Result<(), CommandError> {
    let known = state.known_hosts.clone();
    let scanned = hostkeys::HostKeyScanner::scan(&hostkeys::Keyscan, &host, port).await?;
    blocking(move || {
        let Some(key) = scanned.first() else {
            return Err(EngineError::Disconnected(format!(
                "{host}:{port} offered no SSH host keys"
            )));
        };
        known.trust(key)
    })
    .await
    .map_err(Into::into)
}

/// Forget every accepted key for a host, so the next connection asks again.
///
/// This is the deliberate second step behind a changed key: the banner that
/// reports the change does not call it, and cannot. The bookmark editor does,
/// which means clearing the alarm takes leaving the failure and going to find
/// the entry it belongs to.
#[tauri::command]
async fn forget_host_key(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
) -> std::result::Result<(), CommandError> {
    let known = state.known_hosts.clone();
    blocking(move || known.forget(&host, port))
        .await
        .map_err(Into::into)
}

/// Create one directory. Not `mkdir -p`: the parent must exist.
#[tauri::command]
async fn mkdir(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<(), CommandError> {
    state.backend(&window).await?.mkdir(&path).await?;
    announce_listing(&state.sessions, &window).await;
    Ok(())
}

/// Rename, refusing to clobber unless `replace` says otherwise.
#[tauri::command]
async fn rename(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
    replace: bool,
    mount: Option<String>,
) -> std::result::Result<(), CommandError> {
    // `mount` is given only by a drop from another window onto a share this
    // window is not the one looking at — a sidebar row for a mount someone
    // else has open (E24-S2). A move is a `rename` on one share, and the share
    // is the one the *items* are on, not the one the window happens to show.
    let (id, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    backend.rename(&from, &to, replace).await?;
    announce_listing_on(&state.sessions, &window, &id).await;
    Ok(())
}

/// Remove a file, or a directory and everything under it.
#[tauri::command]
async fn remove(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    recursive: bool,
) -> std::result::Result<(), CommandError> {
    state
        .backend(&window)
        .await?
        .remove(&path, recursive)
        .await?;
    announce_listing(&state.sessions, &window).await;
    Ok(())
}

/// Everything in the queue, oldest first.
///
/// The window reads this on open and after any lag on the event stream. A
/// broadcast channel drops messages for a slow consumer without telling it, so
/// there has to be a way back to the truth — the sibling engine has none, and
/// its queue view can desync permanently.
#[tauri::command]
async fn transfers(state: tauri::State<'_, AppState>) -> std::result::Result<Vec<Transfer>, ()> {
    Ok(state.transfers.all().await)
}

/// Queue a download of each path, and start moving.
///
/// **A folder is one row**, at `<destination>/<folder>`, exactly as
/// [`enqueue_uploads`] makes it one row — the two commands are the same shape
/// again for the first time since E22-S2.
///
/// It used to be one row per file, walked here by
/// [`plan::plan_downloads`](excalibur_engine::transfer::plan::plan_downloads)
/// before any row existed, on the argument (E13-S4) that a single row for a
/// directory can only report a percentage that means nothing once one file in
/// it fails. E22-S3 withdrew that argument on a measurement rather than a
/// preference. Every row is run by `Worker::attempt`, and every attempt is a
/// whole `sync_endpoints` session — a scoped thread pool, a cancel watcher and
/// a journal, set up and torn down to move one file. On this machine a folder
/// of 1002 files took **21.7 s** that way and **0.31 s** as one session, and
/// only 0.30 s of the 21.7 was spent talking to the share at all; 2000 small
/// files were 38.3 s against 1.1 s. The per-file row was 70× of pure
/// scheduling.
///
/// What the walk here used to buy, and where each part went:
///
/// - **the sizes on the rows** are now the planner's `Planned` event, which is
///   the same scan the transfer was going to do anyway rather than a second
///   one over the browse session;
/// - **an empty remote directory** is created by the sink as it walks, not by
///   this command creating `plan.directories` first;
/// - **a subdirectory that could not be listed** is a per-entry failure inside
///   the session, and [`endpoint::run`](excalibur_engine::transfer::endpoint)
///   refuses to report success for a run with any: the row fails with the
///   count and the first reason. It is one failed row for the folder instead of
///   one failed row beside the ones that worked, which is a real loss of
///   detail and the only thing this change gives up.
///
/// [`plan::plan_downloads`] itself is still there — the CLI's `get` uses it,
/// and it is still the walk that says what a selection *would* copy.
#[tauri::command]
async fn enqueue_downloads(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    mount: Option<String>,
) -> std::result::Result<Vec<String>, CommandError> {
    // `mount` names the share the *paths* are on. Absent for Download, where it
    // is this window's; present for a drop carrying another window's selection
    // (E24-S2), where the answer is in the payload and not in who is asking.
    let (mount, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let root = std::path::PathBuf::from(destination.trim_end_matches('/'));
    let mut ids = Vec::new();
    // The one directory this command still creates. The session creates
    // everything under it, but it opens its sink *at* this folder, and a
    // download into a folder that is not there should say so on a row rather
    // than as a per-entry failure halfway down a tree.
    if let Err(error) = std::fs::create_dir_all(&root) {
        ids.push(
            report_problem(
                &state.transfers,
                String::new(),
                root.to_string_lossy().into_owned(),
                format!("create {}: {error}", root.display()),
            )
            .await,
        );
        return Ok(ids);
    }
    for path in paths {
        // Names from the server are not paths (E13-S4's other rule, and the
        // one worth keeping whatever the row shape is). A listing that answers
        // `..` is describing a write outside the folder the user chose.
        let name = plan::local_name(&path).map(ToOwned::to_owned);
        let Some(name) = name else {
            ids.push(
                report_problem(
                    &state.transfers,
                    path.clone(),
                    root.to_string_lossy().into_owned(),
                    format!("{path:?} is not a name that can be written locally"),
                )
                .await,
            );
            continue;
        };
        // A directory has no size worth showing: its own `stat` size is an
        // inode's, and the planner's `Planned` event fills the real one in a
        // moment later. A stat that fails is not a reason to drop the item —
        // the session will report what is actually wrong with it, on the row
        // the user is looking at.
        let size = match backend.stat(&path).await {
            Ok(attrs) if attrs.kind != EntryKind::Dir => attrs.size,
            _ => 0,
        };
        let id = new_id();
        state
            .transfers
            .add(
                Transfer::new(
                    id.clone(),
                    Direction::Down,
                    path,
                    root.join(name).to_string_lossy().into_owned(),
                    size,
                )
                .on_mount(&mount),
            )
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Put something the walk could not do in the queue, already failed.
///
/// A row rather than an error from the command: one unreadable subdirectory
/// must not cancel the enqueue of everything beside it, and the transfer sheet
/// is where a person looks to find out what a copy actually did.
async fn report_problem(
    queue: &TransferQueue,
    source: String,
    destination: String,
    reason: String,
) -> String {
    let id = new_id();
    let mut transfer = Transfer::new(id.clone(), Direction::Down, source, destination, 0);
    transfer.status = TransferStatus::Failed;
    transfer.error = Some(reason);
    queue.add(transfer).await;
    id
}

/// Queue an upload of each local path into `destination` on the share.
///
/// `mount` names the share `destination` is on: absent for Upload and for a
/// Finder drop, both of which mean this window's, and present for a drop from
/// another window (E24-S2). It is recorded on every row rather than resolved
/// when the worker gets there, because between the enqueue and the start the
/// user can put a different share in front of themselves.
///
/// A **folder is one row**, and there is deliberately no walk here to match
/// [`enqueue_downloads`]'s. Since E22-S2 the transfer runs xsync's planner,
/// which does the walk itself — and doing it twice would stat every file over
/// the browse session before the transfer had sent a byte, then have the
/// planner stat them all again. Before E22-S2 there was no walk on this route
/// at all, which is why a dropped folder failed with `EISDIR`.
#[tauri::command]
async fn enqueue_uploads(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    mount: Option<String>,
) -> std::result::Result<Vec<String>, CommandError> {
    let (mount, _) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let mut ids = Vec::new();
    for path in paths {
        // A directory's own `len()` is its entry size — 96 bytes on APFS —
        // which as a progress total is worse than admitting we do not know
        // yet. The planner's `Planned` event fills it in a moment later.
        let size =
            std::fs::metadata(&path).map_or(0, |meta| if meta.is_dir() { 0 } else { meta.len() });
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
        let dest = if destination.is_empty() {
            name
        } else {
            format!("{}/{name}", destination.trim_end_matches('/'))
        };
        let id = new_id();
        state
            .transfers
            .add(Transfer::new(id.clone(), Direction::Up, path, dest, size).on_mount(&mount))
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Pause one transfer. It keeps its partial, which is its resume point.
///
/// Since E22-S3 "one transfer" can be a whole folder, and the granularity is
/// the row: pausing a folder stops the session, which drains what is in flight
/// and leaves the files it already published where they are.
#[tauri::command]
async fn pause_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Paused).await;
    Ok(())
}

/// Cancel one transfer. Its partial is not kept.
///
/// "Its partial" is the file that was mid-flight: the engine never publishes a
/// staged file it did not finish and verify. For a **folder** row (E22-S3) the
/// files that already landed do stay — a cancel stops a copy, it does not undo
/// one, and removing a tree the user can see arriving is not something a Cancel
/// button should do on their behalf. Retrying the row resumes into the same
/// destination and the planner skips what is already there.
#[tauri::command]
async fn cancel_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Cancelled).await;
    Ok(())
}

/// Put a paused or failed transfer back in the queue.
///
/// A retried **folder** row (E22-S3) is not a second copy of the tree: the
/// session runs the planner again, which skips every file already at the
/// destination on size and mtime, and a file that was torn mid-flight is picked
/// up from the resume journal rather than from the start. So the granularity
/// being the folder costs a re-scan and nothing else — which is the trade that
/// makes one row per folder work at all.
#[tauri::command]
async fn retry_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state
        .transfers
        .update(&id, |transfer| {
            if !matches!(transfer.status, TransferStatus::Running) {
                transfer.status = TransferStatus::Queued;
                transfer.error = None;
            }
        })
        .await;
    run_queue(&state);
    Ok(())
}

/// Pause everything unfinished.
#[tauri::command]
async fn pause_all_transfers(state: tauri::State<'_, AppState>) -> std::result::Result<(), ()> {
    for transfer in state.transfers.pause_all().await {
        state
            .worker
            .stop(&transfer.id, TransferStatus::Paused)
            .await;
    }
    Ok(())
}

/// Drop everything finished or cancelled.
#[tauri::command]
async fn clear_finished_transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.transfers.clear_finished().await;
    Ok(())
}

/// The folder downloads land in by default.
#[tauri::command]
fn download_folder() -> String {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folders the sidebar's FAVORITES section is seeded from.
///
/// Asked of the OS rather than assembled in the window, because every one of
/// them is a fact the platform owns and the webview cannot see: the home
/// directory has no browser API, and the media folders are neither named nor
/// placed the same way twice — macOS keeps films in `~/Movies` and Linux in
/// whatever `XDG_VIDEOS_DIR` says, which on a localised desktop is not an
/// English word at all. Guessing `~/Videos` everywhere would seed a row for a
/// folder that is not there.
///
/// An empty string means *this platform has no such folder*, which the window
/// drops. It is not an error: Linux has no Applications directory a person
/// browses, and `dirs` answers `None` for a media folder the user never made.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UserFolders {
    home: String,
    applications: String,
    desktop: String,
    documents: String,
    pictures: String,
    /// `~/Movies` on macOS, `XDG_VIDEOS_DIR` on Linux. The window labels it by
    /// its own last path segment rather than calling it "Videos" everywhere,
    /// so the row reads as the folder the user already has.
    videos: String,
}

/// Where the sidebar's default favourites live on this machine.
#[tauri::command]
fn user_folders() -> UserFolders {
    fn path(folder: Option<std::path::PathBuf>) -> String {
        folder
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    UserFolders {
        home: path(dirs::home_dir()),
        // `dirs` has no applications folder, because most platforms have no
        // such thing. macOS does, at a fixed absolute path; everywhere else
        // this is deliberately empty rather than pointed at
        // `/usr/share/applications`, which holds `.desktop` files and is not a
        // folder anyone means by "Applications".
        applications: if cfg!(target_os = "macos") {
            "/Applications".to_owned()
        } else {
            String::new()
        },
        desktop: path(dirs::desktop_dir()),
        documents: path(dirs::document_dir()),
        pictures: path(dirs::picture_dir()),
        videos: path(dirs::video_dir()),
    }
}

/// Which of `paths` are directories that exist right now.
///
/// The sidebar hides a favourite whose folder is gone — an external disk
/// ejected, a project deleted — because the only thing clicking it could do is
/// fail. Answered in a batch rather than one call per row: the list is checked
/// on every load, and a round trip per favourite is a round trip per favourite.
///
/// Symlinks are followed (`metadata`, not `symlink_metadata`): a favourite
/// pointing at a link to a real directory opens that directory, so it is a
/// directory as far as this question goes.
#[tauri::command]
fn existing_directories(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .collect()
}

/// Whether a destination is already taken, and what `Keep both` would call it.
///
/// Answered by the engine rather than the window because the naming rule —
/// the suffix goes before the extension, numbering skips what is taken — is
/// the kind of thing two implementations would disagree about.
///
/// **This asks about the items the user picked, not about what is inside
/// them**, and it always did — which is why E22-S3 could make a folder one row
/// without touching this function. A folder named in `paths` clashes when a
/// folder of that name is already at the destination, and the three answers
/// mean, at that granularity:
///
/// - **Skip** drops the folder from the batch; nothing beneath it is looked at.
/// - **Replace** enqueues it, and the session merges into what is there —
///   same-named files are overwritten, ones the source does not have are left
///   alone. It is not a delete-then-copy, because `delete` is off in
///   `LocalSyncOptions` and stays off: a transfer must not remove files at the
///   destination nobody asked about (E22-S1). A folder-level "Replace" is
///   therefore weaker than a file-level one, and that is the honest reading of
///   what a copy can promise.
/// - **Keep both** offers a free name, and **the window does not yet use it** —
///   `resolveConflicts` re-sends the original paths, so it currently behaves as
///   Replace. That is a pre-existing gap, older than this story and not made
///   worse by it: applying the suggested name needs a per-path destination in
///   `enqueue_downloads`/`enqueue_uploads`, which neither command takes.
#[tauri::command]
async fn check_conflicts(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    upload: bool,
    mount: Option<String>,
) -> std::result::Result<Vec<Conflict>, CommandError> {
    // The remote side of the check, which is the destination for an upload and
    // the source for a download. Named explicitly by a cross-window drop, and
    // this window's for everything else.
    let (_, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let mut conflicts = Vec::new();
    for path in paths {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
        let dest = format!("{}/{name}", destination.trim_end_matches('/'));
        let exists = if upload {
            backend.stat(&dest).await.is_ok()
        } else {
            std::path::Path::new(&dest).exists()
        };
        if !exists {
            continue;
        }
        // `taken` is answered against the same side the file is going to, so
        // the suggested name is one that is actually free there.
        let directory = destination.trim_end_matches('/').to_owned();
        let mut used = Vec::new();
        if upload {
            if let Ok(listing) = backend.list_dir(&directory).await {
                used.extend(listing.entries.into_iter().map(|entry| entry.name));
            }
        } else if let Ok(entries) = std::fs::read_dir(&directory) {
            used.extend(
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned()),
            );
        }
        let keep_both_as = conflict::keep_both_name(&name, &|candidate| {
            used.iter().any(|taken| taken == candidate)
        });
        conflicts.push(Conflict {
            transfer_id: String::new(),
            name,
            dest_path: dest,
            keep_both_as,
        });
    }
    Ok(conflicts)
}

/// Check a file out, open it in the OS editor, and watch it for saves.
///
/// The download and the watch both happen here rather than in the window,
/// because a save can arrive at any time and the window may not be looking.
#[tauri::command]
async fn edit_file(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<EditSession, CommandError> {
    let backend = state.backend(&window).await?;
    let session = state.edits.checkout(&backend, &path).await?;
    open_in_editor(&session.local_path);
    // Saves go back to the window that opened the file, not to every window:
    // the write-back toast, and the conflict prompt behind it, belong to
    // whoever is editing. A second window on the same share has no editor open
    // and nothing to say about one.
    watch_for_saves(
        window.app_handle().clone(),
        window.label().to_owned(),
        session.clone(),
    );
    Ok(session)
}

/// Write a working copy back.
///
/// `force` is the conflict prompt's "Replace". Without it a moved remote comes
/// back as `Conflict` rather than being overwritten.
#[tauri::command]
async fn save_edit(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    id: String,
    force: bool,
) -> std::result::Result<SaveOutcome, CommandError> {
    let backend = state.backend(&window).await?;
    let outcome = state.edits.save(&backend, &id, force).await?;
    // A write-back is a mutation like any other: the other window on this share
    // is showing a size and a date that just changed.
    if matches!(outcome, SaveOutcome::Saved { .. }) {
        announce_listing(&state.sessions, &window).await;
    }
    Ok(outcome)
}

/// Every file currently checked out.
#[tauri::command]
async fn edits(state: tauri::State<'_, AppState>) -> std::result::Result<Vec<EditSession>, ()> {
    Ok(state.edits.all().await)
}

/// Finish with a file and remove its working copy.
#[tauri::command]
async fn close_edit(state: tauri::State<'_, AppState>, id: String) -> std::result::Result<(), ()> {
    state.edits.close(&id).await;
    Ok(())
}

/// Hand a path to whatever the OS opens it with.
///
/// `open` on macOS, `xdg-open` elsewhere on Unix, `explorer` on Windows. The
/// child is spawned and forgotten: waiting on an editor would block for as
/// long as the user is editing.
fn open_in_editor(path: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    #[cfg(windows)]
    let program = "explorer";
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// Tell the window whenever a working copy is written.
///
/// Debounced: an editor's save is often several filesystem events — a truncate,
/// a write, an mtime touch, sometimes a rename over the top — and one save
/// should be one write-back rather than four.
fn watch_for_saves(app: tauri::AppHandle, window: String, session: EditSession) {
    use notify::{RecursiveMode, Watcher};

    let local = std::path::PathBuf::from(&session.local_path);
    let Some(directory) = local.parent().map(std::path::Path::to_path_buf) else {
        return;
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok(mut watcher) = notify::recommended_watcher(sender) else {
            return;
        };
        // The *directory*, not the file: an editor that saves by writing a
        // temporary and renaming it over the top replaces the inode, and a
        // watch on the file itself would follow the old one into the bin.
        if watcher
            .watch(&directory, RecursiveMode::NonRecursive)
            .is_err()
        {
            return;
        }
        // `None` on a machine whose clock has barely started, which simply
        // means the first event is not debounced against a previous one.
        let mut last: Option<std::time::Instant> = None;
        while let Ok(Ok(event)) = receiver.recv() {
            if !event.paths.iter().any(|path| path == &local) {
                continue;
            }
            if !matches!(
                event.kind,
                notify::EventKind::Create(_) | notify::EventKind::Modify(_)
            ) {
                continue;
            }
            if last.is_some_and(|last| last.elapsed() < SAVE_DEBOUNCE) {
                continue;
            }
            last = Some(std::time::Instant::now());
            if app
                .emit_to(window.as_str(), "edit://saved", &session)
                .is_err()
            {
                return;
            }
        }
    });
}

/// Filesystem events closer together than this are one save.
const SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// Whether a host is answering, for the sidebar's dimmed rows.
///
/// Deliberately weaker than mounting: this runs for every saved bookmark on a
/// timer, and mounting each one would ask servers for credentials nobody typed.
#[tauri::command]
async fn reachable(host: String, port: u16) -> bool {
    excalibur_engine::session::reachable(&host, port, std::time::Duration::from_secs(2)).await
}

/// Connect a share and make it the active one.
#[tauri::command]
async fn mount(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<(), CommandError> {
    state
        .sessions
        .mount(window.label(), spec)
        .await
        .map_err(Into::into)
}

/// Disconnect a share.
#[tauri::command]
async fn unmount(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state.sessions.unmount(&id).await;
    Ok(())
}

/// Reconnect a mount asking for read-write.
///
/// Returns the mount facts that came back, **including when the server refuses
/// the upgrade**. A share that is still read-only is an answer, not an error:
/// reporting it as a failure is how the recovery button becomes a loop the user
/// presses forever. The window reads `writable` off the result and shows the
/// server's reason if it is still false.
#[tauri::command]
async fn remount_read_write(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<MountInfo, CommandError> {
    state
        .sessions
        .remount_read_write(&id)
        .await
        .map_err(Into::into)
}

/// Switch which mount the calling window is looking at.
///
/// Also how a second window *joins* a share the first one already has: the
/// session is not connected again, both windows simply name it.
#[tauri::command]
async fn set_active(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state
        .sessions
        .set_active(window.label(), &id)
        .await
        .map_err(Into::into)
}

/// Name the calling window, for the platform rather than for the app.
///
/// The title the design draws is painted by the webview — it is centred over
/// the toolbar, and the native title is hidden to make room — so without this
/// the window's real title stays whatever it was built with. Every window was
/// then called "Excalibur", and the macOS Window menu, Mission Control and the
/// app switcher list windows by exactly that: two windows, two identical rows,
/// no way to tell which was which.
///
/// A command of this app's own rather than `core:window:allow-set-title` in the
/// capability. Granting the core window API is granting it to every page the
/// webview ever loads, to save writing these four lines.
#[tauri::command]
async fn set_window_title(
    window: tauri::Window,
    title: String,
) -> std::result::Result<(), CommandError> {
    window.set_title(&title).map_err(|error| CommandError {
        message: error.to_string(),
        surface: excalibur_engine::error::Surface::Banner,
        transient: false,
        host_key: None,
    })
}

/// Open a second browser window on this window's mount and folder (⌘N).
///
/// Built in Rust rather than with `WebviewWindow` from the frontend, so that
/// the label, the size, the macOS title-bar treatment and the capability that
/// grants the new window its commands are all decided in one place. A window
/// the webview created would be a window nobody had granted anything to, and
/// the first `list_dir` from it would fail on a permission error rather than on
/// anything a user could act on.
///
/// The new window's mount is set here, **before** the webview loads, so its
/// first listing has a share to come from rather than a race with a `set_active`
/// round trip. The mount id travels in the URL as well, because the window has
/// to know which of its bookmarks to draw as mounted — and the store cannot
/// tell it, since a `mounted` flag read off disk is from a previous run
/// (E1-S6).
#[tauri::command]
async fn new_window(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<String, CommandError> {
    let app = window.app_handle().clone();
    let label = next_label(&app);
    let mount = state.sessions.active(window.label()).await;
    if let Some(id) = &mount {
        // Joining, not connecting: the share is already up, and a second login
        // for it would be visible to the server and would ask the keychain
        // again for something it already answered.
        state
            .sessions
            .set_active(&label, id)
            .await
            .map_err(CommandError::from)?;
    }
    // Cascaded off the window that opened it. Stacked exactly on top, a new
    // window looks like the old one changed rather than like a second one
    // appeared.
    let at = match (window.outer_position(), window.scale_factor()) {
        (Ok(position), Ok(scale)) => {
            let logical = position.to_logical::<f64>(scale);
            Some((logical.x + 24.0, logical.y + 24.0))
        }
        _ => None,
    };
    open_window(&app, &label, mount.as_deref(), &path, at).map_err(|error| CommandError {
        message: format!("could not open a window: {error}"),
        surface: excalibur_engine::error::Surface::Banner,
        transient: false,
        host_key: None,
    })?;
    Ok(label)
}

/// Start a native drag of this window's selection (E24-S2, E24-S1).
///
/// `image` is the drag image the webview rendered, as PNG **bytes**. Never a
/// path: an item on a share has no local file to point at, and writing one out
/// to name it would be a fetch nobody asked for.
///
/// `folders` is the subset of `paths` the list drew as directories. It is the
/// one thing the engine cannot work out from a path — a folder called `Dune`
/// has no extension to classify — and getting it wrong promises a folder as a
/// file, which Finder accepts and then writes nothing into.
///
/// What crosses to `AppKit` is one pasteboard type, `dev.excalibur.selection`,
/// carrying the mount, the paths and this window's label, and beside it one
/// file promise per item. The promises are what let the same drag end in
/// Finder; nothing is fetched for them until Finder asks.
#[tauri::command]
async fn begin_drag(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    folders: Vec<String>,
    image: Vec<u8>,
) -> std::result::Result<(), CommandError> {
    if paths.is_empty() {
        return Ok(());
    }
    let Some(mount) = state.sessions.active(window.label()).await else {
        return Err(CommandError::from(EngineError::Disconnected(
            "no share is mounted".to_owned(),
        )));
    };
    let selection = Selection {
        mount,
        paths,
        window: window.label().to_owned(),
    };
    let promises = drag::promises_for(&selection, &folders);
    let fulfil = promise_sink(&state);
    let app = window.app_handle().clone();
    let source = window.label().to_owned();

    // AppKit's dragging protocols are main-thread-only, and a Tauri command
    // runs on the async runtime. The hop is awaited rather than fired and
    // forgotten so a drag that could not start says so on the row the user just
    // grabbed, instead of looking like a gesture that was not recognised.
    let (done, wait) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = done.send(open_drag(
            &handle, &source, &selection, &promises, &image, &fulfil,
        ));
    })
    .map_err(|error| drag_failed(&format!("could not reach the main thread: {error}")))?;
    wait.await
        .map_err(|_| drag_failed("the main thread dropped the drag"))?
        .map_err(CommandError::from)
}

/// What fulfils a promise once Finder asks for it (E24-S1).
///
/// Built from the state's `Arc`s rather than borrowing it, because the closure
/// outlives the command: it is called from `AppKit`'s operation queue, at
/// whatever later moment somebody accepts the drop, and `tauri::State` is a
/// borrow of the app that cannot travel there.
fn promise_sink(state: &tauri::State<'_, AppState>) -> drag::PromiseSink {
    let sessions = Arc::clone(&state.sessions);
    let transfers = Arc::clone(&state.transfers);
    let worker = Arc::clone(&state.worker);
    Arc::new(move |request: &drag::PromiseRequest| {
        // Not a tokio worker thread — `AppKit` owns this one — so blocking on
        // the runtime here blocks nothing else. It is also required: the
        // promise is complete when this returns, which is how Finder knows to
        // stop showing a copy in progress.
        tauri::async_runtime::block_on(fulfil_promise(&sessions, &transfers, &worker, request))
    })
}

/// Fetch one promised item into the folder Finder named, and land it there.
///
/// **Staged in a hidden directory inside that folder, then renamed into place.**
/// Two reasons, and both of them are silent data loss without it:
///
/// - the name is Finder's, not ours. A second `Arrival.mkv` dropped on a
///   Desktop that already has one arrives as `Arrival 2.mkv`, and a transfer
///   written straight into the folder would use the name it promised and
///   overwrite the file already there;
/// - a rename inside one directory is on one filesystem, so it is atomic and
///   free. Nothing half-copied ever appears under the final name.
///
/// The transfer itself is the ordinary queue's, so a promise is visible in the
/// transfers sheet while it runs and can be paused or cancelled there — and it
/// is **one row for the whole item**, folder or file, since E22-S3.
///
/// The row is taken back out of the queue when the promise ends, whatever
/// happened. It is the one row in the app whose destination stops existing the
/// moment it finishes, and a queue file that outlived the drag that made it is
/// how this leaked: `TransferQueue::restore` re-queues anything unfinished on
/// the next launch, so an abandoned drag came back as a transfer copying into a
/// staging directory nobody was ever going to sweep. See [`retire`].
async fn fulfil_promise(
    sessions: &Arc<SessionManager>,
    transfers: &Arc<TransferQueue>,
    worker: &Arc<Worker>,
    request: &drag::PromiseRequest,
) -> Result<()> {
    let mount = &request.selection.mount;
    if sessions.backend(mount).await.is_none() {
        return Err(EngineError::Disconnected(format!(
            "{mount} is not connected"
        )));
    }
    let staging = PromiseStaging::inside(request.folder())?;
    let landed = fetch_into(transfers, worker, request, staging.path()).await;
    match landed {
        Ok(landed) => place(&landed, &request.destination),
        Err(error) => Err(error),
    }
    // `staging` is swept here, on every path out including a panic.
}

/// A promise's staging directory, swept when it goes out of scope.
///
/// A guard rather than a `remove_dir_all` at the end of [`fulfil_promise`],
/// because what used to sit there was a comment asking the reader to keep an
/// invariant — "every early return from here on has to take the staging
/// directory with it". A `?` kept it. The `catch_unwind` one frame up in
/// `drag::macos`, which turns a panic in this sink into a refused promise, did
/// not; nor did a `⌘Q` accepted while a copy was still running. Three
/// `.excalibur-*` directories holding 42 MB were found in a real Downloads
/// folder, which is the cost of an invariant a type was not asked to keep.
struct PromiseStaging(std::path::PathBuf);

impl PromiseStaging {
    /// A fresh hidden directory inside the folder Finder named.
    fn inside(folder: &Path) -> Result<Self> {
        let directory = folder.join(format!(".excalibur-{}", new_id()));
        std::fs::create_dir_all(&directory)
            .map_err(|error| EngineError::Io(format!("create {}: {error}", directory.display())))?;
        Ok(Self(directory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PromiseStaging {
    fn drop(&mut self) {
        // Best effort, like every other sweep in this app: failing a drag that
        // worked because its scaffolding could not be removed is the worse
        // trade. [`sweep_abandoned_staging`] is the backstop for a process that
        // died before getting here.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the transfer, and answer with the one thing that landed in `staging`.
async fn fetch_into(
    transfers: &Arc<TransferQueue>,
    worker: &Arc<Worker>,
    request: &drag::PromiseRequest,
    staging: &Path,
) -> Result<std::path::PathBuf> {
    // Names from the server are not paths, the same rule `enqueue_downloads`
    // keeps — and here it guards a write into somebody's Desktop.
    let name = plan::local_name(&request.path).ok_or_else(|| {
        EngineError::Io(format!(
            "{:?} is not a name that can be written locally",
            request.path
        ))
    })?;
    // Subscribed before anything is queued: a small file can finish before the
    // wait below starts, and a wait that missed its own transfer would hold
    // Finder's progress open until the queue happened to change again.
    let changes = transfers.events();
    let id = new_id();
    transfers
        .add(
            Transfer::new(
                id.clone(),
                Direction::Down,
                request.path.clone(),
                staging.join(name).to_string_lossy().into_owned(),
                0,
            )
            .on_mount(&request.selection.mount),
        )
        .await;
    let running = Arc::clone(worker);
    tauri::async_runtime::spawn(async move { running.drain().await });

    let settled = settle(transfers, changes, &id).await;
    retire(transfers, worker, &id).await;
    settled?;
    only_child(staging)
}

/// Wait for one row to stop moving, or say why it did not land.
///
/// The broadcast channel is a **wake-up**, not the record: it drops messages
/// for a slow consumer without telling it, so every pass re-reads the queue,
/// which is the only thing that knows the truth. The timeout is what makes a
/// dropped or closed channel a slower wait rather than one that never ends.
async fn settle(
    transfers: &Arc<TransferQueue>,
    mut changes: tokio::sync::broadcast::Receiver<QueueEvent>,
    id: &str,
) -> Result<()> {
    loop {
        let row = transfers.all().await.into_iter().find(|row| row.id == id);
        // A row cleared out from under us — `Clear finished` landing between the
        // copy ending and this pass — is one nobody can report on any more, so
        // the wait ends and `only_child` becomes the judge of whether anything
        // arrived. Inventing a failure here would refuse a promise that worked.
        let Some(row) = row else {
            return Ok(());
        };
        match row.status {
            TransferStatus::Done => return Ok(()),
            // Paused counts as an ending, not as a wait. The user pausing a
            // queue must not leave Finder spinning on a copy that is never
            // going to move again.
            TransferStatus::Failed | TransferStatus::Cancelled | TransferStatus::Paused => {
                return Err(EngineError::Io(
                    row.error
                        .clone()
                        .unwrap_or_else(|| format!("{} did not finish", row.name)),
                ));
            }
            TransferStatus::Queued | TransferStatus::Running => {}
        }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), changes.recv()).await;
    }
}

/// How long a retired promise waits for its session to let go of the stage.
///
/// A cancel reaches the engine on its own poll — a tenth of a second in both
/// adapters — and the session then finishes the write it is inside before it
/// returns. Two seconds is far past that and short enough that a wedged session
/// does not hold up the drag; the sweep goes ahead either way, because a
/// directory removed while something is still writing is a worse outcome only
/// for that write, and a directory never removed is litter forever.
const RETIRE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop a promise's row, wait for the session to let go, and forget the row.
///
/// Both halves matter and neither used to happen. **Stopping** is what a
/// cancelled or paused promise needs: the old code returned on the first bad
/// row and left its siblings running, so the staging directory was removed and
/// then re-created underneath by a session still writing into it —
/// `LocalSink` calls `create_dir_all` on its own root. **Forgetting** is what a
/// restart needs: a row that names a staging directory must never be in the
/// file that `TransferQueue::restore` re-queues from.
async fn retire(transfers: &Arc<TransferQueue>, worker: &Arc<Worker>, id: &str) {
    worker.stop(id, TransferStatus::Cancelled).await;
    let deadline = std::time::Instant::now() + RETIRE_WAIT;
    while worker.is_busy(id).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    transfers.remove(id).await;
}

/// Remove staging directories a previous run left behind, and their rows.
///
/// The only orphan the guard on [`PromiseStaging`] cannot catch is a process
/// that stopped existing — a quit accepted mid-drag, a crash, a kill. What such
/// a run does leave is its **row**, in a queue file that names the staging
/// directory in `dest_path`, so the app can say which directories are its own
/// rather than deleting anything that matches a pattern in somebody's folder.
///
/// Called before the restored queue is drained, because draining it is the
/// thing that would otherwise copy a whole tree back into one of these.
async fn sweep_abandoned_staging(transfers: &Arc<TransferQueue>) {
    let mut doomed: Vec<String> = Vec::new();
    let mut directories: std::collections::BTreeSet<std::path::PathBuf> =
        std::collections::BTreeSet::new();
    for row in transfers.all().await {
        if let Some(staging) = staging_root(Path::new(&row.dest_path)) {
            directories.insert(staging);
            doomed.push(row.id);
        }
    }
    if doomed.is_empty() {
        return;
    }

    // Gathered first, then done once each. One abandoned drag leaves *one*
    // directory and a row per file in it — thousands, for a folder of any size
    // — so removing per row would call `remove_dir_all` on the same path
    // thousands of times and rewrite the queue file after every one of them.
    // That is quadratic in the bytes and it was seconds of a blank window
    // before the first paint.
    //
    // Off the async runtime because `remove_dir_all` blocks, and this runs on
    // the way up: a blocked runtime thread here is a window that has not been
    // built yet.
    let _ = tauri::async_runtime::spawn_blocking(move || {
        for staging in directories {
            let _ = std::fs::remove_dir_all(staging);
        }
    })
    .await;
    transfers.remove_all(&doomed).await;
}

/// The `.excalibur-*` ancestor of a path, when it has one.
///
/// Two narrowings, because this answer becomes a `remove_dir_all` and a false
/// positive is somebody's data. It matches a whole path **component**, so a
/// folder called `my.excalibur-stuff` is not one; and only a **proper
/// ancestor**, so a file the user downloaded and called `.excalibur-notes.txt`
/// is not one either — a staging directory always has the landed item inside
/// it, which is why the row's path continues past it.
fn staging_root(destination: &Path) -> Option<std::path::PathBuf> {
    let mut walked = std::path::PathBuf::new();
    let mut parts = destination.iter().peekable();
    while let Some(part) = parts.next() {
        walked.push(part);
        if parts.peek().is_some()
            && part
                .to_str()
                .is_some_and(|part| part.starts_with(".excalibur-"))
        {
            return Some(walked);
        }
    }
    None
}

/// The single entry a fulfilled promise left in its staging directory.
///
/// One, always: the plan was made from one path, so it produced either a file
/// or a tree with one root. Anything else means the walk and this function
/// disagree, which is worth an error rather than a guess about which of them.
fn only_child(staging: &Path) -> Result<std::path::PathBuf> {
    let mut entries = std::fs::read_dir(staging)
        .map_err(|error| EngineError::Io(format!("read {}: {error}", staging.display())))?
        .filter_map(std::result::Result::ok);
    let first = entries
        .next()
        .ok_or_else(|| EngineError::Io("the copy produced nothing".to_owned()))?;
    if entries.next().is_some() {
        return Err(EngineError::Io(
            "the copy produced more than one item".to_owned(),
        ));
    }
    Ok(first.path())
}

/// Move what landed to the exact path Finder asked for.
///
/// Whatever is already at that path is removed first. It is not a file of the
/// user's: Finder chose this name precisely because nothing of theirs had it,
/// and what may be there is the empty placeholder it reserved the name with.
fn place(landed: &Path, destination: &Path) -> Result<()> {
    if destination.is_dir() {
        let _ = std::fs::remove_dir_all(destination);
    } else if destination.exists() {
        let _ = std::fs::remove_file(destination);
    }
    std::fs::rename(landed, destination).map_err(|error| {
        EngineError::Io(format!("put {} in place: {error}", destination.display()))
    })
}

/// Install a drop target on every window, then begin the session. Main thread.
///
/// Every window, including the one the drag starts in: a drop back onto the
/// source window's own list — into a folder row, or onto the sidebar — is a
/// move like any other, and a source window that could not receive its own drag
/// would make the commonest gesture in a file manager the one that does
/// nothing. Windows that have no native view yet are skipped rather than
/// failing the drag; one window mid-creation must not stop a drag in another.
#[cfg(target_os = "macos")]
fn open_drag(
    app: &tauri::AppHandle,
    source: &str,
    selection: &Selection,
    promises: &[drag::Promise],
    image: &[u8],
    fulfil: &drag::PromiseSink,
) -> excalibur_engine::error::Result<()> {
    // Defensive: a drag whose end we somehow missed would leave an invisible
    // view over a window, swallowing clicks. Cheap to redo, expensive to debug.
    drag::remove_drop_targets();

    let handle = app.clone();
    let sink: drag::DropSink = Arc::new(move |label: &str, event: drag::DropEvent| {
        // To *that* window's webview, never broadcast: two windows would
        // otherwise both highlight a row for a pointer that is over one of them
        // (E23-S1's rule, applied to a third event stream).
        let name = match event.phase {
            drag::Phase::Over => "drag://over",
            drag::Phase::Drop => "drag://drop",
            drag::Phase::Leave => "drag://leave",
        };
        let _ = handle.emit_to(label, name, &event);
    });

    let mut from = None;
    for (label, other) in app.webview_windows() {
        let Ok(view) = other.ns_view() else { continue };
        drag::install_drop_target(&label, view, Arc::clone(&sink))?;
        if label == source {
            from = Some(view);
        }
    }
    let Some(view) = from else {
        drag::remove_drop_targets();
        return Err(EngineError::Unsupported(
            "the window that started the drag has no native view".to_owned(),
        ));
    };
    drag::start_drag(view, selection, promises, image, fulfil)
}

/// Linux gets this with E24-S4; until then a drag is a gesture that does nothing.
#[cfg(not(target_os = "macos"))]
fn open_drag(
    app: &tauri::AppHandle,
    source: &str,
    selection: &Selection,
    promises: &[drag::Promise],
    image: &[u8],
    fulfil: &drag::PromiseSink,
) -> excalibur_engine::error::Result<()> {
    let _ = (app, source, selection, promises, image, fulfil);
    Err(EngineError::Unsupported(
        "dragging between windows is macOS-only until E24-S4".to_owned(),
    ))
}

fn drag_failed(why: &str) -> CommandError {
    CommandError {
        message: why.to_owned(),
        surface: excalibur_engine::error::Surface::Banner,
        transient: false,
        host_key: None,
    }
}

/// Build one browser window, on a mount and a folder, optionally at a position.
///
/// Shared by ⌘N and by the Dock reopening an app that has no windows left, so
/// the two cannot drift into different sizes or different chrome.
fn open_window(
    app: &tauri::AppHandle,
    label: &str,
    mount: Option<&str>,
    path: &str,
    at: Option<(f64, f64)>,
) -> tauri::Result<()> {
    let url = format!(
        "index.html?label={}&path={}{}",
        encode(label),
        encode(path),
        mount.map_or_else(String::new, |id| format!("&mount={}", encode(id))),
    );
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(url.into()))
            .title("Excalibur")
            .inner_size(1060.0, 640.0)
            .min_inner_size(820.0, 480.0);
    if let Some((x, y)) = at {
        builder = builder.position(x, y);
    }
    // Every window's chrome, including the first: `tauri.conf.json` declares no
    // window, so there is no second copy of this to drift from.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    builder.build().map(|_| ())
}

/// A label no window currently has.
///
/// Checked against the windows that exist rather than trusted from a counter
/// alone: a label collision silently re-uses the *existing* window, so a ⌘N
/// after a close-and-reopen would look like the shortcut had stopped working.
fn next_label(app: &tauri::AppHandle) -> String {
    let open = app.webview_windows();
    // `main` first, and not only for tidiness: `capabilities/default.json`
    // grants this app's commands to `main` and to `browser-*`, and the first
    // window is the one every other name is numbered from.
    if !open.contains_key("main") {
        return "main".to_owned();
    }
    // Bounded rather than an open range: there is no number of windows worth
    // searching forever for, and a machine that has run out of labels has run
    // out of screen a long way back.
    (2..=MAX_WINDOWS)
        .map(|index| format!("browser-{index}"))
        .find(|candidate| !open.contains_key(candidate))
        .unwrap_or_else(|| "browser".to_owned())
}

/// How many browser windows can be open before ⌘N stops finding a name.
///
/// Not a policy about how many windows are useful — it is the point past which
/// searching for a free label is less sane than refusing.
const MAX_WINDOWS: usize = 64;

/// Percent-encode a value going into the new window's query string.
///
/// The counterpart of `percent_decode`, and hand-rolled for the same reason:
/// a mount id is a bookmark's id — `smb:nas.local:media` — and a path is
/// whatever the share calls its folders. Both reach `URLSearchParams` on the
/// other side, which decodes them.
fn encode(raw: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                // `write!` rather than `push_str(&format!(…))`: one allocation
                // for the whole string instead of one per escaped byte.
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Which window a stream URL names, if it names one.
///
/// `excalibur-stream://<label>/<path>`: the label is the authority, because the
/// protocol handler is given a request and not a window — there is no caller to
/// inject one. A URL with no label is answered from the app's mount rather than
/// refused, which covers the platforms that rewrite a custom scheme to
/// `http://excalibur-stream.localhost/…` and lose the authority on the way.
fn window_from_host(host: Option<&str>) -> Option<&str> {
    let host = host?;
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return None;
    }
    Some(host)
}

/// Percent-decode a URI path segment.
///
/// Hand-rolled rather than a dependency: this decodes one path, the rules are
/// three lines, and the alternative is a crate in the audit for it.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The local user's name, for an SFTP spec that did not give one.
///
/// `ssh` defaults to the same thing, so a bookmark with no username behaves
/// the way the command line would.
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_owned())
}

/// Start the worker on whatever is queued, unless it is already running.
///
/// Spawned rather than awaited: `enqueue` returns as soon as the rows exist,
/// so the window paints them immediately and the bytes move behind it.
fn run_queue(state: &tauri::State<'_, AppState>) {
    let worker = Arc::clone(&state.worker);
    tauri::async_runtime::spawn(async move { worker.drain().await });
}

/// An id unique enough to name one transfer for the life of a queue.
fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    format!("{now:x}-{:x}", SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

/// The scheme previews and media are served over.
///
/// A custom protocol rather than a command, because **file bytes must never
/// cross IPC**: a command's return value is serialised through the webview
/// bridge, which would mean holding a 40 GB film in memory to show a frame of
/// it. This streams a byte range straight from the backend to the `<video>`
/// element that asked for it.
const STREAM_SCHEME: &str = "excalibur-stream";

/// Transfers moving at once.
///
/// Three is the design's default. More saturates a link without finishing
/// anything sooner, and makes every row's ETA worse at the same time.
const DEFAULT_CONCURRENCY: usize = 3;

/// Bytes served per request when the webview does not ask for a range.
///
/// A first request with no `Range` is what a `<video>` sends to discover the
/// file; answering the whole thing would defeat the point. One megabyte is
/// enough for a container header and small enough to be instant.
const STREAM_DEFAULT_WINDOW: u64 = 1024 * 1024;

/// Largest range answered in one response, whatever was asked for.
const STREAM_MAX_WINDOW: u64 = 8 * 1024 * 1024;

/// What a `Range: bytes=…` header asked for, resolved against a known size.
///
/// Returns `None` for a syntactically valid header naming a range past the end,
/// which is the case that must answer `416` rather than be clamped into
/// something the caller did not ask for.
fn parse_range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    let Some(header) = header else {
        // No header: serve from the start, a window at a time.
        let end = size.min(STREAM_DEFAULT_WINDOW).saturating_sub(1);
        return Some((0, end));
    };
    let spec = header.strip_prefix("bytes=")?.trim();
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    if start >= size {
        return None;
    }
    let end = match end.trim() {
        "" => size - 1,
        value => value.parse::<u64>().ok()?.min(size - 1),
    };
    if end < start {
        return None;
    }
    // Capped so one request cannot ask for the whole file: a player that sends
    // `bytes=0-` is asking for everything, and answering that literally is how
    // a preview becomes a download. The player asks again for the rest.
    Some((start, end.min(start + STREAM_MAX_WINDOW - 1)))
}

/// Answer one range request from the active mount.
async fn stream_range(
    backend: Arc<dyn ShareBackend>,
    path: &str,
    range: Option<String>,
) -> http::Response<Vec<u8>> {
    let refuse = |status: http::StatusCode| {
        http::Response::builder()
            .status(status)
            .body(Vec::new())
            .unwrap_or_default()
    };

    let Ok(attrs) = backend.stat(path).await else {
        return refuse(http::StatusCode::NOT_FOUND);
    };
    let size = attrs.size;
    let Some((start, end)) = parse_range(range.as_deref(), size) else {
        return http::Response::builder()
            .status(http::StatusCode::RANGE_NOT_SATISFIABLE)
            .header(http::header::CONTENT_RANGE, format!("bytes */{size}"))
            .body(Vec::new())
            .unwrap_or_default();
    };
    let length = u32::try_from(end - start + 1).unwrap_or(u32::MAX);
    // Deliberately still `read_range` and not the streaming handle E13-S7 gave
    // transfers: a seek is a range, not a stream. Each request here is
    // unrelated to the last — a player jumps, an image loads once — so a handle
    // kept open between them would hold a file the user has already navigated
    // away from, to save an open nobody is waiting on.
    let Ok(data) = backend.read_range(path, start, length).await else {
        return refuse(http::StatusCode::INTERNAL_SERVER_ERROR);
    };
    // A short read is the end of the file, so the header describes what is
    // actually in the body rather than what was asked for.
    let end = start + data.len() as u64 - 1;

    http::Response::builder()
        .status(http::StatusCode::PARTIAL_CONTENT)
        .header(http::header::CONTENT_TYPE, mime_for(path))
        .header(http::header::ACCEPT_RANGES, "bytes")
        .header(
            http::header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}"),
        )
        .header(http::header::CONTENT_LENGTH, data.len().to_string())
        // The webview treats a custom scheme as a foreign origin.
        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(data)
        .unwrap_or_default()
}

/// A media type for the `<video>`/`<img>` element, from the name alone.
///
/// The engine's kind table already decided what this file is; this maps its
/// category onto the handful of types a browser needs told. Sniffing would mean
/// reading the file to answer a question the name answers well enough.
fn mime_for(path: &str) -> &'static str {
    use excalibur_engine::kind::Category;
    let name = path.rsplit('/').next().unwrap_or(path);
    let kind = excalibur_engine::kind::classify(name);
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ => match kind.category {
            Category::Text | Category::Code => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        },
    }
}

/// Build and run the desktop app.
///
/// # Panics
///
/// Panics if the platform has no data directory to keep accepted host keys in,
/// or if the webview cannot be created. Neither is a condition the app can
/// continue from.
/// Where the transfer queue is kept between runs.
///
/// Beside the known hosts, for the same reason: it is app state that has to
/// outlive the process, and the engine is what changes it.
fn queue_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("excalibur")
        .join("transfers.json")
}

/// Start the window.
///
/// # Panics
///
/// Panics when no data directory can be found for the host-key store. That is
/// a fatal misconfiguration rather than something to degrade around: without
/// it there is nowhere to record a trust decision, and every SSH connection
/// would ask about the same host forever.
// The builder is one expression: a plugin, a protocol handler, the managed
// state, the event bridges and the command list, each of which reads as a line
// of setup and none of which is a step that could be named separately. Splitting
// it would buy a shorter function and a longer path to what the app registers.
#[allow(clippy::too_many_lines)]
pub fn run() {
    // A store that cannot be located is a fatal misconfiguration rather than
    // something to degrade around: without it there is nowhere to record a
    // trust decision, and every SSH connection would ask again forever.
    let known_hosts = KnownHosts::app_default().expect("no data directory for known_hosts");
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore);
    let sessions = Arc::new(SessionManager::new(Arc::new(ProtocolConnector {
        known_hosts: known_hosts.clone(),
        secrets: Arc::clone(&secrets),
    })));

    // Held for the run loop below, which sees a window vanish and has to tell
    // the manager — the builder consumes the other clones.
    let closing = Arc::clone(&sessions);

    let queue_path = queue_path();
    let queue = Arc::new(TransferQueue::open(queue_path));
    let worker = Arc::new(Worker::new(
        Arc::clone(&queue),
        Arc::new(AppMount {
            sessions: Arc::clone(&sessions),
            known_hosts: known_hosts.clone(),
        }),
        DEFAULT_CONCURRENCY,
    ));

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        // The Upload picker (E13-S4). The webview's own file input yields
        // `File` objects, and the engine needs paths.
        .plugin(tauri_plugin_dialog::init())
        // Previews and media read through here, never through a command.
        .register_asynchronous_uri_scheme_protocol(STREAM_SCHEME, |context, request, responder| {
            let sessions = Arc::clone(&context.app_handle().state::<AppState>().sessions);
            // Which window asked. A protocol handler gets a request, not a
            // caller, so the label cannot be injected the way a command's is —
            // it travels in the URL instead, and the webview puts it there
            // because it knows its own label (E23-S1). Without this, two
            // windows on two shares would both preview the app's most recent
            // mount, and the second window would show the first one's files.
            let label = window_from_host(request.uri().host()).map(str::to_owned);
            // The path is percent-encoded by the webview; a name with a space
            // or a `#` in it is common enough that decoding is not optional.
            let path = percent_decode(request.uri().path().trim_start_matches('/'));
            let range = request
                .headers()
                .get(http::header::RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            tauri::async_runtime::spawn(async move {
                // Nothing mounted is not something to answer with a stand-in:
                // a preview of a file that does not exist is worse than a
                // broken image (E1-S6).
                let mount = match label.as_deref() {
                    Some(window) => sessions.active_backend(window).await,
                    None => sessions.recent_backend().await,
                };
                let response = match mount {
                    Ok(backend) => stream_range(backend, &path, range).await,
                    Err(_) => http::Response::builder()
                        .status(http::StatusCode::SERVICE_UNAVAILABLE)
                        .body(Vec::new())
                        .unwrap_or_default(),
                };
                responder.respond(response);
            });
        })
        .manage(AppState {
            sessions: Arc::clone(&sessions),
            secrets: Arc::clone(&secrets),
            known_hosts,
            edits: Arc::new(EditStore::app_default().expect("no cache directory for edits")),
            transfers: Arc::clone(&queue),
            worker: Arc::clone(&worker),
        })
        .setup(move |app| {
            // Mount changes reach the window as events rather than by polling,
            // so a share that drops is reflected while the user is looking at
            // it. Every event carries the whole state, so a missed one costs
            // nothing but latency.
            let handle = app.handle().clone();
            let mut changes = sessions.events();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = changes.recv().await {
                    // Addressed, not broadcast (E23-S1): a share dropping is
                    // news for the windows looking at that share, and a banner
                    // in a window on a different mount would be a window
                    // reporting a failure that is not its own. The engine names
                    // the audience at the moment the event happened, so a
                    // window that has since switched shares is not told about
                    // one it has left.
                    if event.audience.is_empty() {
                        // Nobody is on it: a mount that failed before any
                        // window took it, or a session outliving its window.
                        // Broadcast rather than drop, so a window that is mid
                        // connect still hears about it.
                        let _ = handle.emit("mount://changed", &event);
                        continue;
                    }
                    for label in &event.audience {
                        let _ = handle.emit_to(label.as_str(), "mount://changed", &event);
                    }
                }
            });

            // Queue changes reach the window the same way. A lagged receiver
            // drops events silently, which is why `transfers` exists as a
            // command: the window can always ask for the truth.
            let handle = app.handle().clone();
            let mut moved = queue.events();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = moved.recv().await {
                    let _ = handle.emit("transfer://progress", &event);
                }
            });

            // Anything restored from the last run is queued, so it starts —
            // but a row left over from a drag that never finished names a
            // staging directory, and draining it would refill one. Swept first,
            // in the same task, so the order is the code's rather than the
            // scheduler's.
            let resuming = Arc::clone(&worker);
            let restored = Arc::clone(&queue);
            tauri::async_runtime::spawn(async move {
                sweep_abandoned_staging(&restored).await;
                resuming.drain().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            mount_info,
            mkdir,
            rename,
            remove,
            test_connection,
            reachable,
            keychain_available,
            supported_protocols,
            set_transfer_concurrency,
            save_secret,
            forget_secret,
            trust_host_key,
            forget_host_key,
            transfers,
            enqueue_downloads,
            enqueue_uploads,
            pause_transfer,
            cancel_transfer,
            retry_transfer,
            pause_all_transfers,
            clear_finished_transfers,
            download_folder,
            user_folders,
            existing_directories,
            check_conflicts,
            edit_file,
            save_edit,
            edits,
            close_edit,
            mount,
            unmount,
            remount_read_write,
            set_active,
            set_window_title,
            new_window,
            begin_drag
        ]);

    // The menu bar exists only on macOS, and only so that ⌘Q is something this
    // app can answer for. Everywhere else Tauri's default menu is what it was.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu_with_confirming_quit)
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ITEM {
                // Through `exit` rather than terminating: that is what raises
                // `ExitRequested`, which is where the confirmation lives.
                app.exit(0);
            }
        });

    builder
        .build(tauri::generate_context!())
        .expect("the webview failed to start")
        // Built rather than `run`, because two of E23-S1's rules live in the
        // run loop and nowhere else: a window closing has to reach the session
        // manager, and the app has to survive the last one.
        .run(move |app, event| match event {
            // Once the app is actually up, which is later than it sounds: the
            // menu is installed onto `NSApp` after `setup` runs, and muda
            // resolves through the installed menu.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Ready => {
                // Before the first window, deliberately. AppKit adds a window
                // to its list when the window is *created*, and does not go
                // back for ones that already existed — so a window built from
                // `tauri.conf.json` (which is built during `build`, before any
                // of this) is permanently absent from the menu. That was the
                // whole bug: with two windows open, the first one could never
                // be picked by name.
                //
                // Which is why `tauri.conf.json` declares no window at all and
                // this opens it. It also settles a duplication `open_window`
                // used to carry by hand — the config's chrome and the builder's
                // had no shared source, and a drifted pair shows up as a second
                // window that grows a native title bar.
                #[cfg(target_os = "macos")]
                hand_window_menu_to_appkit(app);

                if app.webview_windows().is_empty() {
                    let label = next_label(app);
                    let _ = open_window(app, &label, None, "", None);
                }
            }

            // Not `CloseRequested`: a window can refuse to close, and a share
            // released before the window is actually gone is a listing that
            // fails in a window still on screen.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } => {
                let sessions = Arc::clone(&closing);
                // Spawned rather than blocked on: this is the UI thread, and
                // dropping a mount disconnects a client.
                tauri::async_runtime::spawn(async move {
                    sessions.close_window(&label).await;
                });
            }
            tauri::RunEvent::ExitRequested { api, code, .. } if !confirm_exit(app, code) => {
                api.prevent_exit();
            }
            // The Dock icon, clicked on an app that is running with no windows
            // — which is the state the line above deliberately leaves it in.
            // Without this, closing the last window would keep the app alive
            // with no way back into it, which is worse than quitting.
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => {
                // Nothing mounted, at the root: the sessions of the window that
                // closed went with it, and guessing at one of the app's other
                // mounts would put a window on a share nobody asked for.
                let label = next_label(app);
                let _ = open_window(app, &label, None, "", None);
            }
            _ => {}
        });
}

/// The id of the Quit item this app builds for itself.
#[cfg(target_os = "macos")]
const QUIT_MENU_ITEM: &str = "quit";

/// macOS's menu bar, with a Quit item this app can stand in front of and a
/// Window menu that can reach a window.
///
/// Tauri's default menu is reproduced item for item, with **two** changes.
///
/// The app menu's `Quit` is an ordinary item rather than the predefined one.
/// The predefined item sends `terminate:` straight to `NSApplication`, which
/// never reaches `RunEvent::ExitRequested` — so a ⌘Q with transfers in flight
/// would kill them without asking, which is exactly what E23-S1 says must not
/// happen. This one calls `AppHandle::exit`, which does.
///
/// The Window menu is handed to `AppKit` as the application's windows menu.
/// E23-S1
/// made a second window possible without giving anyone a way to get *back* to
/// the first: the menu offered Minimize, Zoom and Close, so a window sitting
/// behind another could only be raised by clicking whatever sliver of it
/// happened to be showing.
///
/// Everything else is copied rather than invented, and the Edit submenu is the
/// reason it has to be copied rather than dropped: `Cut`, `Copy`, `Paste` and
/// `Select All` are menu items on macOS, and a webview with no Edit menu is a
/// webview where ⌘C does nothing in a text field.
///
/// There is deliberately no `New Window` item, even though ⌘N now means
/// something. A menu accelerator is handled before the webview sees the key,
/// and the window's *path* — which ⌘N opens the new window on — lives in the
/// webview. An item here could only ever open a window at the share root.
#[cfg(target_os = "macos")]
fn menu_with_confirming_quit(
    app: &tauri::AppHandle,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

    let package = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config
            .bundle
            .publisher
            .clone()
            .map(|publisher| vec![publisher]),
        ..Default::default()
    };
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ITEM,
        format!("Quit {}", package.name),
        true,
        Some("Cmd+Q"),
    )?;

    // Handed to AppKit below, so it has to exist before the menu that holds it.
    //
    // Minimize/Zoom/Close alone left no way to reach a window that is behind
    // another: with two windows overlapping, the one underneath could only be
    // raised by finding an exposed sliver of it to click. `Bring All to Front`
    // is the standard verb for that, and the *list* of windows underneath it is
    // what actually lets one be picked by name.
    let windows = Submenu::with_id_and_items(
        app,
        WINDOW_MENU_ITEM,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::bring_all_to_front(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let menu = Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                package.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?,
            &Submenu::with_items(
                app,
                "File",
                true,
                &[&PredefinedMenuItem::close_window(app, None)?],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &windows,
        ],
    )?;

    // Not handed to AppKit here: at this point the submenu has not been attached
    // to anything, and muda resolves the `NSMenu` *through* the installed app
    // menu — it finds nothing and returns quietly, so the call would look like
    // it worked and the list would simply never appear.
    // `hand_window_menu_to_appkit` does it once the menu is installed.
    Ok(menu)
}

/// The id of the Window submenu, so it can be found again after installation.
#[cfg(target_os = "macos")]
const WINDOW_MENU_ITEM: &str = "window-menu";

/// Let `AppKit` maintain the list of open windows in the Window menu.
///
/// It appends an item per window, keeps them in step as windows open and close,
/// checkmarks the active one, and wires Cmd-backquote to cycle between them.
/// Doing it by
/// hand would mean rebuilding the menu on every window open, close and retitle
/// — three places to forget — to arrive at a worse copy of what the platform
/// already maintains.
///
/// Called on `RunEvent::Ready` rather than from the menu builder or `setup`,
/// because muda looks the `NSMenu` up through the application's *installed*
/// menu — `NSApp.mainMenu`, and its delegate. Before the menu is installed the
/// lookup fails, and it fails silently: `let Some(..) else { return }`, with no
/// error to propagate. Both earlier points were tried and both looked like they
/// had worked while leaving the menu exactly as it was.
#[cfg(target_os = "macos")]
fn hand_window_menu_to_appkit(app: &tauri::AppHandle) {
    use tauri::menu::MenuItemKind;

    let Some(menu) = app.menu() else { return };
    if let Some(MenuItemKind::Submenu(windows)) = menu.get(WINDOW_MENU_ITEM) {
        let _ = windows.set_as_windows_menu_for_nsapp();
    }
}

/// Whether the app should actually go away, given why it was asked to.
///
/// Two distinct questions wear the same event. `code: None` is *every window
/// closed*, which on a Mac is not a quit at all — the app stays in the Dock
/// with no windows, and ⌘N or the Dock icon brings one back. `code: Some(_)` is
/// a deliberate quit, and the only reason to stand in its way is work in
/// flight.
///
/// The confirmation is asked from Rust, through the dialog plugin's own API
/// rather than through its command, so `dialog:allow-ask` stays out of the
/// capability: a permission granted to the webview is a permission every page
/// in it has, and nothing in this app should be able to put a system modal on
/// screen by calling a plugin. The webview cannot ask this question anyway —
/// the quit does not come from it, and by the time one window could answer,
/// which window would be a question with no answer.
fn confirm_exit(app: &tauri::AppHandle, code: Option<i32>) -> bool {
    // Answered once. Without it the `exit` below re-enters this and asks again,
    // forever.
    static CONFIRMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;

    if code.is_none() {
        // Closing the last window is not quitting (macOS). Elsewhere it is: a
        // Linux app with no window and no tray icon is a process the user
        // cannot see and cannot stop.
        return !cfg!(target_os = "macos");
    }
    if CONFIRMED.load(Ordering::SeqCst) {
        return true;
    }
    let moving = tauri::async_runtime::block_on(app.state::<AppState>().transfers.all())
        .into_iter()
        .filter(|transfer| {
            matches!(
                transfer.status,
                TransferStatus::Running | TransferStatus::Queued
            )
        })
        .count();
    if moving == 0 {
        return true;
    }

    let quitting = app.clone();
    let noun = if moving == 1 { "transfer" } else { "transfers" };
    tauri_plugin_dialog::DialogExt::dialog(app)
        .message(format!(
            "{moving} {noun} still moving. Quitting stops them where they are; \
             a paused transfer keeps its partial and resumes next time."
        ))
        .title("Quit Excalibur?")
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
            "Quit".to_owned(),
            "Keep going".to_owned(),
        ))
        // Not `blocking_show`: this runs on the UI thread, which is the one
        // thread a blocking dialog cannot be waited on from.
        .show(move |quit| {
            if quit {
                CONFIRMED.store(true, Ordering::SeqCst);
                quitting.exit(0);
            }
        });
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        encode, parse_range, percent_decode, staging_root, window_from_host, STREAM_DEFAULT_WINDOW,
        STREAM_MAX_WINDOW,
    };

    #[test]
    fn a_row_inside_a_staging_directory_names_the_directory_to_sweep() {
        // What the startup sweep matches on. The answer has to be the staging
        // directory itself and not the file under it, because the whole tree
        // is scaffolding.
        assert_eq!(
            staging_root(Path::new(
                "/Users/x/Downloads/.excalibur-1a2b-3/Show/season 1/e01.txt"
            )),
            Some(Path::new("/Users/x/Downloads/.excalibur-1a2b-3").to_path_buf())
        );
    }

    #[test]
    fn an_ordinary_download_is_never_swept() {
        // The sweep removes directories, so a false positive is data loss. It
        // matches a whole path component: a file the user happens to have
        // called `.excalibur-notes.txt` must not take its folder with it.
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/Show/season 1/e01.txt")),
            None
        );
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/.excalibur-notes.txt")),
            None
        );
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/my.excalibur-stuff/a.txt")),
            None
        );
    }

    #[test]
    fn no_range_header_serves_a_window_from_the_start() {
        // A `<video>`'s first request has no Range: it is discovering the file.
        // Answering the whole thing would defeat the point of streaming.
        assert_eq!(
            parse_range(None, 100 * 1024 * 1024),
            Some((0, STREAM_DEFAULT_WINDOW - 1))
        );
    }

    #[test]
    fn a_small_file_is_served_whole() {
        assert_eq!(parse_range(None, 10), Some((0, 9)));
    }

    #[test]
    fn an_open_ended_range_is_capped_rather_than_answered_literally() {
        // `bytes=0-` asks for everything. Answering it is how a preview
        // becomes a download; the player asks again for the rest.
        assert_eq!(
            parse_range(Some("bytes=0-"), 40 * 1024 * 1024 * 1024),
            Some((0, STREAM_MAX_WINDOW - 1))
        );
    }

    #[test]
    fn a_closed_range_is_honoured_within_the_cap() {
        assert_eq!(parse_range(Some("bytes=100-199"), 1000), Some((100, 199)));
    }

    #[test]
    fn a_range_is_clamped_to_the_end_of_the_file() {
        assert_eq!(parse_range(Some("bytes=900-5000"), 1000), Some((900, 999)));
    }

    #[test]
    fn a_range_past_the_end_is_refused_rather_than_clamped() {
        // This must answer 416. Clamping it would serve bytes the caller did
        // not ask for, and a player seeking past the end would play them.
        assert_eq!(parse_range(Some("bytes=2000-"), 1000), None);
    }

    #[test]
    fn a_backwards_or_malformed_range_is_refused() {
        for header in ["bytes=200-100", "bytes=", "bytes=abc-", "items=0-1", "0-1"] {
            assert_eq!(parse_range(Some(header), 1000), None, "{header}");
        }
    }

    #[test]
    fn a_stream_url_names_the_window_that_asked() {
        // The authority is the window's label (E23-S1), because a protocol
        // handler is handed a request and not a caller.
        assert_eq!(window_from_host(Some("browser-2")), Some("browser-2"));
        assert_eq!(window_from_host(Some("main")), Some("main"));
    }

    #[test]
    fn a_stream_url_with_no_window_falls_back_rather_than_failing() {
        // The three shapes that carry no label: a URL written before this
        // story, one with no authority at all, and the `http://<scheme>
        // .localhost/…` rewrite that Windows and Android apply to a custom
        // scheme — which loses the authority on the way through.
        assert_eq!(window_from_host(Some("localhost")), None);
        assert_eq!(window_from_host(Some("")), None);
        assert_eq!(window_from_host(None), None);
        assert_eq!(window_from_host(Some("excalibur-stream.localhost")), None);
    }

    #[test]
    fn a_new_window_url_survives_a_bookmark_id_and_a_path() {
        // A mount id is a bookmark's id, which carries colons; a path carries
        // slashes and spaces. Both go into a query string and come back out of
        // `URLSearchParams` on the other side, so neither may be left raw —
        // an unencoded `&` in a filename would split the query in two.
        assert_eq!(encode("smb:nas.local:media"), "smb%3Anas.local%3Amedia");
        assert_eq!(
            encode("Films/Blade Runner & Co"),
            "Films%2FBlade%20Runner%20%26%20Co"
        );
        assert_eq!(encode("browser-2"), "browser-2");
        // Round-trips through this file's own decoder, which is what the
        // stream protocol uses on the way back in.
        assert_eq!(percent_decode(&encode("a b/c#d.mkv")), "a b/c#d.mkv");
    }

    #[test]
    fn a_percent_encoded_path_decodes() {
        // A space or a `#` in a filename is common, and the webview encodes it.
        assert_eq!(
            percent_decode("Blade%20Runner%232.mkv"),
            "Blade Runner#2.mkv"
        );
        assert_eq!(percent_decode("plain.txt"), "plain.txt");
        // A stray `%` is left alone rather than swallowing what follows it.
        assert_eq!(percent_decode("100%.txt"), "100%.txt");
    }
}
