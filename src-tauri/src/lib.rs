//! The Tauri shell: commands, DTOs and the event bridge. Nothing else.
//!
//! Two boundaries this file exists to keep:
//!
//! - **File bytes never cross IPC.** Commands carry paths and metadata. A
//!   preview or a transfer streams through the engine and, later, an asset
//!   protocol handler — never through a command's return value.
//! - **The engine knows nothing about Tauri.** Everything here is a thin
//!   translation, so the engine stays testable without a window.
//!
//! What is left in this file is the app itself: [`run`], the builder it
//! assembles, and the `generate_handler!` list that is the whole of the
//! webview's vocabulary. Everything the builder points at lives in a module:
//!
//! - [`commands`] — the `invoke()` surface, one module per subject
//! - [`state`] — [`AppState`](state::AppState) and the worker's view of it
//! - [`connector`] — a `ConnectionSpec` turned into a live backend
//! - [`stream`] — the `excalibur-stream://` URI scheme
//! - [`events`] — every event name the webview listens on
//! - [`dragging`] — native drag and drop, and Finder's file promises
//! - [`menu`] — the macOS menu bar
//! - [`error`] — what a failed command looks like across the bridge

#![forbid(unsafe_code)]

use std::sync::Arc;

use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::Worker;
use excalibur_engine::{
    EditStore, KeyringStore, KnownHosts, MessageLog, MountStatus, SecretStore, SessionManager,
    TransferStatus,
};
use tauri::{Emitter, Manager};

mod commands;
mod connector;
mod dragging;
mod error;
mod events;
#[cfg(target_os = "macos")]
mod menu;
mod state;
mod stream;
mod util;

use connector::ProtocolConnector;
use state::{AppMount, AppState};

/// Transfers moving at once.
///
/// Three is the design's default. More saturates a link without finishing
/// anything sooner, and makes every row's ETA worse at the same time.
const DEFAULT_CONCURRENCY: usize = 3;

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

/// Build and run the desktop app.
///
/// Start the window.
///
/// # Panics
///
/// Panics if the platform has no data directory to keep accepted host keys in,
/// or if the webview cannot be created. Neither is a condition the app can
/// continue from.
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
    // One log for the app, shared with the connector: the mount's life is
    // written by the manager, and an xsync session's frames by a tap the
    // connector installs as it builds the client (`twopane.md` XTP-E5-S1).
    let log = Arc::new(MessageLog::new());
    let sessions = Arc::new(SessionManager::with_log(
        Arc::new(ProtocolConnector {
            known_hosts: known_hosts.clone(),
            secrets: Arc::clone(&secrets),
            log: Arc::clone(&log),
        }),
        log,
    ));

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
        .register_asynchronous_uri_scheme_protocol(stream::STREAM_SCHEME, stream::serve)
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
            // The same stream wakes the transfer queue. A queued row whose
            // share is not connected is left queued rather than failed, so
            // something has to notice when the share turns up — and a mount
            // reaching `Ready` is exactly that moment. Without this the rows
            // restored at launch would wait for the next enqueue or retry,
            // which is to say for the user to do the app's job.
            let waking = Arc::clone(&worker);
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = changes.recv().await {
                    if event.status == MountStatus::Ready {
                        let worker = Arc::clone(&waking);
                        tauri::async_runtime::spawn(async move { worker.drain().await });
                    }
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
                        let _ = handle.emit(events::MOUNT_CHANGED, &event);
                        continue;
                    }
                    for label in &event.audience {
                        let _ = handle.emit_to(label.as_str(), events::MOUNT_CHANGED, &event);
                    }
                }
            });

            // Queue changes reach the window the same way. A lagged receiver
            // drops events silently, which is why `transfers` exists as a
            // command: the window can always ask for the truth.
            // The message log reaches the window the same way, and `log_lines`
            // is the catch-up for whatever a window missed.
            let handle = app.handle().clone();
            let mut lines = sessions.log().subscribe();
            tauri::async_runtime::spawn(async move {
                while let Ok(line) = lines.recv().await {
                    let _ = handle.emit(events::LOG_LINE, &line);
                }
            });

            let handle = app.handle().clone();
            let mut moved = queue.events();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = moved.recv().await {
                    let _ = handle.emit(events::TRANSFER_PROGRESS, &event);
                }
            });

            let handle = app.handle().clone();
            let mut queue_changes = queue.queue_events();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = queue_changes.recv().await {
                    let _ = handle.emit(events::TRANSFER_QUEUES, &event);
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
                dragging::sweep_abandoned_staging(&restored).await;
                resuming.drain().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::mounts::list_dir,
            commands::mounts::mount_info,
            commands::fs::mkdir,
            commands::fs::rename,
            commands::fs::remove,
            commands::mounts::test_connection,
            commands::mounts::reachable,
            commands::secrets::keychain_available,
            commands::mounts::supported_protocols,
            commands::transfers::set_transfer_concurrency,
            commands::secrets::save_secret,
            commands::secrets::forget_secret,
            commands::transfers::configure_http_downloads,
            commands::secrets::save_http_credential,
            commands::secrets::forget_http_credential,
            commands::secrets::trust_host_key,
            commands::secrets::forget_host_key,
            commands::transfers::transfers,
            commands::transfers::download_queues,
            commands::transfers::create_download_queue,
            commands::transfers::update_download_queue,
            commands::transfers::delete_download_queue,
            commands::transfers::move_transfer_to_queue,
            commands::transfers::reorder_download_queue,
            commands::transfers::reorder_download_queues,
            commands::transfers::enqueue_downloads,
            commands::transfers::enqueue_http_download,
            commands::transfers::probe_http_download,
            commands::transfers::download_limits,
            commands::transfers::enqueue_uploads,
            commands::transfers::pause_transfer,
            commands::transfers::cancel_transfer,
            commands::transfers::remove_transfer,
            commands::transfers::open_transfer_folder,
            commands::transfers::retry_transfer,
            commands::transfers::pause_all_transfers,
            commands::transfers::clear_finished_transfers,
            commands::transfers::retry_failed_transfers,
            commands::transfers::clear_failed_transfers,
            commands::transfers::download_folder,
            commands::transfers::download_urls_from_files,
            commands::mounts::user_folders,
            commands::mounts::existing_directories,
            commands::fs::check_conflicts,
            commands::edits::edit_file,
            commands::edits::save_edit,
            commands::edits::edits,
            commands::edits::close_edit,
            commands::mounts::mount,
            commands::mounts::preconnect,
            commands::mounts::unmount,
            commands::mounts::remount_read_write,
            commands::mounts::reconnect,
            commands::log::log_lines,
            commands::log::clear_log,
            commands::mounts::set_active,
            commands::windows::set_window_title,
            commands::windows::new_window,
            commands::windows::begin_drag
        ]);

    // The menu bar exists only on macOS, and only so that ⌘Q is something this
    // app can answer for. Everywhere else Tauri's default menu is what it was.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::menu_with_confirming_quit)
        .on_menu_event(menu::on_menu_event);

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
                menu::hand_window_menu_to_appkit(app);

                if app.webview_windows().is_empty() {
                    let label = commands::windows::next_label(app);
                    let _ = commands::windows::open_window(app, &label, None, "", None);
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
                let label = commands::windows::next_label(app);
                let _ = commands::windows::open_window(app, &label, None, "", None);
            }
            _ => {}
        });
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
