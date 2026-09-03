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
use excalibur_engine::backend::xsync::XsyncBackend;
use excalibur_engine::backend::{ConnectionSpec, Protocol, ShareBackend};
use excalibur_engine::{
    Connector, KeyringStore, Listing, MountInfo, Result, Secret, SecretStore, SessionManager,
};
use tauri::Emitter;

/// Builds the backend a `ConnectionSpec` names.
///
/// This is where the protocol enum turns into a client. `connect()` is
/// deliberately not on `ShareBackend`: each protocol needs different things to
/// get started — a keychain entry, a host key, a port — and the shape of that
/// should come from the second real backend rather than be guessed before the
/// first.
struct ProtocolConnector;

#[async_trait]
impl Connector for ProtocolConnector {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        match spec.protocol {
            Protocol::Xsync => {
                let backend = XsyncBackend::connect_ssh(&spec.host, &spec.share, None).await?;
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
}

impl From<excalibur_engine::EngineError> for CommandError {
    fn from(error: excalibur_engine::EngineError) -> Self {
        Self {
            message: error.to_string(),
            surface: error.surface(),
            transient: error.transient(),
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
/// Panics if the webview cannot be created, which is not a condition the app
/// can continue from.
pub fn run() {
    let sessions = Arc::new(SessionManager::new(Arc::new(ProtocolConnector)));

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState {
            sessions: Arc::clone(&sessions),
            secrets: Arc::new(KeyringStore),
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
            mount,
            unmount,
            set_active
        ])
        .run(tauri::generate_context!())
        .expect("the webview failed to start");
}
