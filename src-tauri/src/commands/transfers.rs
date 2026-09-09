//! Everything moving, waiting to move, or being told where to move next.

use std::path::Path;
use std::sync::Arc;

use excalibur_engine::backend::EntryKind;
use excalibur_engine::transfer::http::{
    self as download_http, HttpClientSettings, HttpDownloadLimits, HttpDownloadOptions, HttpProbe,
    HttpRuntimeConfig, HTTP_CREDENTIAL_ACCOUNT,
};
use excalibur_engine::transfer::plan;
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::{Direction, EngineError, NamedQueue, Transfer, TransferStatus};

use crate::commands::secrets::blocking;
use crate::dragging::retire;
use crate::error::CommandError;
use crate::state::AppState;
use crate::util::{new_id, open_in_editor};

/// How many transfers may run at once (E17-S1).
///
/// The preference lives in the window, the semaphore lives in the engine, so
/// the value has to travel. Raising takes effect at once; lowering takes
/// effect as running transfers finish, because recalling a permit already held
/// would mean stopping work the user did not ask to stop.
#[tauri::command]
pub(crate) async fn set_transfer_concurrency(
    limit: usize,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.worker.set_concurrency(limit);
    Ok(())
}

/// Apply nonsecret HTTP settings and refresh the in-memory keychain value.
/// There is intentionally no command that returns that value to TypeScript.
#[tauri::command]
pub(crate) async fn configure_http_downloads(
    state: tauri::State<'_, AppState>,
    settings: HttpClientSettings,
) -> std::result::Result<(), CommandError> {
    if settings.headers.iter().any(|header| {
        matches!(
            header.name.to_ascii_lowercase().as_str(),
            "authorization" | "cookie" | "proxy-authorization" | "range"
        )
    }) {
        return Err(CommandError::from(EngineError::Unsupported(
            "credential and Range headers must use the keychain or downloader".to_owned(),
        )));
    }
    if let download_http::HttpProxy::Explicit { url, .. } = &settings.proxy {
        if url.contains('@') {
            return Err(CommandError::from(EngineError::Unsupported(
                "proxy credentials must not be placed in preferences".to_owned(),
            )));
        }
    }
    let secrets = Arc::clone(&state.secrets);
    let credential = blocking(move || secrets.get(HTTP_CREDENTIAL_ACCOUNT))
        .await
        .map_err(CommandError::from)?;
    state
        .worker
        .set_http_config(HttpRuntimeConfig {
            settings,
            credential,
        })
        .await;
    Ok(())
}

/// Everything in the queue, oldest first.
///
/// The window reads this on open and after any lag on the event stream. A
/// broadcast channel drops messages for a slow consumer without telling it, so
/// there has to be a way back to the truth — the sibling engine has none, and
/// its queue view can desync permanently.
#[tauri::command]
pub(crate) async fn transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<Transfer>, ()> {
    Ok(state.transfers.all().await)
}

/// The named download queues and their execution policy.
#[tauri::command]
pub(crate) async fn download_queues(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<NamedQueue>, ()> {
    Ok(state.transfers.queues().await)
}

/// Create a named queue. New rows continue to use the built-in queue until a
/// caller explicitly moves them, preserving the old enqueue commands.
#[tauri::command]
pub(crate) async fn create_download_queue(
    state: tauri::State<'_, AppState>,
    name: String,
    color: String,
    concurrency: usize,
) -> std::result::Result<NamedQueue, String> {
    state
        .transfers
        .create_queue(name, color, concurrency)
        .await
        .map_err(|error| error.to_string())
}

/// Update a queue's label, colour, or per-queue row limit.
#[tauri::command]
pub(crate) async fn update_download_queue(
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
    color: String,
    concurrency: usize,
) -> std::result::Result<NamedQueue, String> {
    state
        .transfers
        .update_queue(&id, name, color, concurrency)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("download queue {id:?} does not exist"))
}

/// Delete a queue after moving its rows into Default.
#[tauri::command]
pub(crate) async fn delete_download_queue(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), String> {
    state
        .transfers
        .delete_queue(&id)
        .await
        .map(|_| ())
        .ok_or_else(|| format!("download queue {id:?} cannot be deleted"))
}

/// Move one transfer to a queue, appending it at that queue's tail.
#[tauri::command]
pub(crate) async fn move_transfer_to_queue(
    state: tauri::State<'_, AppState>,
    id: String,
    queue_id: String,
) -> std::result::Result<Transfer, String> {
    state
        .transfers
        .move_to_queue(&id, &queue_id)
        .await
        .ok_or_else(|| format!("transfer {id:?} or queue {queue_id:?} does not exist"))
}

/// Apply a drag-reordered list of row ids within one named queue.
#[tauri::command]
pub(crate) async fn reorder_download_queue(
    state: tauri::State<'_, AppState>,
    queue_id: String,
    ids: Vec<String>,
) -> std::result::Result<(), String> {
    if state.transfers.reorder_queue(&queue_id, &ids).await {
        Ok(())
    } else {
        Err(format!("download queue {queue_id:?} does not exist"))
    }
}

/// Apply the visible order of the named queues.
#[tauri::command]
pub(crate) async fn reorder_download_queues(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> std::result::Result<(), String> {
    if state.transfers.reorder_queues(&ids).await {
        Ok(())
    } else {
        Err("could not reorder download queues".to_owned())
    }
}

/// Queue a download of each path, and start moving.
///
/// **A folder is one row**, at `<destination>/<folder>`, exactly as
/// [`enqueue_uploads`] makes it one row — the two commands are the same shape
/// again for the first time since E22-S2.
///
/// It used to be one row per file, walked here by
/// [`plan::plan_downloads`](excalibur_engine::transfer::plan::plan_downloads)
/// before any row existed, on the argument (E13-S4) that a single row for a
/// directory can only report a percentage that means nothing once one file in
/// it fails. E22-S3 withdrew that argument on a measurement rather than a
/// preference. Every row is run by `Worker::attempt`, and every attempt is a
/// whole `sync_endpoints` session — a scoped thread pool, a cancel watcher and
/// a journal, set up and torn down to move one file. On this machine a folder
/// of 1002 files took **21.7 s** that way and **0.31 s** as one session, and
/// only 0.30 s of the 21.7 was spent talking to the share at all; 2000 small
/// files were 38.3 s against 1.1 s. The per-file row was 70× of pure
/// scheduling.
///
/// What the walk here used to buy, and where each part went:
///
/// - **the sizes on the rows** are now the planner's `Planned` event, which is
///   the same scan the transfer was going to do anyway rather than a second
///   one over the browse session;
/// - **an empty remote directory** is created by the sink as it walks, not by
///   this command creating `plan.directories` first;
/// - **a subdirectory that could not be listed** is a per-entry failure inside
///   the session, and [`endpoint::run`](excalibur_engine::transfer::endpoint)
///   refuses to report success for a run with any: the row fails with the
///   count and the first reason. It is one failed row for the folder instead of
///   one failed row beside the ones that worked, which is a real loss of
///   detail and the only thing this change gives up.
///
/// [`plan::plan_downloads`] itself is still there — the CLI's `get` uses it,
/// and it is still the walk that says what a selection *would* copy.
#[tauri::command]
pub(crate) async fn enqueue_downloads(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    mount: Option<String>,
    // `Keep both`'s answer: what to call a source whose name is already taken
    // at the destination. Keyed by source path, because the prompt answers
    // about one clashing name at a time and the rest keep their own.
    rename: Option<std::collections::HashMap<String, String>>,
) -> std::result::Result<Vec<String>, CommandError> {
    // `mount` names the share the *paths* are on. Absent for Download, where it
    // is this window's; present for a drop carrying another window's selection
    // (E24-S2), where the answer is in the payload and not in who is asking.
    let (mount, backend) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let root = std::path::PathBuf::from(destination.trim_end_matches('/'));
    let mut ids = Vec::new();
    // The one directory this command still creates. The session creates
    // everything under it, but it opens its sink *at* this folder, and a
    // download into a folder that is not there should say so on a row rather
    // than as a per-entry failure halfway down a tree.
    if let Err(error) = std::fs::create_dir_all(&root) {
        ids.push(
            report_problem(
                &state.transfers,
                String::new(),
                root.to_string_lossy().into_owned(),
                format!("create {}: {error}", root.display()),
            )
            .await,
        );
        return Ok(ids);
    }
    for path in paths {
        // Names from the server are not paths (E13-S4's other rule, and the
        // one worth keeping whatever the row shape is). A listing that answers
        // `..` is describing a write outside the folder the user chose.
        // `Keep both` renames on the destination's suggestion, and it is still
        // sanitised: a name from the far side is no more a path than a name
        // from a listing is.
        let name = rename
            .as_ref()
            .and_then(|map| map.get(&path))
            .and_then(|chosen| plan::local_name(chosen))
            .map(ToOwned::to_owned)
            .or_else(|| plan::local_name(&path).map(ToOwned::to_owned));
        let Some(name) = name else {
            ids.push(
                report_problem(
                    &state.transfers,
                    path.clone(),
                    root.to_string_lossy().into_owned(),
                    format!("{path:?} is not a name that can be written locally"),
                )
                .await,
            );
            continue;
        };
        // A directory has no size worth showing: its own `stat` size is an
        // inode's, and the planner's `Planned` event fills the real one in a
        // moment later. A stat that fails is not a reason to drop the item —
        // the session will report what is actually wrong with it, on the row
        // the user is looking at.
        let size = match backend.stat(&path).await {
            Ok(attrs) if attrs.kind != EntryKind::Dir => attrs.size,
            _ => 0,
        };
        let id = new_id();
        state
            .transfers
            .add(
                Transfer::new(
                    id.clone(),
                    Direction::Down,
                    path,
                    root.join(name).to_string_lossy().into_owned(),
                    size,
                )
                .on_mount(&mount),
            )
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Queue an HTTP resource without requiring a mounted share (XDL-E1).
#[tauri::command]
pub(crate) async fn enqueue_http_download(
    state: tauri::State<'_, AppState>,
    url: String,
    destination: String,
    options: Option<HttpDownloadOptions>,
) -> std::result::Result<String, CommandError> {
    let id = new_id();
    let transfer =
        Transfer::http_with_options(id.clone(), url, destination, options.unwrap_or_default())?;
    state.transfers.add(transfer).await;
    run_queue(&state);
    Ok(id)
}

/// Network inspection stays on a blocking worker, never the webview thread.
#[tauri::command]
pub(crate) async fn probe_http_download(
    url: String,
) -> std::result::Result<HttpProbe, CommandError> {
    tokio::task::spawn_blocking(move || download_http::probe_download(&url))
        .await
        .map_err(|error| EngineError::Io(format!("the HTTP probe task failed: {error}")))?
        .map_err(CommandError::from)
}

#[tauri::command]
pub(crate) fn download_limits() -> HttpDownloadLimits {
    download_http::download_limits()
}

/// Put something the walk could not do in the queue, already failed.
///
/// A row rather than an error from the command: one unreadable subdirectory
/// must not cancel the enqueue of everything beside it, and the transfer sheet
/// is where a person looks to find out what a copy actually did.
async fn report_problem(
    queue: &TransferQueue,
    source: String,
    destination: String,
    reason: String,
) -> String {
    let id = new_id();
    let mut transfer = Transfer::new(id.clone(), Direction::Down, source, destination, 0);
    transfer.status = TransferStatus::Failed;
    transfer.error = Some(reason);
    queue.add(transfer).await;
    id
}

/// Queue an upload of each local path into `destination` on the share.
///
/// `mount` names the share `destination` is on: absent for Upload and for a
/// Finder drop, both of which mean this window's, and present for a drop from
/// another window (E24-S2). It is recorded on every row rather than resolved
/// when the worker gets there, because between the enqueue and the start the
/// user can put a different share in front of themselves.
///
/// A **folder is one row**, and there is deliberately no walk here to match
/// [`enqueue_downloads`]'s. Since E22-S2 the transfer runs xsync's planner,
/// which does the walk itself — and doing it twice would stat every file over
/// the browse session before the transfer had sent a byte, then have the
/// planner stat them all again. Before E22-S2 there was no walk on this route
/// at all, which is why a dropped folder failed with `EISDIR`.
#[tauri::command]
pub(crate) async fn enqueue_uploads(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    destination: String,
    mount: Option<String>,
    // `Keep both`'s answer, keyed by source path. See `enqueue_downloads`.
    rename: Option<std::collections::HashMap<String, String>>,
) -> std::result::Result<Vec<String>, CommandError> {
    let (mount, _) = state.mount_and_backend(&window, mount.as_deref()).await?;
    let mut ids = Vec::new();
    for path in paths {
        // A directory's own `len()` is its entry size — 96 bytes on APFS —
        // which as a progress total is worse than admitting we do not know
        // yet. The planner's `Planned` event fills it in a moment later.
        let size =
            std::fs::metadata(&path).map_or(0, |meta| if meta.is_dir() { 0 } else { meta.len() });
        // The chosen name is taken apart the same way, so a `Keep both` answer
        // cannot smuggle a separator and write outside the chosen folder.
        let name = rename.as_ref().and_then(|map| map.get(&path)).map_or_else(
            || path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned(),
            |chosen| {
                chosen
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(chosen)
                    .to_owned()
            },
        );
        let dest = if destination.is_empty() {
            name
        } else {
            format!("{}/{name}", destination.trim_end_matches('/'))
        };
        let id = new_id();
        state
            .transfers
            .add(Transfer::new(id.clone(), Direction::Up, path, dest, size).on_mount(&mount))
            .await;
        ids.push(id);
    }
    run_queue(&state);
    Ok(ids)
}

/// Pause one transfer. It keeps its partial, which is its resume point.
///
/// Since E22-S3 "one transfer" can be a whole folder, and the granularity is
/// the row: pausing a folder stops the session, which drains what is in flight
/// and leaves the files it already published where they are.
#[tauri::command]
pub(crate) async fn pause_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Paused).await;
    Ok(())
}

/// Cancel one transfer. Its partial is not kept.
///
/// "Its partial" is the file that was mid-flight: the engine never publishes a
/// staged file it did not finish and verify. For a **folder** row (E22-S3) the
/// files that already landed do stay — a cancel stops a copy, it does not undo
/// one, and removing a tree the user can see arriving is not something a Cancel
/// button should do on their behalf. Retrying the row resumes into the same
/// destination and the planner skips what is already there.
#[tauri::command]
pub(crate) async fn cancel_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state.worker.stop(&id, TransferStatus::Cancelled).await;
    Ok(())
}

/// Remove one queue row after stopping it. The worker's cancellation path
/// discards its staging/partial output; completed files are never touched.
#[tauri::command]
pub(crate) async fn remove_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    retire(&state.transfers, &state.worker, &id).await;
    Ok(())
}

/// Reveal the local folder which contains a transfer's destination.
///
/// The webview supplies a destination, never a shell command. Resolving the
/// parent as a directory keeps this command from opening a URL or an arbitrary
/// non-local target through the OS shell.
#[tauri::command]
pub(crate) async fn open_transfer_folder(
    destination: String,
) -> std::result::Result<(), CommandError> {
    let destination = std::path::PathBuf::from(destination);
    let folder = destination
        .parent()
        .ok_or_else(|| CommandError::banner("The download has no parent folder"))?;
    let folder =
        std::fs::canonicalize(folder).map_err(|error| CommandError::banner(error.to_string()))?;
    if !folder.is_dir() {
        return Err(CommandError::banner("The download folder is not available"));
    }
    open_in_editor(&folder.to_string_lossy());
    Ok(())
}

/// Put a paused or failed transfer back in the queue.
///
/// A retried **folder** row (E22-S3) is not a second copy of the tree: the
/// session runs the planner again, which skips every file already at the
/// destination on size and mtime, and a file that was torn mid-flight is picked
/// up from the resume journal rather than from the start. So the granularity
/// being the folder costs a re-scan and nothing else — which is the trade that
/// makes one row per folder work at all.
#[tauri::command]
pub(crate) async fn retry_transfer(
    state: tauri::State<'_, AppState>,
    id: String,
) -> std::result::Result<(), ()> {
    state
        .transfers
        .update(&id, |transfer| {
            if !matches!(transfer.status, TransferStatus::Running) {
                transfer.status = TransferStatus::Queued;
                transfer.error = None;
            }
        })
        .await;
    run_queue(&state);
    Ok(())
}

/// Put every failed transfer back in the queue.
///
/// Safe to offer as one gesture only because a share that is not mounted is now
/// a *wait* rather than a failure: re-queueing hundreds of rows against nothing
/// used to mean hundreds of instant failures, which is the retry storm the user
/// already lived through. Rows whose share is still missing sit as `queued ·
/// waiting for its share` until it arrives.
#[tauri::command]
pub(crate) async fn retry_failed_transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.transfers.requeue_failed().await;
    run_queue(&state);
    Ok(())
}

/// Drop every failed transfer.
///
/// The other half of the pair, and separate from `Clear finished` on purpose: a
/// failed row has a `Retry` on it, so sweeping it away with the done ones would
/// throw away a choice the user had not made yet. Here it is a thing they asked
/// for by name.
#[tauri::command]
pub(crate) async fn clear_failed_transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.transfers.clear_failed().await;
    Ok(())
}

/// Pause everything unfinished.
#[tauri::command]
pub(crate) async fn pause_all_transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    for transfer in state.transfers.pause_all().await {
        state
            .worker
            .stop(&transfer.id, TransferStatus::Paused)
            .await;
    }
    Ok(())
}

/// Drop everything finished or cancelled.
#[tauri::command]
pub(crate) async fn clear_finished_transfers(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), ()> {
    state.transfers.clear_finished().await;
    Ok(())
}

/// The folder downloads land in by default.
#[tauri::command]
pub(crate) fn download_folder() -> String {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Read URL lines from text files explicitly dropped on this window.
///
/// The webview receives native drops as paths, so this narrow command is the
/// bridge for XDL-E8-S1. It accepts only a small batch of ordinary `.txt`
/// files and caps each before reading; a drop must never turn into an
/// unbounded IPC allocation. URL validation stays in the Add dialog, where an
/// invalid line can be shown and corrected instead of disappearing silently.
#[tauri::command]
pub(crate) fn download_urls_from_files(
    paths: Vec<String>,
) -> std::result::Result<Vec<String>, String> {
    const MAX_FILES: usize = 16;
    const MAX_FILE_BYTES: u64 = 1024 * 1024;

    if paths.is_empty() || paths.len() > MAX_FILES {
        return Err(format!("drop between 1 and {MAX_FILES} URL text files"));
    }

    let mut urls = Vec::new();
    for raw in paths {
        let path = Path::new(&raw);
        let is_text = path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("txt"));
        if !is_text {
            return Err(format!("{} is not a .txt file", path.display()));
        }
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(format!("{} is larger than 1 MiB", path.display()));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {} as UTF-8 text: {error}", path.display()))?;
        urls.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned),
        );
    }
    Ok(urls)
}

/// Start the worker on whatever is queued, unless it is already running.
///
/// Spawned rather than awaited: `enqueue` returns as soon as the rows exist,
/// so the window paints them immediately and the bytes move behind it.
fn run_queue(state: &tauri::State<'_, AppState>) {
    let worker = Arc::clone(&state.worker);
    tauri::async_runtime::spawn(async move { worker.drain().await });
}

#[cfg(test)]
mod tests {
    use super::download_urls_from_files;

    #[test]
    fn a_dropped_text_file_yields_url_lines_without_comments() {
        let path = std::env::temp_dir().join(format!(
            "excalibur-dropped-urls-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "# exported list\nhttps://a.test/one.iso\n\nhttps://b.test/two.iso\n",
        )
        .expect("write URL list");
        let result = download_urls_from_files(vec![path.to_string_lossy().into_owned()]);
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            result.expect("read URL list"),
            ["https://a.test/one.iso", "https://b.test/two.iso"]
        );
    }
}
