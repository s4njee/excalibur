//! Edit in place: a remote file, in the editor the user already uses.
//!
//! The shape is a round trip, and the only interesting part is the return
//! leg. Downloading is a download. Opening an editor is one command. What
//! costs care is writing the result back **without ever silently overwriting
//! somebody else's change** — which is why every write-back goes through
//! [`crate::backend::ShareBackend::write_all_if_unchanged`] rather than a
//! plain write, and why a refusal comes back as an answer for the user rather
//! than an error to swallow.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::backend::{Attrs, CasOutcome, ShareBackend};
use crate::error::{EngineError, Result};

/// How long an abandoned working copy survives.
///
/// The AC asks for "when the editor's file handle count drops to zero for ten
/// minutes". Counting an editor's open handles is not something a portable app
/// can do — an editor may close the file between saves, or never hold it open
/// at all — so this is ten minutes of *no modification* instead. It errs
/// towards keeping a file the user might still be editing, which is the safe
/// direction.
pub const IDLE_RETENTION: Duration = Duration::from_secs(10 * 60);

/// One file checked out for editing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSession {
    /// Stable id, also the working directory's name.
    pub id: String,
    /// Path on the share.
    pub remote_path: String,
    /// Where the working copy lives.
    pub local_path: String,
    /// Base name, for a toast.
    pub name: String,
    /// How many times it has been written back.
    pub saves: u32,
}

/// What a write-back did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum SaveOutcome {
    /// Written back. `saves` is the running count, for the toast.
    Saved {
        /// How many times this session has been written back.
        saves: u32,
    },
    /// The remote moved. Nothing was written; the user decides what next.
    Conflict {
        /// Size of what is there now, for the prompt.
        size: u64,
        /// Its modification time, for the prompt.
        modified_ms: Option<i64>,
    },
    /// The working copy has not changed since the last write-back.
    Unchanged,
}

/// Every file currently checked out.
pub struct EditStore {
    root: PathBuf,
    sessions: Arc<RwLock<HashMap<String, Checkout>>>,
}

#[derive(Debug, Clone)]
struct Checkout {
    session: EditSession,
    /// The remote as it was when this was last in agreement with it.
    ///
    /// Updated on every successful write-back, so the *next* save compares
    /// against what this editor last wrote rather than against the original —
    /// otherwise the second save of a session would always look like a
    /// conflict with the first.
    expected: Attrs,
    /// Size and mtime of the working copy at the last write-back, so an
    /// unchanged file is not written back on every filesystem event. Editors
    /// touch files they have not changed.
    written: Option<(u64, Option<SystemTime>)>,
}

impl EditStore {
    /// A store rooted at a cache directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// The app's cache directory, per platform.
    ///
    /// # Errors
    ///
    /// Fails when the platform has no cache directory.
    pub fn app_default() -> Result<Self> {
        let base = dirs::cache_dir()
            .ok_or_else(|| EngineError::Io("this platform has no cache directory".to_owned()))?;
        Ok(Self::new(base.join("excalibur").join("edit")))
    }

    /// Check a file out: download it, and remember what the remote looked like.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or the working copy cannot be
    /// written. Refuses outright on a read-only mount: the round trip's whole
    /// purpose is the write back, and starting one that cannot finish would
    /// waste the user's edit.
    pub async fn checkout(
        &self,
        backend: &Arc<dyn ShareBackend>,
        remote_path: &str,
    ) -> Result<EditSession> {
        if !backend.mount_info().writable {
            return Err(EngineError::ReadOnly(backend.mount_info().reason));
        }
        let attrs = backend.stat(remote_path).await?;
        let name = remote_path
            .rsplit('/')
            .next()
            .unwrap_or(remote_path)
            .to_owned();
        let id = session_id(remote_path);

        // One directory per session, named by the id rather than by the file,
        // so two files with the same name from different directories do not
        // land on each other.
        let directory = self.root.join(&id);
        std::fs::create_dir_all(&directory)
            .map_err(|error| EngineError::Io(format!("create {}: {error}", directory.display())))?;
        let local = directory.join(&name);

        let size = u32::try_from(attrs.size.min(u64::from(u32::MAX))).unwrap_or(u32::MAX);
        let contents = backend.read_range(remote_path, 0, size).await?;
        std::fs::write(&local, &contents)
            .map_err(|error| EngineError::Io(format!("write {}: {error}", local.display())))?;

        let session = EditSession {
            id: id.clone(),
            remote_path: remote_path.to_owned(),
            local_path: local.to_string_lossy().into_owned(),
            name,
            saves: 0,
        };
        self.sessions.write().await.insert(
            id,
            Checkout {
                session: session.clone(),
                expected: attrs,
                written: local_stamp(&local),
            },
        );
        Ok(session)
    }

    /// Write a working copy back, refusing if the remote moved.
    ///
    /// # Errors
    ///
    /// Fails when the working copy cannot be read or the write fails. A
    /// *conflict* is `Ok(SaveOutcome::Conflict)`, not an error: the remote
    /// having changed is the answer the caller asked for, and it carries what
    /// is there now so the user can be told.
    pub async fn save(
        &self,
        backend: &Arc<dyn ShareBackend>,
        id: &str,
        force: bool,
    ) -> Result<SaveOutcome> {
        let checkout = {
            let sessions = self.sessions.read().await;
            sessions
                .get(id)
                .cloned()
                .ok_or_else(|| EngineError::NotFound(format!("no edit session {id}")))?
        };
        let local = PathBuf::from(&checkout.session.local_path);
        let stamp = local_stamp(&local);
        // An editor writing a file it did not change is common — a save with
        // no edits, a plugin touching mtime. Uploading for each would put a
        // row in the queue for nothing.
        if !force && stamp.is_some() && stamp == checkout.written {
            return Ok(SaveOutcome::Unchanged);
        }
        let contents = std::fs::read(&local)
            .map_err(|error| EngineError::Io(format!("read {}: {error}", local.display())))?;

        let expected = if force {
            None
        } else {
            Some(&checkout.expected)
        };
        match backend
            .write_all_if_unchanged(&checkout.session.remote_path, expected, &contents)
            .await?
        {
            CasOutcome::Written(attrs) => {
                let mut sessions = self.sessions.write().await;
                let saves = if let Some(entry) = sessions.get_mut(id) {
                    entry.session.saves += 1;
                    // The next save compares against what this editor just
                    // wrote, not against the original.
                    entry.expected = attrs;
                    entry.written = stamp;
                    entry.session.saves
                } else {
                    1
                };
                Ok(SaveOutcome::Saved { saves })
            }
            CasOutcome::Changed(current) => Ok(SaveOutcome::Conflict {
                size: current.size,
                modified_ms: current.modified_ms,
            }),
        }
    }

    /// Every session currently checked out.
    pub async fn all(&self) -> Vec<EditSession> {
        self.sessions
            .read()
            .await
            .values()
            .map(|checkout| checkout.session.clone())
            .collect()
    }

    /// Forget a session and remove its working copy.
    pub async fn close(&self, id: &str) {
        let removed = self.sessions.write().await.remove(id);
        if let Some(checkout) = removed {
            let _ = std::fs::remove_dir_all(self.root.join(&checkout.session.id));
        }
    }

    /// Remove working copies nothing has touched for [`IDLE_RETENTION`].
    ///
    /// Only ones with no unsaved change: a file the user edited and has not
    /// written back is the last thing to delete on a timer, whatever the AC's
    /// ten minutes says about handles.
    pub async fn sweep(&self) -> Vec<String> {
        let now = SystemTime::now();
        let stale: Vec<String> = self
            .sessions
            .read()
            .await
            .values()
            .filter(|checkout| {
                let local = Path::new(&checkout.session.local_path);
                let stamp = local_stamp(local);
                let unsaved = stamp.is_some() && stamp != checkout.written;
                if unsaved {
                    return false;
                }
                stamp
                    .and_then(|(_, modified)| modified)
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|idle| idle > IDLE_RETENTION)
            })
            .map(|checkout| checkout.session.id.clone())
            .collect();
        for id in &stale {
            self.close(id).await;
        }
        stale
    }
}

/// Size and modification time of a working copy, if it is there.
fn local_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

/// A session id from a remote path.
///
/// Derived rather than random, so opening the same file twice reuses one
/// working copy instead of leaving the first edit orphaned in a directory
/// nobody will look in again.
fn session_id(remote_path: &str) -> String {
    blake3::hash(remote_path.as_bytes()).to_hex()[..16].to_owned()
}
