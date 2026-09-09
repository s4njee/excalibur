//! Changing what is on a share: create, rename, remove, and the conflict
//! question the transfer commands ask before any of it.

use excalibur_engine::transfer::conflict::{self, Conflict};

use crate::error::CommandError;
use crate::events::{announce_listing, announce_listing_on};
use crate::state::AppState;

/// Create one directory. Not `mkdir -p`: the parent must exist.
#[tauri::command]
pub(crate) async fn mkdir(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> std::result::Result<(), CommandError> {
    state.backend(&window).await?.mkdir(&path).await?;
    announce_listing(&state.sessions, &window).await;
    Ok(())
}

/// Rename, refusing to clobber unless `replace` says otherwise.
#[tauri::command]
pub(crate) async fn rename(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
    replace: bool,
    mount: Option<String>,
) -> std::result::Result<(), CommandError> {
    // `mount` is given only by a drop from another window onto a share this
    // window is not the one looking at — a sidebar row for a mount someone
    // else has open (E24-S2). A move is a `rename` on one share, and the share
    // is the one the *items* are on, not the one the window happens to show.
    let (id, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    backend.rename(&from, &to, replace).await?;
    announce_listing_on(&state.sessions, &window, &id).await;
    Ok(())
}

/// Remove a file, or a directory and everything under it.
#[tauri::command]
pub(crate) async fn remove(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    recursive: bool,
) -> std::result::Result<(), CommandError> {
    state
        .backend(&window)
        .await?
        .remove(&path, recursive)
        .await?;
    announce_listing(&state.sessions, &window).await;
    Ok(())
}

/// Whether a destination is already taken, and what `Keep both` would call it.
///
/// Answered by the engine rather than the window because the naming rule —
/// the suffix goes before the extension, numbering skips what is taken — is
/// the kind of thing two implementations would disagree about.
///
/// **This asks about the items the user picked, not about what is inside
/// them**, and it always did — which is why E22-S3 could make a folder one row
/// without touching this function. A folder named in `paths` clashes when a
/// folder of that name is already at the destination, and the three answers
/// mean, at that granularity:
///
/// - **Skip** drops the folder from the batch; nothing beneath it is looked at.
/// - **Replace** enqueues it, and the session merges into what is there —
///   same-named files are overwritten, ones the source does not have are left
///   alone. It is not a delete-then-copy, because `delete` is off in
///   `LocalSyncOptions` and stays off: a transfer must not remove files at the
///   destination nobody asked about (E22-S1). A folder-level "Replace" is
///   therefore weaker than a file-level one, and that is the honest reading of
///   what a copy can promise.
/// - **Keep both** offers a free name, and **the window does not yet use it** —
///   `resolveConflicts` re-sends the original paths, so it currently behaves as
///   Replace. That is a pre-existing gap, older than this story and not made
///   worse by it: applying the suggested name needs a per-path destination in
///   `enqueue_downloads`/`enqueue_uploads`, which neither command takes.
#[tauri::command]
pub(crate) async fn check_conflicts(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    upload: bool,
    mount: Option<String>,
) -> std::result::Result<Vec<Conflict>, CommandError> {
    // The remote side of the check, which is the destination for an upload and
    // the source for a download. Named explicitly by a cross-window drop, and
    // this window's for everything else.
    let (_, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let mut conflicts = Vec::new();
    for path in paths {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
        let dest = format!("{}/{name}", destination.trim_end_matches('/'));
        let exists = if upload {
            backend.stat(&dest).await.is_ok()
        } else {
            std::path::Path::new(&dest).exists()
        };
        if !exists {
            continue;
        }
        // `taken` is answered against the same side the file is going to, so
        // the suggested name is one that is actually free there.
        let directory = destination.trim_end_matches('/').to_owned();
        let mut used = Vec::new();
        if upload {
            if let Ok(listing) = backend.list_dir(&directory).await {
                used.extend(listing.entries.into_iter().map(|entry| entry.name));
            }
        } else if let Ok(entries) = std::fs::read_dir(&directory) {
            used.extend(
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned()),
            );
        }
        let keep_both_as = conflict::keep_both_name(&name, &|candidate| {
            used.iter().any(|taken| taken == candidate)
        });
        conflicts.push(Conflict {
            transfer_id: String::new(),
            name,
            dest_path: dest,
            keep_both_as,
        });
    }
    Ok(conflicts)
}
