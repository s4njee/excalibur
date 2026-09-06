//! The folder walk behind the Download verb (E13-S4).
//!
//! The cases worth writing down are the ones a happy-path copy never reaches:
//! an empty directory, a subdirectory that cannot be listed, and a listing
//! that names something which must not become a local path.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, ShareBackend, WriteMode,
};
use excalibur_engine::transfer::plan::plan_downloads;
use excalibur_engine::{EngineError, Result};

/// A share with `tree/` in it:
///
/// ```text
/// tree/note.txt
/// tree/a/f1.bin
/// tree/a/b/deep.bin
/// tree/empty/
/// ```
async fn share() -> Arc<MockBackend> {
    let backend = MockBackend::empty();
    for directory in ["tree", "tree/a", "tree/a/b", "tree/empty"] {
        backend.mkdir(directory).await.expect("mkdir");
    }
    for (path, bytes) in [
        ("tree/note.txt", &b"note"[..]),
        ("tree/a/f1.bin", &b"one"[..]),
        ("tree/a/b/deep.bin", &b"deeper"[..]),
    ] {
        backend
            .write_range(path, WriteMode::Create, bytes)
            .await
            .expect("seed");
    }
    Arc::new(backend)
}

/// A backend that answers as the mock does, with one thing rigged.
struct Rigged {
    inner: Arc<MockBackend>,
    /// A directory whose listing fails, as an unreadable one would.
    refuse: Option<String>,
    /// An extra entry to smuggle into every listing.
    smuggle: Option<Entry>,
}

#[async_trait]
impl ShareBackend for Rigged {
    fn mount_info(&self) -> MountInfo {
        self.inner.mount_info()
    }
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    async fn capacity(&self) -> Result<Capacity> {
        self.inner.capacity().await
    }
    async fn list_dir(&self, path: &str) -> Result<Listing> {
        if self.refuse.as_deref() == Some(path) {
            return Err(EngineError::PermissionDenied(path.to_owned()));
        }
        let mut listing = self.inner.list_dir(path).await?;
        if let Some(entry) = self.smuggle.clone() {
            listing.entries.push(entry);
        }
        Ok(listing)
    }
    async fn stat(&self, path: &str) -> Result<Attrs> {
        self.inner.stat(path).await
    }
    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.inner.read_range(path, offset, length).await
    }
    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        self.inner.write_range(path, mode, data).await
    }
    async fn mkdir(&self, path: &str) -> Result<()> {
        self.inner.mkdir(path).await
    }
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.inner.rename(from, to, replace).await
    }
    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.inner.remove(path, recursive).await
    }
    async fn ping(&self) -> Result<Health> {
        self.inner.ping().await
    }
}

fn entry(name: &str, path: &str, kind: EntryKind) -> Entry {
    Entry {
        name: name.to_owned(),
        path: path.to_owned(),
        attrs: Attrs {
            kind,
            size: 1,
            modified_ms: None,
            mode: None,
            link_target: None,
            change_cookie: None,
        },
        kind_label: "Rigged".to_owned(),
        kind_category: excalibur_engine::kind::Category::Other,
        hidden: false,
    }
}

#[tokio::test]
async fn a_folder_becomes_one_row_per_file_under_its_own_name() {
    let share = share().await;
    let into = Path::new("/tmp/downloads");
    let plan = plan_downloads(share.as_ref(), &["tree".to_owned()], into).await;

    let mut landing: Vec<String> = plan
        .files
        .iter()
        .map(|file| file.destination.to_string_lossy().into_owned())
        .collect();
    landing.sort();
    assert_eq!(
        landing,
        [
            "/tmp/downloads/tree/a/b/deep.bin",
            "/tmp/downloads/tree/a/f1.bin",
            "/tmp/downloads/tree/note.txt",
        ]
    );
    // The relative shape is preserved, so a directory does not flatten into
    // one heap of names — where two `deep.bin`s would silently become one.
    assert!(plan.problems.is_empty(), "{:?}", plan.problems);
    assert_eq!(plan.bytes(), 4 + 3 + 6);
}

#[tokio::test]
async fn an_empty_directory_is_still_created() {
    // It has no file to drag its parent into existence, so unless the plan
    // names it, an empty folder simply vanishes from a copy that then reports
    // success.
    let share = share().await;
    let plan = plan_downloads(
        share.as_ref(),
        &["tree".to_owned()],
        Path::new("/tmp/downloads"),
    )
    .await;

    let directories: Vec<String> = plan
        .directories
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    assert!(
        directories.contains(&"/tmp/downloads/tree/empty".to_owned()),
        "{directories:?}"
    );
    // Parents before children, so creating them in order works.
    let root = directories
        .iter()
        .position(|path| path == "/tmp/downloads/tree")
        .expect("the root of the copy");
    let nested = directories
        .iter()
        .position(|path| path == "/tmp/downloads/tree/a/b")
        .expect("a nested directory");
    assert!(root < nested, "{directories:?}");
}

#[tokio::test]
async fn a_subdirectory_that_cannot_be_listed_is_reported_and_not_skipped() {
    // The failure this story exists to prevent: a copy that omits a directory
    // it could not read and says nothing, so the result looks complete.
    let share = Rigged {
        inner: share().await,
        refuse: Some("tree/a".to_owned()),
        smuggle: None,
    };
    let plan = plan_downloads(&share, &["tree".to_owned()], Path::new("/tmp/downloads")).await;

    assert_eq!(plan.problems.len(), 1, "{:?}", plan.problems);
    assert_eq!(plan.problems[0].path, "tree/a");
    assert!(plan.problems[0].reason.contains("permission"));
    // Everything outside the unreadable subtree is still planned.
    assert_eq!(plan.files.len(), 1);
    assert!(plan.files[0].source.ends_with("note.txt"));
}

#[tokio::test]
async fn a_name_from_the_server_never_escapes_the_destination() {
    // A listing is data from somebody else's machine. `..` in it describes a
    // write outside the folder the user picked, and treating it as a filename
    // is how a download becomes a way to overwrite an arbitrary local path.
    let share = Rigged {
        inner: share().await,
        refuse: None,
        smuggle: Some(entry("..", "tree/..", EntryKind::File)),
    };
    let plan = plan_downloads(&share, &["tree".to_owned()], Path::new("/tmp/downloads")).await;

    for file in &plan.files {
        assert!(
            file.destination.starts_with("/tmp/downloads/tree"),
            "{} escaped the destination",
            file.destination.display()
        );
    }
    assert!(
        plan.problems
            .iter()
            .any(|problem| problem.path == "tree/.."),
        "the refusal was not reported: {:?}",
        plan.problems
    );
}

#[tokio::test]
async fn a_listing_that_loops_stops_and_says_so() {
    // A directory that contains itself would otherwise walk forever, and the
    // walk is over answers the server chooses.
    let share = Rigged {
        inner: share().await,
        refuse: None,
        smuggle: Some(entry("a", "tree/a", EntryKind::Dir)),
    };
    let plan = plan_downloads(&share, &["tree".to_owned()], Path::new("/tmp/downloads")).await;

    assert!(
        plan.problems
            .iter()
            .any(|problem| problem.reason.contains("loop")),
        "{:?}",
        plan.problems
    );
}

#[tokio::test]
async fn a_file_is_one_row_and_needs_no_walk() {
    let share = share().await;
    let plan = plan_downloads(
        share.as_ref(),
        &["tree/note.txt".to_owned()],
        Path::new("/tmp/downloads"),
    )
    .await;

    assert_eq!(plan.files.len(), 1);
    assert_eq!(
        plan.files[0].destination.to_string_lossy(),
        "/tmp/downloads/note.txt"
    );
    assert!(plan.directories.is_empty());
}

#[tokio::test]
async fn a_path_that_cannot_be_stated_still_becomes_a_row() {
    // Dropping it would be the silent skip. The row carries the real error
    // from the worker, which is where the user is already looking.
    let share = share().await;
    let plan = plan_downloads(
        share.as_ref(),
        &["tree/gone.txt".to_owned()],
        Path::new("/tmp/downloads"),
    )
    .await;

    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].size, 0);
}

#[test]
fn a_name_that_is_not_a_name_is_refused_before_it_becomes_a_path() {
    // The security rule of this file, reachable on its own since E22-S3: a
    // caller that makes one row for a whole folder needs it as much as the
    // walk that makes one per file, and two copies of it is one too many.
    use excalibur_engine::transfer::plan::local_name;

    assert_eq!(local_name("Media/Show"), Some("Show"));
    assert_eq!(local_name("Media/Show/"), Some("Show"));
    assert_eq!(local_name("Show"), Some("Show"));
    assert_eq!(local_name("Media/.."), None);
    assert_eq!(local_name(".."), None);
    assert_eq!(local_name("."), None);
    assert_eq!(local_name(""), None);
    assert_eq!(local_name("Media/a\\b"), None);
    assert_eq!(local_name("Media/a\0b"), None);
}
