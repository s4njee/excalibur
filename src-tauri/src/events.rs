//! The event names the webview listens on, and the helpers that fire them.
//!
//! Every one of these strings is part of the app's wire contract with the
//! frontend: `state/events.ts` subscribes to them by name, so a rename here is
//! a silent breakage rather than a compile error. They are constants for that
//! reason, and nothing else in this crate spells them out by hand.

use excalibur_engine::SessionManager;
use tauri::{Emitter, Manager};

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
pub(crate) async fn announce_listing(sessions: &SessionManager, window: &tauri::Window) {
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
pub(crate) async fn announce_listing_on(
    sessions: &SessionManager,
    window: &tauri::Window,
    id: &str,
) {
    for label in sessions.windows_on(id).await {
        let _ = window
            .app_handle()
            .emit_to(label.as_str(), LISTING_CHANGED, id);
    }
}

/// Fired at every window on a mount whose contents this app just changed.
pub(crate) const LISTING_CHANGED: &str = "listing://changed";

/// Fired at every window on a mount whose status changed, or broadcast when
/// no window is on it yet.
pub(crate) const MOUNT_CHANGED: &str = "mount://changed";

/// One row of the transfer queue moved, finished or failed.
pub(crate) const TRANSFER_PROGRESS: &str = "transfer://progress";

/// A named download queue was created, renamed, reordered or removed.
pub(crate) const TRANSFER_QUEUES: &str = "transfer://queues";

/// Fired at the window that opened a file when its working copy is written.
pub(crate) const EDIT_SAVED: &str = "edit://saved";

/// Told to the focused window when the Transfers menu item is chosen.
#[cfg(target_os = "macos")]
pub(crate) const MENU_TRANSFERS: &str = "menu://transfers";

/// A drag is over a window, was dropped on it, or has left it.
///
/// Addressed to that window's webview and never broadcast: two windows would
/// otherwise both highlight a row for a pointer that is over one of them
/// (E23-S1's rule, applied to a third event stream).
#[cfg(target_os = "macos")]
pub(crate) const DRAG_OVER: &str = "drag://over";
/// See [`DRAG_OVER`].
#[cfg(target_os = "macos")]
pub(crate) const DRAG_DROP: &str = "drag://drop";
/// See [`DRAG_OVER`].
#[cfg(target_os = "macos")]
pub(crate) const DRAG_LEAVE: &str = "drag://leave";
