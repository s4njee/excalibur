//! The OS keychain and the SSH trust store.
//!
//! Everything here writes or forgets. **There is deliberately no command that
//! reads a secret back to the window** — see [`save_secret`].

use std::sync::Arc;

use excalibur_engine::transfer::http::HTTP_CREDENTIAL_ACCOUNT;
use excalibur_engine::{hostkeys, EngineError, Secret};

use crate::error::CommandError;
use crate::state::AppState;

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
pub(crate) async fn blocking<T, F>(work: F) -> T
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
pub(crate) async fn keychain_available(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<bool, ()> {
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
pub(crate) async fn save_secret(
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
pub(crate) async fn forget_secret(
    state: tauri::State<'_, AppState>,
    account: String,
) -> std::result::Result<(), CommandError> {
    let secrets = Arc::clone(&state.secrets);
    blocking(move || secrets.delete(&account))
        .await
        .map_err(Into::into)
}

/// Save the HTTP Basic password in the OS keychain and refresh Rust memory.
#[tauri::command]
pub(crate) async fn save_http_credential(
    state: tauri::State<'_, AppState>,
    username: String,
    secret: String,
) -> std::result::Result<(), CommandError> {
    let secrets = Arc::clone(&state.secrets);
    let value = Secret::new(secret);
    blocking({
        let value = value.clone();
        move || secrets.set(HTTP_CREDENTIAL_ACCOUNT, &value)
    })
    .await
    .map_err(CommandError::from)?;
    let mut runtime = state.worker.http_config().await;
    runtime.settings.username = Some(username);
    runtime.credential = Some(value);
    state.worker.set_http_config(runtime).await;
    Ok(())
}

/// Delete the HTTP Basic password from both the OS keychain and process memory.
#[tauri::command]
pub(crate) async fn forget_http_credential(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), CommandError> {
    let secrets = Arc::clone(&state.secrets);
    blocking(move || secrets.delete(HTTP_CREDENTIAL_ACCOUNT))
        .await
        .map_err(CommandError::from)?;
    let mut runtime = state.worker.http_config().await;
    runtime.credential = None;
    state.worker.set_http_config(runtime).await;
    Ok(())
}

/// Accept a host's SSH key, after the user compared its fingerprint.
///
/// Takes the host and port rather than the fingerprint the window was shown:
/// re-scanning means the key that gets recorded is one the host is offering
/// now, so a stale prompt left open on screen cannot be used to trust a key
/// the host has already stopped presenting.
#[tauri::command]
pub(crate) async fn trust_host_key(
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
pub(crate) async fn forget_host_key(
    state: tauri::State<'_, AppState>,
    host: String,
    port: u16,
) -> std::result::Result<(), CommandError> {
    let known = state.known_hosts.clone();
    blocking(move || known.forget(&host, port))
        .await
        .map_err(Into::into)
}
