//! Connecting shares, and the facts a window needs about them.

use excalibur_engine::backend::ConnectionSpec;
use excalibur_engine::{Listing, MountInfo};

use crate::error::CommandError;
use crate::state::AppState;

/// One directory, with the mount facts that must not drift from it.
#[tauri::command]
pub(crate) async fn list_dir(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    mount: Option<String>,
) -> std::result::Result<Listing, CommandError> {
    // `mount` names which share to list, for a window drawing two panes on two
    // of them (`twopane.md` XTP-E8-S1). Absent is this window's active mount,
    // which is every single-pane listing and was the only case until now — the
    // same rule `rename`, `remove` and `check_conflicts` already follow for a
    // drop carrying another window's selection (E24-S2).
    let (_, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    backend.list_dir(&path).await.map_err(Into::into)
}

/// The mount facts alone, for a status bar refresh that does not need a
/// listing.
#[tauri::command]
pub(crate) async fn mount_info(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<MountInfo, CommandError> {
    Ok(state.backend(&window).await?.mount_info())
}

/// Try a connection and report what it found, without keeping it.
#[tauri::command]
pub(crate) async fn test_connection(
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<MountInfo, CommandError> {
    state.sessions.test(&spec).await.map_err(Into::into)
}

/// Which protocols this build can actually connect.
///
/// Compiled rather than listed, because SMB is behind a cargo feature: one
/// frontend bundle serves a build with it and a build without, so a static
/// list in the window would be wrong for one of them. Letting a user fill in a
/// form that cannot connect is worse than telling them up front.
#[tauri::command]
pub(crate) async fn supported_protocols() -> std::result::Result<Vec<&'static str>, ()> {
    // `local` is unconditional: it is the standard library, so there is no
    // build of this binary that cannot open a folder on the machine it is
    // running on (E21-S2).
    let mut protocols = vec!["xsync", "sftp", "nfs", "local"];
    if cfg!(feature = "smb") {
        protocols.push("smb");
    }
    Ok(protocols)
}

/// Whether a host is answering, for the sidebar's dimmed rows.
///
/// Deliberately weaker than mounting: this runs for every saved bookmark on a
/// timer, and mounting each one would ask servers for credentials nobody typed.
#[tauri::command]
pub(crate) async fn reachable(host: String, port: u16) -> bool {
    excalibur_engine::session::reachable(&host, port, std::time::Duration::from_secs(2)).await
}

/// Connect a share and make it the active one.
#[tauri::command]
pub(crate) async fn mount(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<(), CommandError> {
    state
        .sessions
        .mount(window.label(), spec)
        .await
        .map_err(Into::into)
}

/// Connect a share without putting the calling window on it.
///
/// What the sidebar's local favourites are opened through at startup, so the
/// folders the user keeps are already held before anything asks for them — and,
/// the reason it exists, so a queue restored from the last run has something to
/// wait *for*: an unmounted share is a wait now, and a wait needs an arrival.
///
/// **A failure here is not news.** A favourite whose disk was ejected fails,
/// and it must not raise a banner in a window that is looking at something
/// else — so the error is returned to the caller, which drops it, and the
/// session event carries no audience because no window is on this mount.
#[tauri::command]
pub(crate) async fn preconnect(
    state: tauri::State<'_, AppState>,
    spec: ConnectionSpec,
) -> std::result::Result<(), CommandError> {
    state.sessions.preconnect(spec).await.map_err(Into::into)
}

/// Disconnect a share.
#[tauri::command]
pub(crate) async fn unmount(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state.sessions.unmount(&id).await;
    Ok(())
}

/// Reconnect a mount asking for read-write.
///
/// Returns the mount facts that came back, **including when the server refuses
/// the upgrade**. A share that is still read-only is an answer, not an error:
/// reporting it as a failure is how the recovery button becomes a loop the user
/// presses forever. The window reads `writable` off the result and shows the
/// server's reason if it is still false.
#[tauri::command]
pub(crate) async fn remount_read_write(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<MountInfo, CommandError> {
    state
        .sessions
        .remount_read_write(&id)
        .await
        .map_err(Into::into)
}

/// Reconnect a mount as it was mounted.
///
/// The toolbar's `Reconnect` and the connection banner's. Not
/// `remount_read_write`: that asks for read-write whatever the share was
/// mounted as, which would quietly hand write access to a share somebody chose
/// to open read-only. Like it, a server that answers read-only is an answer
/// rather than an error, and the window re-reads `writable` from the result.
#[tauri::command]
pub(crate) async fn reconnect(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<MountInfo, CommandError> {
    state.sessions.reconnect(&id).await.map_err(Into::into)
}

/// Switch which mount the calling window is looking at.
///
/// Also how a second window *joins* a share the first one already has: the
/// session is not connected again, both windows simply name it.
#[tauri::command]
pub(crate) async fn set_active(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), CommandError> {
    state
        .sessions
        .set_active(window.label(), &id)
        .await
        .map_err(Into::into)
}

/// The folders the sidebar's FAVORITES section is seeded from.
///
/// Asked of the OS rather than assembled in the window, because every one of
/// them is a fact the platform owns and the webview cannot see: the home
/// directory has no browser API, and the media folders are neither named nor
/// placed the same way twice — macOS keeps films in `~/Movies` and Linux in
/// whatever `XDG_VIDEOS_DIR` says, which on a localised desktop is not an
/// English word at all. Guessing `~/Videos` everywhere would seed a row for a
/// folder that is not there.
///
/// An empty string means *this platform has no such folder*, which the window
/// drops. It is not an error: Linux has no Applications directory a person
/// browses, and `dirs` answers `None` for a media folder the user never made.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserFolders {
    pub(crate) home: String,
    pub(crate) applications: String,
    pub(crate) desktop: String,
    pub(crate) documents: String,
    pub(crate) pictures: String,
    /// `~/Movies` on macOS, `XDG_VIDEOS_DIR` on Linux. The window labels it by
    /// its own last path segment rather than calling it "Videos" everywhere,
    /// so the row reads as the folder the user already has.
    pub(crate) videos: String,
}

/// Where the sidebar's default favourites live on this machine.
#[tauri::command]
pub(crate) fn user_folders() -> UserFolders {
    fn path(folder: Option<std::path::PathBuf>) -> String {
        folder
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    UserFolders {
        home: path(dirs::home_dir()),
        // `dirs` has no applications folder, because most platforms have no
        // such thing. macOS does, at a fixed absolute path; everywhere else
        // this is deliberately empty rather than pointed at
        // `/usr/share/applications`, which holds `.desktop` files and is not a
        // folder anyone means by "Applications".
        applications: if cfg!(target_os = "macos") {
            "/Applications".to_owned()
        } else {
            String::new()
        },
        desktop: path(dirs::desktop_dir()),
        documents: path(dirs::document_dir()),
        pictures: path(dirs::picture_dir()),
        videos: path(dirs::video_dir()),
    }
}

/// Which of `paths` are directories that exist right now.
///
/// The sidebar hides a favourite whose folder is gone — an external disk
/// ejected, a project deleted — because the only thing clicking it could do is
/// fail. Answered in a batch rather than one call per row: the list is checked
/// on every load, and a round trip per favourite is a round trip per favourite.
///
/// Symlinks are followed (`metadata`, not `symlink_metadata`): a favourite
/// pointing at a link to a real directory opens that directory, so it is a
/// directory as far as this question goes.
#[tauri::command]
pub(crate) fn existing_directories(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .collect()
}
