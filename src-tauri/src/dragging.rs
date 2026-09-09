//! Native drag and drop, and the file promises a drag into Finder leaves
//! behind.

use std::path::Path;
use std::sync::Arc;

use excalibur_engine::drag::{self, Selection};
use excalibur_engine::transfer::plan;
use excalibur_engine::transfer::queue::{QueueEvent, TransferQueue};
use excalibur_engine::transfer::worker::Worker;
use excalibur_engine::{Direction, EngineError, Result, SessionManager, Transfer, TransferStatus};
#[cfg(target_os = "macos")]
use tauri::{Emitter, Manager};

use crate::error::CommandError;
#[cfg(target_os = "macos")]
use crate::events::{DRAG_DROP, DRAG_LEAVE, DRAG_OVER};
use crate::state::AppState;
use crate::util::new_id;

/// What fulfils a promise once Finder asks for it (E24-S1).
///
/// Built from the state's `Arc`s rather than borrowing it, because the closure
/// outlives the command: it is called from `AppKit`'s operation queue, at
/// whatever later moment somebody accepts the drop, and `tauri::State` is a
/// borrow of the app that cannot travel there.
pub(crate) fn promise_sink(state: &tauri::State<'_, AppState>) -> drag::PromiseSink {
    let sessions = Arc::clone(&state.sessions);
    let transfers = Arc::clone(&state.transfers);
    let worker = Arc::clone(&state.worker);
    Arc::new(move |request: &drag::PromiseRequest| {
        // Not a tokio worker thread — `AppKit` owns this one — so blocking on
        // the runtime here blocks nothing else. It is also required: the
        // promise is complete when this returns, which is how Finder knows to
        // stop showing a copy in progress.
        tauri::async_runtime::block_on(fulfil_promise(&sessions, &transfers, &worker, request))
    })
}

/// Fetch one promised item into the folder Finder named, and land it there.
///
/// **Staged in a hidden directory inside that folder, then renamed into place.**
/// Two reasons, and both of them are silent data loss without it:
///
/// - the name is Finder's, not ours. A second `Arrival.mkv` dropped on a
///   Desktop that already has one arrives as `Arrival 2.mkv`, and a transfer
///   written straight into the folder would use the name it promised and
///   overwrite the file already there;
/// - a rename inside one directory is on one filesystem, so it is atomic and
///   free. Nothing half-copied ever appears under the final name.
///
/// The transfer itself is the ordinary queue's, so a promise is visible in the
/// transfers sheet while it runs and can be paused or cancelled there — and it
/// is **one row for the whole item**, folder or file, since E22-S3.
///
/// The row is taken back out of the queue when the promise ends, whatever
/// happened. It is the one row in the app whose destination stops existing the
/// moment it finishes, and a queue file that outlived the drag that made it is
/// how this leaked: `TransferQueue::restore` re-queues anything unfinished on
/// the next launch, so an abandoned drag came back as a transfer copying into a
/// staging directory nobody was ever going to sweep. See [`retire`].
async fn fulfil_promise(
    sessions: &Arc<SessionManager>,
    transfers: &Arc<TransferQueue>,
    worker: &Arc<Worker>,
    request: &drag::PromiseRequest,
) -> Result<()> {
    let mount = &request.selection.mount;
    if sessions.backend(mount).await.is_none() {
        return Err(EngineError::Disconnected(format!(
            "{mount} is not connected"
        )));
    }
    let staging = PromiseStaging::inside(request.folder())?;
    let landed = fetch_into(transfers, worker, request, staging.path()).await;
    match landed {
        Ok(landed) => place(&landed, &request.destination),
        Err(error) => Err(error),
    }
    // `staging` is swept here, on every path out including a panic.
}

/// A promise's staging directory, swept when it goes out of scope.
///
/// A guard rather than a `remove_dir_all` at the end of [`fulfil_promise`],
/// because what used to sit there was a comment asking the reader to keep an
/// invariant — "every early return from here on has to take the staging
/// directory with it". A `?` kept it. The `catch_unwind` one frame up in
/// `drag::macos`, which turns a panic in this sink into a refused promise, did
/// not; nor did a `⌘Q` accepted while a copy was still running. Three
/// `.excalibur-*` directories holding 42 MB were found in a real Downloads
/// folder, which is the cost of an invariant a type was not asked to keep.
struct PromiseStaging(std::path::PathBuf);

impl PromiseStaging {
    /// A fresh hidden directory inside the folder Finder named.
    fn inside(folder: &Path) -> Result<Self> {
        let directory = folder.join(format!(".excalibur-{}", new_id()));
        std::fs::create_dir_all(&directory)
            .map_err(|error| EngineError::Io(format!("create {}: {error}", directory.display())))?;
        Ok(Self(directory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PromiseStaging {
    fn drop(&mut self) {
        // Best effort, like every other sweep in this app: failing a drag that
        // worked because its scaffolding could not be removed is the worse
        // trade. [`sweep_abandoned_staging`] is the backstop for a process that
        // died before getting here.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the transfer, and answer with the one thing that landed in `staging`.
async fn fetch_into(
    transfers: &Arc<TransferQueue>,
    worker: &Arc<Worker>,
    request: &drag::PromiseRequest,
    staging: &Path,
) -> Result<std::path::PathBuf> {
    // Names from the server are not paths, the same rule `enqueue_downloads`
    // keeps — and here it guards a write into somebody's Desktop.
    let name = plan::local_name(&request.path).ok_or_else(|| {
        EngineError::Io(format!(
            "{:?} is not a name that can be written locally",
            request.path
        ))
    })?;
    // Subscribed before anything is queued: a small file can finish before the
    // wait below starts, and a wait that missed its own transfer would hold
    // Finder's progress open until the queue happened to change again.
    let changes = transfers.events();
    let id = new_id();
    transfers
        .add(
            Transfer::new(
                id.clone(),
                Direction::Down,
                request.path.clone(),
                staging.join(name).to_string_lossy().into_owned(),
                0,
            )
            .on_mount(&request.selection.mount),
        )
        .await;
    let running = Arc::clone(worker);
    tauri::async_runtime::spawn(async move { running.drain().await });

    let settled = settle(transfers, changes, &id).await;
    retire(transfers, worker, &id).await;
    settled?;
    only_child(staging)
}

/// Wait for one row to stop moving, or say why it did not land.
///
/// The broadcast channel is a **wake-up**, not the record: it drops messages
/// for a slow consumer without telling it, so every pass re-reads the queue,
/// which is the only thing that knows the truth. The timeout is what makes a
/// dropped or closed channel a slower wait rather than one that never ends.
async fn settle(
    transfers: &Arc<TransferQueue>,
    mut changes: tokio::sync::broadcast::Receiver<QueueEvent>,
    id: &str,
) -> Result<()> {
    loop {
        let row = transfers.all().await.into_iter().find(|row| row.id == id);
        // A row cleared out from under us — `Clear finished` landing between the
        // copy ending and this pass — is one nobody can report on any more, so
        // the wait ends and `only_child` becomes the judge of whether anything
        // arrived. Inventing a failure here would refuse a promise that worked.
        let Some(row) = row else {
            return Ok(());
        };
        match row.status {
            TransferStatus::Done => return Ok(()),
            // Paused counts as an ending, not as a wait. The user pausing a
            // queue must not leave Finder spinning on a copy that is never
            // going to move again.
            TransferStatus::Failed | TransferStatus::Cancelled | TransferStatus::Paused => {
                return Err(EngineError::Io(
                    row.error
                        .clone()
                        .unwrap_or_else(|| format!("{} did not finish", row.name)),
                ));
            }
            TransferStatus::Queued | TransferStatus::Running => {}
        }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), changes.recv()).await;
    }
}

/// How long a retired promise waits for its session to let go of the stage.
///
/// A cancel reaches the engine on its own poll — a tenth of a second in both
/// adapters — and the session then finishes the write it is inside before it
/// returns. Two seconds is far past that and short enough that a wedged session
/// does not hold up the drag; the sweep goes ahead either way, because a
/// directory removed while something is still writing is a worse outcome only
/// for that write, and a directory never removed is litter forever.
const RETIRE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop a promise's row, wait for the session to let go, and forget the row.
///
/// Both halves matter and neither used to happen. **Stopping** is what a
/// cancelled or paused promise needs: the old code returned on the first bad
/// row and left its siblings running, so the staging directory was removed and
/// then re-created underneath by a session still writing into it —
/// `LocalSink` calls `create_dir_all` on its own root. **Forgetting** is what a
/// restart needs: a row that names a staging directory must never be in the
/// file that `TransferQueue::restore` re-queues from.
pub(crate) async fn retire(transfers: &Arc<TransferQueue>, worker: &Arc<Worker>, id: &str) {
    worker.stop(id, TransferStatus::Cancelled).await;
    let deadline = std::time::Instant::now() + RETIRE_WAIT;
    while worker.is_busy(id).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    transfers.remove(id).await;
}

/// Remove staging directories a previous run left behind, and their rows.
///
/// The only orphan the guard on [`PromiseStaging`] cannot catch is a process
/// that stopped existing — a quit accepted mid-drag, a crash, a kill. What such
/// a run does leave is its **row**, in a queue file that names the staging
/// directory in `dest_path`, so the app can say which directories are its own
/// rather than deleting anything that matches a pattern in somebody's folder.
///
/// Called before the restored queue is drained, because draining it is the
/// thing that would otherwise copy a whole tree back into one of these.
pub(crate) async fn sweep_abandoned_staging(transfers: &Arc<TransferQueue>) {
    let mut doomed: Vec<String> = Vec::new();
    let mut directories: std::collections::BTreeSet<std::path::PathBuf> =
        std::collections::BTreeSet::new();
    for row in transfers.all().await {
        if let Some(staging) = staging_root(Path::new(&row.dest_path)) {
            directories.insert(staging);
            doomed.push(row.id);
        }
    }
    if doomed.is_empty() {
        return;
    }

    // Gathered first, then done once each. One abandoned drag leaves *one*
    // directory and a row per file in it — thousands, for a folder of any size
    // — so removing per row would call `remove_dir_all` on the same path
    // thousands of times and rewrite the queue file after every one of them.
    // That is quadratic in the bytes and it was seconds of a blank window
    // before the first paint.
    //
    // Off the async runtime because `remove_dir_all` blocks, and this runs on
    // the way up: a blocked runtime thread here is a window that has not been
    // built yet.
    let _ = tauri::async_runtime::spawn_blocking(move || {
        for staging in directories {
            let _ = std::fs::remove_dir_all(staging);
        }
    })
    .await;
    transfers.remove_all(&doomed).await;
}

/// The `.excalibur-*` ancestor of a path, when it has one.
///
/// Two narrowings, because this answer becomes a `remove_dir_all` and a false
/// positive is somebody's data. It matches a whole path **component**, so a
/// folder called `my.excalibur-stuff` is not one; and only a **proper
/// ancestor**, so a file the user downloaded and called `.excalibur-notes.txt`
/// is not one either — a staging directory always has the landed item inside
/// it, which is why the row's path continues past it.
fn staging_root(destination: &Path) -> Option<std::path::PathBuf> {
    let mut walked = std::path::PathBuf::new();
    let mut parts = destination.iter().peekable();
    while let Some(part) = parts.next() {
        walked.push(part);
        if parts.peek().is_some()
            && part
                .to_str()
                .is_some_and(|part| part.starts_with(".excalibur-"))
        {
            return Some(walked);
        }
    }
    None
}

/// The single entry a fulfilled promise left in its staging directory.
///
/// One, always: the plan was made from one path, so it produced either a file
/// or a tree with one root. Anything else means the walk and this function
/// disagree, which is worth an error rather than a guess about which of them.
fn only_child(staging: &Path) -> Result<std::path::PathBuf> {
    let mut entries = std::fs::read_dir(staging)
        .map_err(|error| EngineError::Io(format!("read {}: {error}", staging.display())))?
        .filter_map(std::result::Result::ok);
    let first = entries
        .next()
        .ok_or_else(|| EngineError::Io("the copy produced nothing".to_owned()))?;
    if entries.next().is_some() {
        return Err(EngineError::Io(
            "the copy produced more than one item".to_owned(),
        ));
    }
    Ok(first.path())
}

/// Move what landed to the exact path Finder asked for.
///
/// Whatever is already at that path is removed first. It is not a file of the
/// user's: Finder chose this name precisely because nothing of theirs had it,
/// and what may be there is the empty placeholder it reserved the name with.
fn place(landed: &Path, destination: &Path) -> Result<()> {
    if destination.is_dir() {
        let _ = std::fs::remove_dir_all(destination);
    } else if destination.exists() {
        let _ = std::fs::remove_file(destination);
    }
    std::fs::rename(landed, destination).map_err(|error| {
        EngineError::Io(format!("put {} in place: {error}", destination.display()))
    })
}

/// Install a drop target on every window, then begin the session. Main thread.
///
/// Every window, including the one the drag starts in: a drop back onto the
/// source window's own list — into a folder row, or onto the sidebar — is a
/// move like any other, and a source window that could not receive its own drag
/// would make the commonest gesture in a file manager the one that does
/// nothing. Windows that have no native view yet are skipped rather than
/// failing the drag; one window mid-creation must not stop a drag in another.
#[cfg(target_os = "macos")]
pub(crate) fn open_drag(
    app: &tauri::AppHandle,
    source: &str,
    selection: &Selection,
    promises: &[drag::Promise],
    image: &[u8],
    fulfil: &drag::PromiseSink,
) -> excalibur_engine::error::Result<()> {
    // Defensive: a drag whose end we somehow missed would leave an invisible
    // view over a window, swallowing clicks. Cheap to redo, expensive to debug.
    drag::remove_drop_targets();

    let handle = app.clone();
    let sink: drag::DropSink = Arc::new(move |label: &str, event: drag::DropEvent| {
        // To *that* window's webview, never broadcast: two windows would
        // otherwise both highlight a row for a pointer that is over one of them
        // (E23-S1's rule, applied to a third event stream).
        let name = match event.phase {
            drag::Phase::Over => DRAG_OVER,
            drag::Phase::Drop => DRAG_DROP,
            drag::Phase::Leave => DRAG_LEAVE,
        };
        let _ = handle.emit_to(label, name, &event);
    });

    let mut from = None;
    for (label, other) in app.webview_windows() {
        let Ok(view) = other.ns_view() else { continue };
        drag::install_drop_target(&label, view, Arc::clone(&sink))?;
        if label == source {
            from = Some(view);
        }
    }
    let Some(view) = from else {
        drag::remove_drop_targets();
        return Err(EngineError::Unsupported(
            "the window that started the drag has no native view".to_owned(),
        ));
    };
    drag::start_drag(view, selection, promises, image, fulfil)
}

/// Linux gets this with E24-S4; until then a drag is a gesture that does nothing.
#[cfg(not(target_os = "macos"))]
pub(crate) fn open_drag(
    app: &tauri::AppHandle,
    source: &str,
    selection: &Selection,
    promises: &[drag::Promise],
    image: &[u8],
    fulfil: &drag::PromiseSink,
) -> excalibur_engine::error::Result<()> {
    let _ = (app, source, selection, promises, image, fulfil);
    Err(EngineError::Unsupported(
        "dragging between windows is macOS-only until E24-S4".to_owned(),
    ))
}

pub(crate) fn drag_failed(why: &str) -> CommandError {
    CommandError::banner(why)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::staging_root;

    #[test]
    fn a_row_inside_a_staging_directory_names_the_directory_to_sweep() {
        // What the startup sweep matches on. The answer has to be the staging
        // directory itself and not the file under it, because the whole tree
        // is scaffolding.
        assert_eq!(
            staging_root(Path::new(
                "/Users/x/Downloads/.excalibur-1a2b-3/Show/season 1/e01.txt"
            )),
            Some(Path::new("/Users/x/Downloads/.excalibur-1a2b-3").to_path_buf())
        );
    }

    #[test]
    fn an_ordinary_download_is_never_swept() {
        // The sweep removes directories, so a false positive is data loss. It
        // matches a whole path component: a file the user happens to have
        // called `.excalibur-notes.txt` must not take its folder with it.
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/Show/season 1/e01.txt")),
            None
        );
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/.excalibur-notes.txt")),
            None
        );
        assert_eq!(
            staging_root(Path::new("/Users/x/Downloads/my.excalibur-stuff/a.txt")),
            None
        );
    }
}
