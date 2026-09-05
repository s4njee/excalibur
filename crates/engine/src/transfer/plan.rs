//! Turning a selection into rows (E13-S4).
//!
//! A folder is not a transfer. It expands to one row per file, because a
//! single row for a directory can only report a percentage that means nothing
//! when one file in it fails — so the walk that produces those rows has to
//! happen somewhere, and it happens here rather than in the Tauri command so
//! that it can be tested without a window.
//!
//! Two rules the walk exists to keep:
//!
//! - **Nothing is skipped silently.** A subdirectory that cannot be listed is
//!   a reported problem, not one fewer file. The alternative is a copy that
//!   looks complete and is not, which is the worst outcome a file transfer has.
//! - **Names from the server are not paths.** Every component is checked before
//!   it becomes part of a local path. A server that answers a listing with
//!   `..` — or with a name containing a separator — is describing a write
//!   outside the folder the user chose, and that is a security bug rather than
//!   an odd filename.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::backend::{EntryKind, ShareBackend};

/// One file to fetch, and where it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDownload {
    /// Share-relative source path.
    pub source: String,
    /// Absolute local destination.
    pub destination: PathBuf,
    /// Size as of the walk, or `0` when it could not be read.
    pub size: u64,
}

/// Something the walk could not do, in the words the row will show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProblem {
    /// The share path it happened at.
    pub path: String,
    /// Where its contents would have gone.
    pub destination: PathBuf,
    /// What went wrong.
    pub reason: String,
}

/// What a selection expands to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadPlan {
    /// One entry per file, in the order the walk found them.
    pub files: Vec<PlannedDownload>,
    /// Every local directory the plan needs, parents before children.
    ///
    /// Listed rather than left to each file's parent being created on demand,
    /// because an **empty** remote directory has no file to trigger that and
    /// would otherwise vanish from the copy.
    pub directories: Vec<PathBuf>,
    /// What could not be walked. Never silently dropped.
    pub problems: Vec<PlanProblem>,
}

impl DownloadPlan {
    /// Bytes the plan expects to move, as far as it could tell.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// Directories visited before the walk gives up.
///
/// A depth-first walk over a *server's* answers is a walk over data somebody
/// else controls: a listing that names its own parent is a loop, and the
/// visited set below already stops that one. This is the backstop for the
/// shape it cannot see — a generated tree that is simply endless. 100k
/// directories is far past any real share and far short of running the machine
/// out of memory.
const MAX_DIRECTORIES: usize = 100_000;

/// Expand `paths` into one row per file, under `destination`.
///
/// A file becomes one row at `<destination>/<name>`. A directory becomes every
/// file beneath it at `<destination>/<name>/<relative path>`, plus the
/// directories themselves so the empty ones survive.
///
/// Never fails: a path that cannot be walked becomes a [`PlanProblem`], and a
/// file whose `stat` fails still becomes a row with an unknown size — the
/// worker will report the real error on that row, which is where the user is
/// looking. Returning an error instead would let one unreadable file cancel a
/// copy of ten thousand.
pub async fn plan_downloads(
    share: &dyn ShareBackend,
    paths: &[String],
    destination: &Path,
) -> DownloadPlan {
    let mut plan = DownloadPlan::default();
    let mut visited: HashSet<String> = HashSet::new();

    for path in paths {
        let trimmed = path.trim_end_matches('/');
        let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
        let Some(name) = safe_component(name) else {
            plan.problems.push(PlanProblem {
                path: path.clone(),
                destination: destination.to_path_buf(),
                reason: format!("{name:?} is not a name that can be written locally"),
            });
            continue;
        };
        let local = destination.join(name);

        // A stat that fails is not a reason to drop the item: it may be a
        // permission error the row should show, and treating it as a file of
        // unknown size gets it in front of the user either way.
        let attrs = share.stat(path).await.ok();
        if attrs.as_ref().map(|attrs| attrs.kind) != Some(EntryKind::Dir) {
            plan.files.push(PlannedDownload {
                source: path.clone(),
                destination: local,
                size: attrs.map_or(0, |attrs| attrs.size),
            });
            continue;
        }
        walk(share, path, &local, &mut plan, &mut visited).await;
    }

    plan
}

/// Depth-first, iteratively.
///
/// Iterative rather than recursive for the same reason `XsyncBackend::remove`
/// is: an `async fn` that calls itself needs boxing, and a deep tree would be
/// bounded by this process's stack rather than by the share.
async fn walk(
    share: &dyn ShareBackend,
    root: &str,
    local_root: &Path,
    plan: &mut DownloadPlan,
    visited: &mut HashSet<String>,
) {
    let mut pending = vec![(root.to_owned(), local_root.to_path_buf())];
    // The root itself, so an empty folder is still created.
    plan.directories.push(local_root.to_path_buf());
    visited.insert(root.to_owned());

    while let Some((remote, local)) = pending.pop() {
        if plan.directories.len() > MAX_DIRECTORIES {
            plan.problems.push(PlanProblem {
                path: remote,
                destination: local,
                reason: format!("gave up after {MAX_DIRECTORIES} directories"),
            });
            return;
        }
        let listing = match share.list_dir(&remote).await {
            Ok(listing) => listing,
            Err(error) => {
                // The whole point of the problem list: this subtree is missing
                // from the copy and the user is told so.
                plan.problems.push(PlanProblem {
                    path: remote,
                    destination: local,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        for entry in listing.entries {
            let Some(name) = safe_component(&entry.name) else {
                plan.problems.push(PlanProblem {
                    path: entry.path,
                    destination: local.clone(),
                    reason: format!("{:?} is not a name that can be written locally", entry.name),
                });
                continue;
            };
            let child = local.join(name);
            // A symlink is copied as the bytes it points at, like every other
            // read: following it as a *directory* would let a link to `/` walk
            // the server's whole filesystem into someone's Downloads folder.
            if entry.attrs.kind == EntryKind::Dir {
                if !visited.insert(entry.path.clone()) {
                    // A listing that names a directory already walked is a
                    // loop. Stopping is the answer; saying so is the report.
                    plan.problems.push(PlanProblem {
                        path: entry.path,
                        destination: child,
                        reason: "already visited: the listing describes a loop".to_owned(),
                    });
                    continue;
                }
                plan.directories.push(child.clone());
                pending.push((entry.path, child));
            } else {
                plan.files.push(PlannedDownload {
                    source: entry.path,
                    destination: child,
                    size: entry.attrs.size,
                });
            }
        }
    }
}

/// One path component, or `None` when it must not become one.
///
/// `..` is the one that matters — a listing containing it would write outside
/// the folder the user picked — but the rest are equally not names: a
/// separator would silently create a directory level, and an empty component
/// or a NUL is not a filename on any platform this ships to.
fn safe_component(name: &str) -> Option<&str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return None;
    }
    Some(name)
}
