//! Checking a file out to the OS editor, and writing it back.

use excalibur_engine::{EditSession, SaveOutcome};
use tauri::{Emitter, Manager};

use crate::error::CommandError;
use crate::events::{announce_listing, EDIT_SAVED};
use crate::state::AppState;
use crate::util::open_in_editor;

/// Check a file out, open it in the OS editor, and watch it for saves.
///
/// The download and the watch both happen here rather than in the window,
/// because a save can arrive at any time and the window may not be looking.
#[tauri::command]
pub(crate) async fn edit_file(
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
pub(crate) async fn save_edit(
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
pub(crate) async fn edits(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<EditSession>, ()> {
    Ok(state.edits.all().await)
}

/// Finish with a file and remove its working copy.
#[tauri::command]
pub(crate) async fn close_edit(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.edits.close(&id).await;
    Ok(())
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
            if app.emit_to(window.as_str(), EDIT_SAVED, &session).is_err() {
                return;
            }
        }
    });
}

/// Filesystem events closer together than this are one save.
const SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
