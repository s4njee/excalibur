//! Small, process-backed media playback used by Quick Look.
//!
//! The GPUI shell intentionally does not decode media itself.  Instead it
//! starts the user's installed `mpv` executable with a carefully constructed
//! argument vector.  This keeps video/audio/image support (including codecs
//! such as H.264, H.265 and AV1) in the media engine that already knows how to
//! play them, while keeping file names and URLs out of a shell command line.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// The kind of media being opened in the Quick Look player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

/// A running `mpv` process.
///
/// `MediaPlayer` owns the child process.  Dropping it always attempts to stop
/// and reap that process, so closing a Quick Look panel cannot leave an orphan
/// media player behind.
pub struct MediaPlayer {
    child: Option<Child>,
}

impl MediaPlayer {
    /// Start `mpv` for one URL or local path.
    ///
    /// The source is appended after `--`, making a path beginning with `-` a
    /// media source rather than another mpv option.  No shell is involved at
    /// any point, so names and URLs remain exactly one argument each.
    ///
    /// # Errors
    ///
    /// Returns an error when the source contains a NUL, `mpv` cannot be found,
    /// or the player process cannot be started.
    pub fn open(source_url: &str, name: &str, kind: MediaKind) -> Result<Self, String> {
        reject_nul("media source", source_url)?;
        reject_nul("media name", name)?;

        let executable = locate_mpv()?;
        let mut command = Command::new(executable);
        command
            .args(build_mpv_args(source_url, name, kind))
            // A media process must not fill a pipe that nobody is draining.
            // `mpv` is intentionally quiet, and null stdio also protects us
            // when a codec or demuxer still emits diagnostics.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = command
            .spawn()
            .map_err(|error| format!("failed to start mpv for Quick Look: {error}"))?;
        Ok(Self { child: Some(child) })
    }

    /// Stop and reap the player.  Calling this more than once is harmless.
    ///
    /// # Errors
    ///
    /// Returns an error when the child process cannot be inspected, stopped,
    /// or reaped.
    pub fn stop(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };

        if child
            .try_wait()
            .map_err(|error| format!("failed to inspect Quick Look player: {error}"))?
            .is_none()
        {
            // A process may exit between try_wait and kill.  In that race,
            // wait() below still reaps it and the operation is successful.
            if let Err(error) = child.kill() {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!("failed to stop Quick Look player: {error}"));
                }
            }
        }

        child
            .wait()
            .map_err(|error| format!("failed to reap Quick Look player: {error}"))?;
        self.child = None;
        Ok(())
    }

    /// Whether the owned process is still running.
    #[must_use]
    pub fn is_running(&mut self) -> bool {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .is_some_and(|status| status.is_none())
    }
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };

        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

/// Locate `mpv` using PATH followed by the standard macOS app locations.
///
/// # Errors
///
/// Returns an error when none of the candidate paths contains an executable
/// `mpv` binary.
pub fn locate_mpv() -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH");
    let candidates = mpv_candidates(path.as_deref());
    candidates
        .iter()
        .find(|candidate| is_executable(candidate))
        .cloned()
        .ok_or_else(|| {
            "Quick Look requires mpv, but no executable was found in PATH or the standard macOS locations (install mpv and try again)".to_owned()
        })
}

/// Build the ordered list searched by [`locate_mpv`].
///
/// This is public so callers and tests can inspect the deterministic search
/// order without changing the process environment.  Existing entries are
/// de-duplicated while preserving their first occurrence.
#[must_use]
pub fn mpv_candidates(path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            let candidate = directory.join("mpv");
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }

    // These paths are harmless on non-macOS platforms and keeping them in the
    // pure candidate list makes the search behavior easy to test everywhere.
    for candidate in [
        "/opt/homebrew/bin/mpv",
        "/usr/local/bin/mpv",
        "/Applications/mpv.app/Contents/MacOS/mpv",
    ]
    .map(PathBuf::from)
    {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// Construct mpv's arguments without invoking a shell.
#[must_use]
pub fn build_mpv_args(source_url: &str, name: &str, kind: MediaKind) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--no-config"),
        OsString::from("--quiet"),
        OsString::from("--terminal=no"),
        OsString::from("--force-window=immediate"),
        OsString::from("--osc=yes"),
        OsString::from("--input-default-bindings=yes"),
        OsString::from("--hwdec=auto-safe"),
        OsString::from("--autofit=90%x90%"),
    ];

    // Keep the window visible for still images after mpv has decoded them.
    // Video and audio use the normal playback lifetime, while all three kinds
    // still get a real window for controls and the on-screen controller.
    if kind == MediaKind::Image {
        args.push(OsString::from("--image-display-duration=inf"));
    }

    args.push(OsString::from(format!("--title=Quick Look — {name}")));
    args.push(OsString::from("--"));
    args.push(OsString::from(source_url));
    args
}

fn reject_nul(label: &str, value: &str) -> Result<(), String> {
    if value.contains('\0') {
        Err(format!(
            "{label} contains a NUL byte and cannot be passed to mpv"
        ))
    } else {
        Ok(())
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_prefer_path_and_deduplicate_entries() {
        let path = std::env::join_paths(["/tmp/mpv-one", "/tmp/mpv-one", "/tmp/mpv-two"])
            .expect("test PATH should be valid");
        let candidates = mpv_candidates(Some(&path));

        assert_eq!(candidates[0], PathBuf::from("/tmp/mpv-one/mpv"));
        assert_eq!(candidates[1], PathBuf::from("/tmp/mpv-two/mpv"));
        assert_eq!(candidates.len(), 5);
    }

    #[test]
    fn arguments_keep_hostile_names_and_urls_as_single_values() {
        let name = "a name; $(touch /tmp/pwned) — [x].mkv";
        let url = "https://example.invalid/a path/file?x=1&y=2";
        let args = build_mpv_args(url, name, MediaKind::Video);

        assert!(args.contains(&OsString::from(format!("--title=Quick Look — {name}"))));
        assert_eq!(args[args.len() - 2], OsString::from("--"));
        assert_eq!(args.last(), Some(&OsString::from(url)));
        assert_eq!(
            args.iter()
                .filter(|arg| arg == &&OsString::from(url))
                .count(),
            1
        );
    }

    #[test]
    fn image_arguments_keep_still_frame_visible() {
        let args = build_mpv_args("/tmp/image.png", "image.png", MediaKind::Image);
        assert!(args.contains(&OsString::from("--image-display-duration=inf")));
    }

    #[test]
    fn media_kinds_are_copyable_and_distinct() {
        let kind = MediaKind::Audio;
        assert_eq!(kind, MediaKind::Audio);
        assert_ne!(kind, MediaKind::Image);
    }
}
