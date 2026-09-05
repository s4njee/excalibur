//! The local backend against a real directory (E21-S1).
//!
//! Unlike the protocol backends, this one needs no server and no fixture, so
//! these run everywhere and always. The cases worth writing down are the ones
//! a happy-path listing never reaches: a path that tries to climb out of the
//! share, a root this user cannot write to, a symlink pointing somewhere else
//! entirely, and a delete that has to be undoable.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::ShareBackend;
use excalibur_engine::{EngineError, EntryKind, WriteMode};

/// A share with something of every shape in it.
fn share() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(root.path().join("Documentaries")).unwrap();
    std::fs::write(root.path().join("notes.txt"), b"hello from a real folder").unwrap();
    std::fs::write(root.path().join("Arrival.2016.mkv"), vec![7_u8; 4096]).unwrap();
    std::fs::write(root.path().join(".DS_Store"), b"x").unwrap();
    root
}

/// A name nothing else could own, for the tests that put something in the
/// real Trash and have to take it out again afterwards.
fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Where the platform's Trash put `name`, if this is a platform with one whose
/// layout these tests know.
fn trashed(name: &str) -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var("HOME").ok()?);
    #[cfg(target_os = "macos")]
    let candidate = home.join(".Trash").join(name);
    #[cfg(not(target_os = "macos"))]
    let candidate = std::env::var("XDG_DATA_HOME")
        .map_or_else(|_| home.join(".local/share"), std::path::PathBuf::from)
        .join("Trash/files")
        .join(name);
    candidate.exists().then_some(candidate)
}

/// Take this test's own item back out of the real Trash.
///
/// Only ever the exact unique name it put there, so nothing a person actually
/// deleted can be caught by it.
fn tidy_trash(name: &str) {
    let Some(path) = trashed(name) else { return };
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };
    // The freedesktop layout keeps a sidecar beside the file; macOS does not.
    if let Some(parent) = path.parent().and_then(|files| files.parent()) {
        let _ = std::fs::remove_file(parent.join("info").join(format!("{name}.trashinfo")));
    }
}

/// Whether this process is root, for which a mode-0500 directory is no
/// obstacle and the write-probe test would be meaningless.
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

#[tokio::test]
async fn lists_a_real_folder_with_kinds_sizes_and_hiddenness() {
    let root = share();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");

    let listing = backend.list_dir("").await.expect("list the root");
    assert_eq!(listing.entries.len(), 4);
    assert!(listing.mount.writable, "{}", listing.mount.reason);
    assert_eq!(listing.mount.protocol_label, "local");
    assert_eq!(listing.mount.security_label, "local");

    let film = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Arrival.2016.mkv")
        .expect("the film");
    assert_eq!(film.attrs.size, 4096);
    assert_eq!(film.attrs.kind, EntryKind::File);
    // The label comes from the shared table, not from anything the filesystem
    // knows: the same name is the same kind on every backend.
    assert_eq!(film.kind_label, "Matroska Video");
    assert!(film.attrs.modified_ms.is_some_and(|ms| ms > 0));
    assert!(film.attrs.mode.is_some_and(|mode| mode & 0o400 != 0));

    let hidden = listing
        .entries
        .iter()
        .find(|entry| entry.name == ".DS_Store")
        .expect("the dotfile");
    assert!(hidden.hidden);
    assert!(!film.hidden);

    let folder = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Documentaries")
        .expect("the folder");
    assert_eq!(folder.attrs.kind, EntryKind::Dir);
    assert_eq!(folder.path, "Documentaries");

    // A nested listing's paths are share-relative and slash-separated, which
    // is what the window navigates with.
    std::fs::write(root.path().join("Documentaries/Koyaanisqatsi.mp4"), b"x").unwrap();
    let nested = backend.list_dir("Documentaries").await.expect("list");
    assert_eq!(nested.entries[0].path, "Documentaries/Koyaanisqatsi.mp4");
}

#[tokio::test]
async fn stats_reads_ranges_and_reports_capacity() {
    let root = share();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");

    let attrs = backend.stat("notes.txt").await.expect("stat");
    assert_eq!(attrs.kind, EntryKind::File);
    assert_eq!(attrs.size, 24);

    let whole = backend
        .read_range("notes.txt", 0, 4096)
        .await
        .expect("read");
    assert_eq!(whole, b"hello from a real folder");
    let ranged = backend.read_range("notes.txt", 6, 4).await.expect("range");
    assert_eq!(ranged, b"from");
    // Past the end is an empty read, not a failure: the trait lets a caller
    // stop on a short read, so it has to be able to ask for one.
    assert!(backend
        .read_range("notes.txt", 4096, 16)
        .await
        .expect("read past the end")
        .is_empty());

    assert!(matches!(
        backend.stat("nothing.txt").await,
        Err(EngineError::NotFound(_))
    ));

    let capacity = backend.capacity().await.expect("capacity");
    assert!(capacity.total > 0);
    assert!(capacity.free <= capacity.total);
    // It rides with the listing too, because the status bar and the rows above
    // it must not be able to disagree.
    let mount = backend.list_dir("").await.expect("list").mount;
    assert!(mount.capacity.is_some_and(|it| it.total > 0));
}

#[tokio::test]
async fn one_handle_serves_concurrent_reads_of_the_same_file() {
    let root = share();
    // Distinguishable per position, so an interleaving that mixed two reads up
    // would show as wrong bytes rather than as the right length.
    let bytes: Vec<u8> = (0..64_u32 * 1024)
        .map(|index| u8::try_from(index % 256).expect("in range"))
        .collect();
    std::fs::write(root.path().join("big.bin"), &bytes).unwrap();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");

    let handle = backend
        .open_read("big.bin")
        .await
        .expect("open")
        .expect("the local backend has a handle to give");
    assert!(handle.max_read() >= 1024 * 1024);

    let (first, second, third) = tokio::join!(
        handle.read_at(0, 4096),
        handle.read_at(32 * 1024, 4096),
        handle.read_at(60 * 1024, 8192),
    );
    assert_eq!(first.expect("first"), bytes[..4096]);
    assert_eq!(second.expect("second"), bytes[32 * 1024..32 * 1024 + 4096]);
    // Short only at the end of the file, which is the one place the trait
    // allows it.
    assert_eq!(third.expect("third"), bytes[60 * 1024..]);

    handle.close().await.expect("close");
    // Closing twice is not an error; reading after it is.
    handle.close().await.expect("close again");
    assert!(handle.read_at(0, 16).await.is_err());
}

#[tokio::test]
async fn writes_create_resume_mkdir_and_rename() {
    let root = share();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");

    backend
        .write_range("fresh.txt", WriteMode::Create, b"written by excalibur")
        .await
        .expect("write");
    assert_eq!(
        std::fs::read(root.path().join("fresh.txt")).unwrap(),
        b"written by excalibur"
    );

    // A resumed upload lands at the offset it stopped at. Appending would put
    // it at whatever the length happens to be, which is the same thing only
    // when nothing else has touched the file.
    backend
        .write_range(
            "fresh.txt",
            WriteMode::Resume { offset: 8 },
            b"BY EXCALIBUR",
        )
        .await
        .expect("resume");
    assert_eq!(
        std::fs::read(root.path().join("fresh.txt")).unwrap(),
        b"written BY EXCALIBUR"
    );

    backend.mkdir("Shorts").await.expect("mkdir");
    assert!(root.path().join("Shorts").is_dir());
    assert!(matches!(
        backend.mkdir("Shorts").await,
        Err(EngineError::AlreadyExists(_))
    ));

    backend
        .rename("fresh.txt", "Shorts/moved.txt", false)
        .await
        .expect("rename");
    assert!(root.path().join("Shorts/moved.txt").is_file());
    assert!(!root.path().join("fresh.txt").exists());

    // Without `replace`, an occupied destination is refused rather than
    // silently overwritten by `rename(2)`.
    std::fs::write(root.path().join("occupied.txt"), b"mine").unwrap();
    assert!(matches!(
        backend
            .rename("Shorts/moved.txt", "occupied.txt", false)
            .await,
        Err(EngineError::AlreadyExists(_))
    ));
    backend
        .rename("Shorts/moved.txt", "occupied.txt", true)
        .await
        .expect("replace");
    assert_eq!(
        std::fs::read(root.path().join("occupied.txt")).unwrap(),
        b"written BY EXCALIBUR"
    );
}

#[tokio::test]
async fn remove_takes_a_file_out_of_the_share_and_into_the_trash() {
    let root = share();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");
    assert_eq!(backend.mount_info().options, "trash");

    // Unique, so that what is asserted about the Trash afterwards is this
    // test's file and could be nothing else.
    let name = unique("excalibur-trash-probe.txt");
    std::fs::write(root.path().join(&name), b"deleted by a test").unwrap();

    backend.remove(&name, false).await.expect("remove");
    assert!(
        !root.path().join(&name).exists(),
        "it has to leave the share"
    );

    // Deleting a file on your own Mac must not be more permanent than Finder
    // makes it, and "not permanent" is only checkable by finding it again.
    #[cfg(target_os = "macos")]
    {
        let landed = trashed(&name).expect("it should be in the Trash, not gone");
        assert_eq!(std::fs::read(&landed).unwrap(), b"deleted by a test");
    }
    // This test put it there; this test takes it away again.
    tidy_trash(&name);
}

#[tokio::test]
async fn a_folder_goes_whole_and_only_when_recursive() {
    let root = share();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");
    // Named uniquely because it ends up in the real Trash, which this test has
    // to be able to find it in and take it out of again.
    let folder = unique("excalibur-trash-folder");
    std::fs::create_dir(root.path().join(&folder)).unwrap();
    std::fs::write(root.path().join(&folder).join("Baraka.mkv"), b"x").unwrap();

    // Non-recursive on a directory with something in it is refused, the same
    // as `rmdir`. Trashing it anyway would make the flag a lie.
    assert!(backend.remove(&folder, false).await.is_err());
    assert!(root.path().join(&folder).is_dir());

    backend.remove(&folder, true).await.expect("remove");
    assert!(!root.path().join(&folder).exists());
    // Whole, contents and all: one item in the Trash and one Put Back, rather
    // than an empty folder the user can restore and nothing inside it.
    #[cfg(target_os = "macos")]
    {
        let landed = trashed(&folder).expect("the folder should be in the Trash, whole");
        assert!(landed.join("Baraka.mkv").is_file());
    }
    tidy_trash(&folder);

    // The share's own root is not a thing the share can delete.
    assert!(matches!(
        backend.remove("", true).await,
        Err(EngineError::Unsupported(_))
    ));
}

#[tokio::test]
async fn a_read_only_mount_refuses_every_write_at_the_backend() {
    let root = share();
    let backend = LocalBackend::open(root.path(), true).await.expect("open");

    let mount = backend.mount_info();
    assert!(!mount.writable);
    assert!(
        mount.reason.contains("mounted read-only"),
        "the reason names the cause: {}",
        mount.reason
    );

    // Every verb, not just the obvious one: reaching any of these is a bug in
    // the window's gating, and defence in depth is cheaper than trusting it.
    assert!(matches!(
        backend
            .write_range("notes.txt", WriteMode::Create, b"no")
            .await,
        Err(EngineError::ReadOnly(_))
    ));
    assert!(matches!(
        backend.mkdir("Shorts").await,
        Err(EngineError::ReadOnly(_))
    ));
    assert!(matches!(
        backend.rename("notes.txt", "other.txt", false).await,
        Err(EngineError::ReadOnly(_))
    ));
    assert!(matches!(
        backend.remove("notes.txt", false).await,
        Err(EngineError::ReadOnly(_))
    ));
    // And nothing happened.
    assert!(root.path().join("notes.txt").is_file());
    assert!(!root.path().join("Shorts").exists());

    // Reading still works, which is the point of a read-only mount.
    assert_eq!(
        backend.read_range("notes.txt", 0, 5).await.expect("read"),
        b"hello"
    );
}

#[tokio::test]
async fn a_root_this_user_cannot_write_to_mounts_read_only_with_a_reason() {
    if is_root() {
        eprintln!("skipping: root can write to a mode-0500 directory");
        return;
    }
    let root = share();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

    let backend = LocalBackend::open(root.path(), false).await.expect("open");
    let mount = backend.mount_info();

    // Probed, not assumed: the caller asked for read-write and the folder said
    // no. Nothing but an attempted write answers that question, because the
    // mode bits, the ACL and the mount flags all have a say.
    assert!(!mount.writable);
    assert!(
        mount.reason.contains("no write permission"),
        "the reason names the cause: {}",
        mount.reason
    );
    assert!(
        mount
            .reason
            .contains(&root.path().canonicalize().unwrap().display().to_string()),
        "and names the folder: {}",
        mount.reason
    );
    // Reading a folder you may not write to still works.
    assert_eq!(backend.list_dir("").await.expect("list").entries.len(), 4);

    // The probe left nothing behind, on the path where it failed or the one
    // where it worked.
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let writable = LocalBackend::open(root.path(), false).await.expect("open");
    assert!(writable.mount_info().writable);
    assert_eq!(writable.list_dir("").await.expect("list").entries.len(), 4);
}

#[tokio::test]
async fn nothing_outside_the_share_is_reachable_through_a_path() {
    let root = share();
    let outside = tempfile::tempdir().expect("temp dir");
    std::fs::write(outside.path().join("secret.txt"), b"not yours").unwrap();
    let backend = LocalBackend::open(root.path(), false).await.expect("open");

    // Refused rather than repaired. A path that climbs out of the share is a
    // bug above this line or an attempt, and normalising it into something
    // that resolves would serve whichever of the two it was.
    let refused = [
        "..",
        "../secret.txt",
        "Documentaries/../../secret.txt",
        ".",
        "./notes.txt",
        // Absolute, in both spellings the OS would accept.
        "/etc/passwd",
        "/",
        // An empty component: a leading, trailing or doubled separator.
        "/notes.txt",
        "notes.txt/",
        "Documentaries//..",
        // A NUL, which the syscall would refuse in its own way and much later.
        "notes\0.txt",
    ];
    for path in refused {
        assert!(
            matches!(backend.list_dir(path).await, Err(EngineError::NotFound(_))),
            "list_dir({path:?}) must be refused"
        );
        assert!(
            matches!(backend.stat(path).await, Err(EngineError::NotFound(_))),
            "stat({path:?}) must be refused"
        );
        assert!(
            matches!(
                backend.read_range(path, 0, 16).await,
                Err(EngineError::NotFound(_))
            ),
            "read_range({path:?}) must be refused"
        );
        assert!(
            matches!(
                backend.write_range(path, WriteMode::Create, b"x").await,
                Err(EngineError::NotFound(_))
            ),
            "write_range({path:?}) must be refused"
        );
        assert!(
            matches!(backend.mkdir(path).await, Err(EngineError::NotFound(_))),
            "mkdir({path:?}) must be refused"
        );
        assert!(
            matches!(
                backend.remove(path, true).await,
                Err(EngineError::NotFound(_))
            ),
            "remove({path:?}) must be refused"
        );
        assert!(
            matches!(
                backend.rename("notes.txt", path, true).await,
                Err(EngineError::NotFound(_))
            ),
            "rename to {path:?} must be refused"
        );
    }

    // Nothing outside was read, written or deleted along the way.
    assert_eq!(
        std::fs::read(outside.path().join("secret.txt")).unwrap(),
        b"not yours"
    );
    assert!(root.path().join("notes.txt").is_file());
}

#[tokio::test]
async fn a_symlink_out_of_the_share_is_listed_as_one_and_can_still_be_opened() {
    let root = share();
    let outside = tempfile::tempdir().expect("temp dir");
    let target = outside.path().join("elsewhere.txt");
    std::fs::write(&target, b"over here").unwrap();
    std::os::unix::fs::symlink(&target, root.path().join("shortcut.txt")).unwrap();

    let backend = LocalBackend::open(root.path(), false).await.expect("open");
    let listing = backend.list_dir("").await.expect("list");
    let link = listing
        .entries
        .iter()
        .find(|entry| entry.name == "shortcut.txt")
        .expect("the link");

    // A link is a link in the listing: the row says so, and says where it
    // points, rather than quietly showing the file at the other end.
    assert_eq!(link.attrs.kind, EntryKind::Symlink);
    assert_eq!(link.kind_label, "Alias");
    assert_eq!(
        link.attrs.link_target.as_deref(),
        Some(target.to_string_lossy().as_ref())
    );

    // And it is followed on open, because this is the user's own machine and
    // Finder follows them. Confinement is about the path model, not a
    // privilege boundary.
    assert_eq!(
        backend
            .read_range("shortcut.txt", 0, 64)
            .await
            .expect("read"),
        b"over here"
    );
    assert_eq!(
        backend.stat("shortcut.txt").await.expect("stat").kind,
        EntryKind::File
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_hides_a_flagged_folder_that_has_no_dot() {
    let root = share();
    std::fs::create_dir(root.path().join("Library")).unwrap();
    let flagged = std::process::Command::new("chflags")
        .arg("hidden")
        .arg(root.path().join("Library"))
        .status()
        .expect("run chflags");
    assert!(flagged.success());

    let backend = LocalBackend::open(root.path(), false).await.expect("open");
    let listing = backend.list_dir("").await.expect("list");
    let library = listing
        .entries
        .iter()
        .find(|entry| entry.name == "Library")
        .expect("the folder");

    // `~/Library` is the one every user meets: `UF_HIDDEN` and no dot. A
    // dotfile-only rule puts it in the middle of the home folder, which Finder
    // does not.
    assert!(library.hidden, "UF_HIDDEN is hidden, dot or no dot");
    assert!(
        !listing
            .entries
            .iter()
            .any(|entry| entry.name == "Documentaries" && entry.hidden),
        "and an unflagged folder is not"
    );
}

#[tokio::test]
async fn the_root_is_resolved_once_so_a_symlinked_root_still_works() {
    let root = share();
    let alias = tempfile::tempdir().expect("temp dir");
    let link = alias.path().join("share");
    std::os::unix::fs::symlink(root.path(), &link).unwrap();

    let backend = LocalBackend::open(&link, false).await.expect("open");
    assert_eq!(backend.list_dir("").await.expect("list").entries.len(), 4);
    assert_eq!(
        backend.root(),
        root.path().canonicalize().unwrap().as_path(),
        "the root is canonical, so every path below it is built from one spelling"
    );

    // A root that is not a directory, or is not there at all, is a failure to
    // open rather than an empty share.
    assert!(LocalBackend::open(&root.path().join("notes.txt"), false)
        .await
        .is_err());
    assert!(LocalBackend::open(Path::new("/no/such/folder"), false)
        .await
        .is_err());
}
