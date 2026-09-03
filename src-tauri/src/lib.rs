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
use excalibur_engine::{
    hostkeys, Connector, EngineError, KeyringStore, KnownHosts, Listing, MountInfo, Result, Secret,
    SecretStore, SessionManager, Trust,
};
use tauri::Emitter;

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

/// What the shell holds: the mounts, and a demo share when there are none.
struct AppState {
    sessions: Arc<SessionManager>,
    /// The OS credential store. Written to from the window, never read back to
    /// it: see the commands below.
    secrets: Arc<dyn SecretStore>,
    /// Accepted SSH host keys. Shared with the connector, which is what
    /// enforces them.
    known_hosts: KnownHosts,
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

/// Switch which mount the window is looking at.
#[tauri::command]
async fn set_active(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state.sessions.set_active(&id).await.map_err(Into::into)
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

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState {
            sessions: Arc::clone(&sessions),
            secrets: Arc::new(KeyringStore),
            known_hosts,
            demo: Arc::new(MockBackend::media_share()),
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            mount_info,
            test_connection,
            reachable,
            keychain_available,
            save_secret,
            forget_secret,
            trust_host_key,
            forget_host_key,
            mount,
            unmount,
            set_active
        ])
        .run(tauri::generate_context!())
        .expect("the webview failed to start");
}
