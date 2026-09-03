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

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::xsync::{SshTarget, XsyncBackend};
use excalibur_engine::backend::{ConnectionSpec, Protocol, ShareBackend};
use excalibur_engine::transfer::conflict::{self, Conflict};
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::{BackendSource, Worker};
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
}

#[async_trait]
impl Connector for ProtocolConnector {
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
            // SMB, NFS and SFTP arrive with E4, E5 and E6. Until then a spec
            // naming one is answered honestly rather than silently mocked.
            other => Err(excalibur_engine::EngineError::Unsupported(format!(
                "{other:?} is not implemented yet"
            ))),
        }
    }
}

/// The worker's window onto the session manager.
///
/// A thin adapter rather than the worker taking a `SessionManager` directly:
/// the worker should not know how a mount is chosen, only that there is one,
/// which is what lets a transfer survive a reconnect that swapped the backend
/// underneath it.
struct ActiveMount {
    sessions: Arc<SessionManager>,
    demo: Arc<dyn ShareBackend>,
}

#[async_trait]
impl BackendSource for ActiveMount {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        Ok(self
            .sessions
            .active_backend()
            .await
            .unwrap_or_else(|_| Arc::clone(&self.demo)))
    }
}

/// What the shell holds: the mounts, and a demo share when there are none.
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
    /// Serves the design's fixed data until a real share is mounted, so the
    /// window has something to draw on first run.
    demo: Arc<dyn ShareBackend>,
}

impl AppState {
    /// The backend a command should use: the active mount, or the demo.
    async fn backend(&self) -> Arc<dyn ShareBackend> {
        self.sessions
            .active_backend()
            .await
            .unwrap_or_else(|_| Arc::clone(&self.demo))
    }
}

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
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<Listing, CommandError> {
    state
        .backend()
        .await
        .list_dir(&path)
        .await
        .map_err(Into::into)
}

/// The mount facts alone, for a status bar refresh that does not need a
/// listing.
#[tauri::command]
async fn mount_info(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<MountInfo, CommandError> {
    Ok(state.backend().await.mount_info())
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
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<(), CommandError> {
    state.backend().await.mkdir(&path).await.map_err(Into::into)
}

/// Rename, refusing to clobber unless `replace` says otherwise.
#[tauri::command]
async fn rename(
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
    replace: bool,
) -> std::result::Result<(), CommandError> {
    state
        .backend()
        .await
        .rename(&from, &to, replace)
        .await
        .map_err(Into::into)
}

/// Remove a file, or a directory and everything under it.
#[tauri::command]
async fn remove(
    state: tauri::State<'_, AppState>,
    path: String,
    recursive: bool,
) -> std::result::Result<(), CommandError> {
    state
        .backend()
        .await
        .remove(&path, recursive)
        .await
        .map_err(Into::into)
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
#[tauri::command]
async fn enqueue_downloads(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
) -> std::result::Result<Vec<String>, CommandError> {
    let backend = state.backend().await;
    let mut ids = Vec::new();
    for path in paths {
        // The size comes from a stat rather than the listing the window had:
        // a progress bar built on a number that has since changed is worse
        // than one that admits it does not know.
        let size = backend.stat(&path).await.map_or(0, |attrs| attrs.size);
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let id = new_id();
        state
            .transfers
            .add(Transfer::new(
                id.clone(),
                Direction::Down,
                path,
                format!("{}/{name}", destination.trim_end_matches('/')),
                size,
            ))
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Queue an upload of each local path into `destination` on the share.
#[tauri::command]
async fn enqueue_uploads(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
) -> std::result::Result<Vec<String>, CommandError> {
    let mut ids = Vec::new();
    for path in paths {
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
        let dest = if destination.is_empty() {
            name
        } else {
            format!("{}/{name}", destination.trim_end_matches('/'))
        };
        let id = new_id();
        state
            .transfers
            .add(Transfer::new(id.clone(), Direction::Up, path, dest, size))
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Pause one transfer. It keeps its partial, which is its resume point.
#[tauri::command]
async fn pause_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Paused).await;
    Ok(())
}

/// Cancel one transfer. Its partial is not kept.
#[tauri::command]
async fn cancel_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Cancelled).await;
    Ok(())
}

/// Put a paused or failed transfer back in the queue.
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

/// Whether a destination is already taken, and what `Keep both` would call it.
///
/// Answered by the engine rather than the window because the naming rule —
/// the suffix goes before the extension, numbering skips what is taken — is
/// the kind of thing two implementations would disagree about.
#[tauri::command]
async fn check_conflicts(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    upload: bool,
) -> std::result::Result<Vec<Conflict>, CommandError> {
    let backend = state.backend().await;
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<EditSession, CommandError> {
    let backend = state.backend().await;
    let session = state.edits.checkout(&backend, &path).await?;
    open_in_editor(&session.local_path);
    watch_for_saves(app, session.clone());
    Ok(session)
}

/// Write a working copy back.
///
/// `force` is the conflict prompt's "Replace". Without it a moved remote comes
/// back as `Conflict` rather than being overwritten.
#[tauri::command]
async fn save_edit(
    state: tauri::State<'_, AppState>,
    id: String,
    force: bool,
) -> std::result::Result<SaveOutcome, CommandError> {
    let backend = state.backend().await;
    state
        .edits
        .save(&backend, &id, force)
        .await
        .map_err(Into::into)
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
fn watch_for_saves(app: tauri::AppHandle, session: EditSession) {
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
            if app.emit("edit://saved", &session).is_err() {
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
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<(), CommandError> {
    state.sessions.mount(spec).await.map_err(Into::into)
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

/// Switch which mount the window is looking at.
#[tauri::command]
async fn set_active(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state.sessions.set_active(&id).await.map_err(Into::into)
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
pub fn run() {
    // A store that cannot be located is a fatal misconfiguration rather than
    // something to degrade around: without it there is nowhere to record a
    // trust decision, and every SSH connection would ask again forever.
    let known_hosts = KnownHosts::app_default().expect("no data directory for known_hosts");
    let sessions = Arc::new(SessionManager::new(Arc::new(ProtocolConnector {
        known_hosts: known_hosts.clone(),
    })));

    let demo: Arc<dyn ShareBackend> = Arc::new(MockBackend::media_share());
    // Beside the known hosts, for the same reason: it is app state that has to
    // outlive the process, and the engine is what changes it.
    let queue_path = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("excalibur")
        .join("transfers.json");
    let queue = Arc::new(TransferQueue::open(queue_path));
    let worker = Arc::new(Worker::new(
        Arc::clone(&queue),
        Arc::new(ActiveMount {
            sessions: Arc::clone(&sessions),
            demo: Arc::clone(&demo),
        }),
        DEFAULT_CONCURRENCY,
    ));

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        // The Upload picker (E13-S4). The webview's own file input yields
        // `File` objects, and the engine needs paths.
        .plugin(tauri_plugin_dialog::init())
        // Previews and media read through here, never through a command.
        .register_asynchronous_uri_scheme_protocol(STREAM_SCHEME, |context, request, responder| {
            let sessions = Arc::clone(&context.app_handle().state::<AppState>().sessions);
            let demo = Arc::clone(&context.app_handle().state::<AppState>().demo);
            // The path is percent-encoded by the webview; a name with a space
            // or a `#` in it is common enough that decoding is not optional.
            let path = percent_decode(request.uri().path().trim_start_matches('/'));
            let range = request
                .headers()
                .get(http::header::RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            tauri::async_runtime::spawn(async move {
                let backend = sessions.active_backend().await.unwrap_or(demo);
                responder.respond(stream_range(backend, &path, range).await);
            });
        })
        .manage(AppState {
            sessions: Arc::clone(&sessions),
            secrets: Arc::new(KeyringStore),
            known_hosts,
            edits: Arc::new(EditStore::app_default().expect("no cache directory for edits")),
            transfers: Arc::clone(&queue),
            worker: Arc::clone(&worker),
            demo: Arc::clone(&demo),
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
                    let _ = handle.emit("mount://changed", &event);
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

            // Anything restored from the last run is queued, so it starts.
            let resuming = Arc::clone(&worker);
            tauri::async_runtime::spawn(async move { resuming.drain().await });
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
            check_conflicts,
            edit_file,
            save_edit,
            edits,
            close_edit,
            mount,
            unmount,
            remount_read_write,
            set_active
        ])
        .run(tauri::generate_context!())
        .expect("the webview failed to start");
}

#[cfg(test)]
mod tests {
    use super::{parse_range, percent_decode, STREAM_DEFAULT_WINDOW, STREAM_MAX_WINDOW};

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
