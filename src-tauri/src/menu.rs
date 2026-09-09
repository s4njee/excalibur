//! macOS's menu bar.
//!
//! The whole file is `#[cfg(target_os = "macos")]`: everywhere else Tauri's
//! default menu is what it was, and this app has nothing to add to it.
#![cfg(target_os = "macos")]

use tauri::{Emitter, Manager};

use crate::events::MENU_TRANSFERS;

/// The id of the Quit item this app builds for itself.
const QUIT_MENU_ITEM: &str = "quit";

/// The id of the View menu's Transfers item.
const TRANSFERS_MENU_ITEM: &str = "transfers";

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
pub(crate) fn menu_with_confirming_quit(
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

    // Safe as a menu item where `New Window` was not: this only toggles a
    // panel, and the focused window is the whole of what it needs to know.
    // The accelerator matches `state/shortcuts.ts`'s `transfers`, and on macOS
    // it is the menu that answers it — the webview never sees ⇧⌘T — so the two
    // cannot drift into meaning different things.
    let transfers = MenuItem::with_id(
        app,
        TRANSFERS_MENU_ITEM,
        "Transfers",
        true,
        Some("Cmd+Shift+T"),
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
                &[
                    &transfers,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::fullscreen(app, None)?,
                ],
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
pub(crate) fn hand_window_menu_to_appkit(app: &tauri::AppHandle) {
    use tauri::menu::MenuItemKind;

    let Some(menu) = app.menu() else { return };
    if let Some(MenuItemKind::Submenu(windows)) = menu.get(WINDOW_MENU_ITEM) {
        let _ = windows.set_as_windows_menu_for_nsapp();
    }
}

/// Answer a click on one of the two items this app owns.
// The event is owned because `Builder::on_menu_event` hands it over that way.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn on_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    if event.id() == QUIT_MENU_ITEM {
        // Through `exit` rather than terminating: that is what raises
        // `ExitRequested`, which is where the confirmation lives.
        app.exit(0);
    } else if event.id() == TRANSFERS_MENU_ITEM {
        // To the focused window rather than broadcast: the panel is
        // per-window state, and a menu command belongs to the window
        // the user is looking at, not to every window at once.
        if let Some((label, _)) = app
            .webview_windows()
            .iter()
            .find(|(_, window)| window.is_focused().unwrap_or(false))
        {
            let _ = app.emit_to(label.as_str(), MENU_TRANSFERS, ());
        }
    }
}
