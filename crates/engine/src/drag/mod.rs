//! Native drag and drop between our own windows (E24-S2).
//!
//! **Why this is native at all.** Tauri's `tauri://drag-*` events carry OS file
//! paths and nothing else, and a webview cannot start a drag that Finder or a
//! second webview can receive. A selection on a share has no OS path — that is
//! the whole point of the app — so a drag out of a list has to put something
//! else on the pasteboard, and only `AppKit` can do that. Decided in plan.md's
//! "Decisions taken", *Drag and drop*.
//!
//! **What crosses the boundary.** One private pasteboard type,
//! [`SELECTION_TYPE`], carrying [`Selection`] as JSON: which mount the items
//! came from, their share-relative paths, and which window started the drag.
//! E24-S1 will add `NSFilePromiseProvider`s *beside* this type on the same
//! pasteboard, so a single drag can end in Finder or in another window; the
//! name and the payload are fixed here so that story does not have to change
//! them.
//!
//! **What this module is.** The safe wrapper the rest of the tree sees. All the
//! `unsafe` is in [`macos`], one module with `#![allow(unsafe_code)]` and a
//! `// SAFETY:` note per block, per the workspace's rule. Everything here is
//! safe Rust, and the two pure parts — the payload's encoding and the
//! phase/modifier vocabulary — are testable without a window.
//!
//! **Threading.** `AppKit`'s dragging protocols are `MainThreadOnly`. Every
//! function below refuses off the main thread rather than doing something
//! undefined, so the shell has to say `run_on_main_thread` out loud.

#[cfg(target_os = "macos")]
pub mod macos;

use std::ffi::c_void;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};

/// The private pasteboard type an Excalibur drag carries.
///
/// A reverse-DNS name under a domain this project owns, because a pasteboard
/// type is a global namespace shared with every other running app. E24-S1 adds
/// file promises alongside it; it never replaces it.
pub const SELECTION_TYPE: &str = "dev.excalibur.selection";

/// What one drag is carrying.
///
/// Paths are share-relative in the same shape `navigation.ts` uses — no leading
/// slash, `""` for the root — because that is what every backend verb takes.
/// They are meaningless without `mount`, which is why the two travel together
/// rather than the destination assuming the drag came from wherever it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    /// The mount id the paths belong to (a bookmark id, e.g. `smb:nas:media`).
    pub mount: String,
    /// Share-relative paths, in the order the list had them.
    pub paths: Vec<String>,
    /// The Tauri label of the window the drag started in.
    ///
    /// Carried so a drop back into the *same* folder of the *same* window can
    /// be recognised as the no-op it is, and so the source window can be told
    /// to refresh after a move.
    pub window: String,
}

impl Selection {
    /// The JSON that goes on the pasteboard.
    ///
    /// Infallible by construction: every field is a `String` or a `Vec<String>`,
    /// which `serde_json` cannot fail to serialise. An empty string on a
    /// pasteboard would be indistinguishable from a drag carrying nothing, so
    /// the impossible branch is spelled out rather than silently `unwrap_or`d.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| String::from("{}"))
    }

    /// Read a payload back, or `None` if it is not one of ours.
    ///
    /// A pasteboard is written by whatever app is dragging. Anything under our
    /// own type name is still *someone else's* bytes until it parses, so this
    /// returns an `Option` rather than defaulting fields into existence.
    #[must_use]
    pub fn from_json(raw: &str) -> Option<Self> {
        let parsed: Self = serde_json::from_str(raw).ok()?;
        // A payload with no mount cannot be acted on: every verb the drop
        // resolves to needs one. Refusing here means the frontend never has to
        // ask whether the string it got is empty.
        if parsed.mount.is_empty() || parsed.paths.is_empty() {
            return None;
        }
        Some(parsed)
    }
}

/// Where a drag is in its life over one window.
///
/// The same three phases E13-S5's `onFileDrop` already has, so the webview
/// hit-tests an Excalibur drag with the code it hit-tests a Finder drag with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// The pointer is inside, and has not been released.
    Over,
    /// Released here.
    Drop,
    /// Left without being released.
    Leave,
}

/// The modifier keys held at the moment of the event.
///
/// Only the four that mean something to a file manager. `alt` is the one this
/// story acts on — it forces a copy — but the drop ring reads them all, and a
/// struct that grows a field later is cheaper than a bare bool that has to be
/// replaced everywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
// Four `bool`s rather than a bitflag: these are read one at a time by name,
// and `modifiers.alt` at the call site is the whole point of the struct.
#[allow(clippy::struct_excessive_bools)]
pub struct Modifiers {
    /// ⌥ — forces a copy.
    pub alt: bool,
    pub shift: bool,
    /// ⌘.
    pub command: bool,
    pub control: bool,
}

/// One dragging message, in the terms the webview needs.
// No `Eq`: the coordinates are `f64`. Comparing two of these for equality is
// something only a test does, and it does it on the fields it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropEvent {
    pub phase: Phase,
    /// Pointer position in **physical** pixels from the window's top-left.
    ///
    /// Physical and top-left because that is what Tauri reports for a Finder
    /// drag, and `backend.ts` already converts exactly once for that. Two
    /// coordinate conventions for two kinds of drag would be a factor-of-two
    /// error on a Retina display — the highlight on the wrong row and the files
    /// in the wrong folder — which is the bug E13-S5's Results already record.
    pub x: f64,
    pub y: f64,
    pub modifiers: Modifiers,
    /// The payload, when the drag is one of ours and parsed. `None` on `leave`.
    pub selection: Option<Selection>,
}

/// Where a drop event is delivered: the window's label, and the event.
///
/// An `Arc` rather than a `Box` because one sink serves every window's drop
/// target — they are installed together, for the duration of one drag — and the
/// alternative is a clone of the shell's emit closure per window.
pub type DropSink = Arc<dyn Fn(&str, DropEvent) + Send + Sync>;

/// Put a drop target over one window, for the duration of a drag.
///
/// **Why "for the duration" and not once at startup.** The window's content
/// view is *below* the `WKWebView`, and `AppKit` finds a drag's destination by
/// hit-testing from the top down — so a permanently installed target under the
/// webview would never see a drag, and one installed over it would swallow
/// every click. Targets therefore exist only while an Excalibur drag is in
/// flight, which is a period during which the webview receives no mouse events
/// anyway because `AppKit` owns the tracking loop.
///
/// The consequence worth knowing: a drag from **Finder** never meets one of
/// these, so E13-S5's `tauri://drag-*` path is untouched and OS file drops are
/// handled in exactly one place.
///
/// `ns_view` must be the window's content view, as `tauri::Window::ns_view()`
/// returns it. Must be called on the main thread.
///
/// # Errors
/// If the platform has no implementation, or this is not the main thread.
pub fn install_drop_target(label: &str, ns_view: *mut c_void, sink: DropSink) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::install_drop_target(label, ns_view, sink)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (label, ns_view, sink);
        Err(unsupported())
    }
}

/// Take every drop target down.
///
/// Called when a drag ends, and defensively before a new one starts: a target
/// left over from a drag whose end was missed would be an invisible view
/// swallowing clicks, which is the worst way for this to fail.
pub fn remove_drop_targets() {
    #[cfg(target_os = "macos")]
    macos::remove_drop_targets();
}

/// Begin a native drag of `selection` from `ns_view`'s window.
///
/// `image_png` is the drag image the webview rendered — **bytes, never a
/// path**. A dragged item that is not local yet has no file to point at, and
/// writing one out to name it would be a fetch the user did not ask for.
///
/// The drop targets installed by [`install_drop_target`] are removed
/// automatically when the session ends, however it ends.
///
/// # Errors
/// If the platform has no implementation, this is not the main thread, the view
/// is not in a window, or the image cannot be decoded.
pub fn start_drag(ns_view: *mut c_void, selection: &Selection, image_png: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::start_drag(ns_view, selection, image_png)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ns_view, selection, image_png);
        Err(unsupported())
    }
}

/// The one refusal every non-macOS path gives, in the same words.
#[cfg(not(target_os = "macos"))]
fn unsupported() -> EngineError {
    EngineError::Unsupported("dragging between windows is macOS-only until E24-S4".to_owned())
}

/// Kept in scope on macOS too, so the `use` above is not platform-conditional.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn unsupported() -> EngineError {
    EngineError::Unsupported("dragging between windows is macOS-only until E24-S4".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Modifiers, Phase, Selection, SELECTION_TYPE};

    fn selection() -> Selection {
        Selection {
            mount: "smb:nas.local:media".to_owned(),
            paths: vec!["Films/Arrival.mkv".to_owned(), "Films/Dune".to_owned()],
            window: "browser-2".to_owned(),
        }
    }

    #[test]
    fn the_type_name_is_the_one_e24_s1_will_add_promises_beside() {
        // Pinned as a literal rather than derived: a pasteboard type is a
        // global name, and the drag out to Finder has to write the *same* one
        // or a drag will work in one direction only.
        assert_eq!(SELECTION_TYPE, "dev.excalibur.selection");
    }

    #[test]
    fn a_payload_survives_the_pasteboard_round_trip() {
        let json = selection().to_json();
        assert_eq!(Selection::from_json(&json), Some(selection()));
    }

    #[test]
    fn the_json_names_its_fields_the_way_the_webview_reads_them() {
        // The frontend parses this string, so the key names are part of the
        // contract and not an implementation detail of serde's defaults.
        let json = selection().to_json();
        assert!(json.contains("\"mount\":\"smb:nas.local:media\""), "{json}");
        assert!(json.contains("\"window\":\"browser-2\""), "{json}");
        assert!(json.contains("\"paths\":["), "{json}");
    }

    #[test]
    fn anything_that_is_not_ours_is_not_a_selection() {
        // Under our own type name, but written by something else. The failure
        // this prevents is a drop acting on a mount id of `""`, which every
        // backend lookup would then answer for with the *app's* recent mount.
        assert_eq!(Selection::from_json("not json"), None);
        assert_eq!(Selection::from_json("{}"), None);
        assert_eq!(
            Selection::from_json(r#"{"mount":"","paths":["a"],"window":"main"}"#),
            None,
        );
    }

    #[test]
    fn a_payload_with_no_paths_is_refused_rather_than_dropped_as_nothing() {
        // A drag of zero items would otherwise reach the drop ring and offer
        // "Move 0 items", which is a sentence about nothing.
        assert_eq!(
            Selection::from_json(r#"{"mount":"local::/tmp","paths":[],"window":"main"}"#),
            None,
        );
    }

    #[test]
    fn phases_and_modifiers_serialise_as_the_webview_spells_them() {
        assert_eq!(serde_json::to_string(&Phase::Over).unwrap(), "\"over\"");
        assert_eq!(serde_json::to_string(&Phase::Drop).unwrap(), "\"drop\"");
        assert_eq!(serde_json::to_string(&Phase::Leave).unwrap(), "\"leave\"");
        let flags = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(
            serde_json::to_string(&flags).unwrap(),
            r#"{"alt":true,"shift":false,"command":false,"control":false}"#,
        );
    }
}
