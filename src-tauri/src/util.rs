//! The three helpers that belong to no one module.

/// The local user's name, for an SFTP spec that did not give one.
///
/// `ssh` defaults to the same thing, so a bookmark with no username behaves
/// the way the command line would.
pub(crate) fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_owned())
}

/// An id unique enough to name one transfer for the life of a queue.
pub(crate) fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    format!("{now:x}-{:x}", SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

/// Hand a path to whatever the OS opens it with.
///
/// `open` on macOS, `xdg-open` elsewhere on Unix, `explorer` on Windows. The
/// child is spawned and forgotten: waiting on an editor would block for as
/// long as the user is editing.
pub(crate) fn open_in_editor(path: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    #[cfg(windows)]
    let program = "explorer";
    let _ = std::process::Command::new(program).arg(path).spawn();
}
