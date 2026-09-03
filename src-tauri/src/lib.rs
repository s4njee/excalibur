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

use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::{Listing, MountInfo, ShareBackend};

/// The session's backends. M0 holds exactly one, a mock; the session manager
/// that holds several and reconnects them is E2-S2.
struct AppState {
    backend: Arc<dyn ShareBackend>,
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
) -> Result<Listing, CommandError> {
    let backend = Arc::clone(&state.backend);
    backend.list_dir(&path).await.map_err(Into::into)
}

/// The mount facts alone, for a status bar refresh that does not need a
/// listing.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "tauri::command requires State by value"
)]
fn mount_info(state: tauri::State<'_, AppState>) -> MountInfo {
    // `State` is a handle, not a value worth borrowing through; Tauri's macro
    // requires it by value.
    state.backend.mount_info()
}

/// Build and run the desktop app.
///
/// # Panics
///
/// Panics if the webview cannot be created, which is not a condition the app
/// can continue from.
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            backend: Arc::new(MockBackend::media_share()),
        })
        .invoke_handler(tauri::generate_handler![list_dir, mount_info])
        .run(tauri::generate_context!())
        .expect("the webview failed to start");
}
