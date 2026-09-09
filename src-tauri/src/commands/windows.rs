//! Windows: naming them, opening them, and dragging out of them.

use excalibur_engine::drag::{self, Selection};
use excalibur_engine::EngineError;
use tauri::Manager;

use crate::dragging::{drag_failed, open_drag, promise_sink};
use crate::error::CommandError;
use crate::state::AppState;

/// Name the calling window, for the platform rather than for the app.
///
/// The title the design draws is painted by the webview — it is centred over
/// the toolbar, and the native title is hidden to make room — so without this
/// the window's real title stays whatever it was built with. Every window was
/// then called "Excalibur", and the macOS Window menu, Mission Control and the
/// app switcher list windows by exactly that: two windows, two identical rows,
/// no way to tell which was which.
///
/// A command of this app's own rather than `core:window:allow-set-title` in the
/// capability. Granting the core window API is granting it to every page the
/// webview ever loads, to save writing these four lines.
#[tauri::command]
pub(crate) async fn set_window_title(
    window: tauri::Window,
    title: String,
) -> std::result::Result<(), CommandError> {
    window
        .set_title(&title)
        .map_err(|error| CommandError::banner(error.to_string()))
}

/// Open a second browser window on this window's mount and folder (⌘N).
///
/// Built in Rust rather than with `WebviewWindow` from the frontend, so that
/// the label, the size, the macOS title-bar treatment and the capability that
/// grants the new window its commands are all decided in one place. A window
/// the webview created would be a window nobody had granted anything to, and
/// the first `list_dir` from it would fail on a permission error rather than on
/// anything a user could act on.
///
/// The new window's mount is set here, **before** the webview loads, so its
/// first listing has a share to come from rather than a race with a `set_active`
/// round trip. The mount id travels in the URL as well, because the window has
/// to know which of its bookmarks to draw as mounted — and the store cannot
/// tell it, since a `mounted` flag read off disk is from a previous run
/// (E1-S6).
#[tauri::command]
pub(crate) async fn new_window(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<String, CommandError> {
    let app = window.app_handle().clone();
    let label = next_label(&app);
    let mount = state.sessions.active(window.label()).await;
    if let Some(id) = &mount {
        // Joining, not connecting: the share is already up, and a second login
        // for it would be visible to the server and would ask the keychain
        // again for something it already answered.
        state
            .sessions
            .set_active(&label, id)
            .await
            .map_err(CommandError::from)?;
    }
    // Cascaded off the window that opened it. Stacked exactly on top, a new
    // window looks like the old one changed rather than like a second one
    // appeared.
    let at = match (window.outer_position(), window.scale_factor()) {
        (Ok(position), Ok(scale)) => {
            let logical = position.to_logical::<f64>(scale);
            Some((logical.x + 24.0, logical.y + 24.0))
        }
        _ => None,
    };
    open_window(&app, &label, mount.as_deref(), &path, at)
        .map_err(|error| CommandError::banner(format!("could not open a window: {error}")))?;
    Ok(label)
}

/// Start a native drag of this window's selection (E24-S2, E24-S1).
///
/// `image` is the drag image the webview rendered, as PNG **bytes**. Never a
/// path: an item on a share has no local file to point at, and writing one out
/// to name it would be a fetch nobody asked for.
///
/// `folders` is the subset of `paths` the list drew as directories. It is the
/// one thing the engine cannot work out from a path — a folder called `Dune`
/// has no extension to classify — and getting it wrong promises a folder as a
/// file, which Finder accepts and then writes nothing into.
///
/// What crosses to `AppKit` is one pasteboard type, `dev.excalibur.selection`,
/// carrying the mount, the paths and this window's label, and beside it one
/// file promise per item. The promises are what let the same drag end in
/// Finder; nothing is fetched for them until Finder asks.
#[tauri::command]
pub(crate) async fn begin_drag(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    folders: Vec<String>,
    image: Vec<u8>,
) -> std::result::Result<(), CommandError> {
    if paths.is_empty() {
        return Ok(());
    }
    let Some(mount) = state.sessions.active(window.label()).await else {
        return Err(CommandError::from(EngineError::Disconnected(
            "no share is mounted".to_owned(),
        )));
    };
    let selection = Selection {
        mount,
        paths,
        window: window.label().to_owned(),
    };
    let promises = drag::promises_for(&selection, &folders);
    let fulfil = promise_sink(&state);
    let app = window.app_handle().clone();
    let source = window.label().to_owned();

    // AppKit's dragging protocols are main-thread-only, and a Tauri command
    // runs on the async runtime. The hop is awaited rather than fired and
    // forgotten so a drag that could not start says so on the row the user just
    // grabbed, instead of looking like a gesture that was not recognised.
    let (done, wait) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = done.send(open_drag(
            &handle, &source, &selection, &promises, &image, &fulfil,
        ));
    })
    .map_err(|error| drag_failed(&format!("could not reach the main thread: {error}")))?;
    wait.await
        .map_err(|_| drag_failed("the main thread dropped the drag"))?
        .map_err(CommandError::from)
}

/// Build one browser window, on a mount and a folder, optionally at a position.
///
/// Shared by ⌘N and by the Dock reopening an app that has no windows left, so
/// the two cannot drift into different sizes or different chrome.
pub(crate) fn open_window(
    app: &tauri::AppHandle,
    label: &str,
    mount: Option<&str>,
    path: &str,
    at: Option<(f64, f64)>,
) -> tauri::Result<()> {
    let url = format!(
        "index.html?label={}&path={}{}",
        encode(label),
        encode(path),
        mount.map_or_else(String::new, |id| format!("&mount={}", encode(id))),
    );
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App(url.into()))
            .title("Excalibur")
            .inner_size(1060.0, 640.0)
            .min_inner_size(820.0, 480.0);
    if let Some((x, y)) = at {
        builder = builder.position(x, y);
    }
    // Every window's chrome, including the first: `tauri.conf.json` declares no
    // window, so there is no second copy of this to drift from.
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    builder.build().map(|_| ())
}

/// A label no window currently has.
///
/// Checked against the windows that exist rather than trusted from a counter
/// alone: a label collision silently re-uses the *existing* window, so a ⌘N
/// after a close-and-reopen would look like the shortcut had stopped working.
pub(crate) fn next_label(app: &tauri::AppHandle) -> String {
    let open = app.webview_windows();
    // `main` first, and not only for tidiness: `capabilities/default.json`
    // grants this app's commands to `main` and to `browser-*`, and the first
    // window is the one every other name is numbered from.
    if !open.contains_key("main") {
        return "main".to_owned();
    }
    // Bounded rather than an open range: there is no number of windows worth
    // searching forever for, and a machine that has run out of labels has run
    // out of screen a long way back.
    (2..=MAX_WINDOWS)
        .map(|index| format!("browser-{index}"))
        .find(|candidate| !open.contains_key(candidate))
        .unwrap_or_else(|| "browser".to_owned())
}

/// How many browser windows can be open before ⌘N stops finding a name.
///
/// Not a policy about how many windows are useful — it is the point past which
/// searching for a free label is less sane than refusing.
const MAX_WINDOWS: usize = 64;

/// Percent-encode a value going into the new window's query string.
///
/// The counterpart of `percent_decode`, and hand-rolled for the same reason:
/// a mount id is a bookmark's id — `smb:nas.local:media` — and a path is
/// whatever the share calls its folders. Both reach `URLSearchParams` on the
/// other side, which decodes them.
fn encode(raw: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                // `write!` rather than `push_str(&format!(…))`: one allocation
                // for the whole string instead of one per escaped byte.
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;
    use crate::stream::percent_decode;

    #[test]
    fn a_new_window_url_survives_a_bookmark_id_and_a_path() {
        // A mount id is a bookmark's id, which carries colons; a path carries
        // slashes and spaces. Both go into a query string and come back out of
        // `URLSearchParams` on the other side, so neither may be left raw —
        // an unencoded `&` in a filename would split the query in two.
        assert_eq!(encode("smb:nas.local:media"), "smb%3Anas.local%3Amedia");
        assert_eq!(
            encode("Films/Blade Runner & Co"),
            "Films%2FBlade%20Runner%20%26%20Co"
        );
        assert_eq!(encode("browser-2"), "browser-2");
        // Round-trips through this app's own decoder, which is what the
        // stream protocol uses on the way back in.
        assert_eq!(percent_decode(&encode("a b/c#d.mkv")), "a b/c#d.mkv");
    }
}
