//! The message log, as the window reads it (`twopane.md` XTP-E5-S1).
//!
//! Two commands and an event, the same shape the transfer queue has: the
//! stream carries what happens next, and the ring is how a window opened later
//! catches up. A window that could only see lines written after it opened
//! would be missing exactly the ones explaining why it was opened.

use excalibur_engine::LogLine;

use crate::error::CommandError;
use crate::state::AppState;

/// Everything the log holds, oldest first.
#[tauri::command]
pub(crate) async fn log_lines(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<LogLine>, CommandError> {
    Ok(state.sessions.log().lines())
}

/// Empty the log. The stream is untouched; new lines still arrive.
#[tauri::command]
pub(crate) async fn clear_log(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), CommandError> {
    state.sessions.log().clear();
    Ok(())
}
