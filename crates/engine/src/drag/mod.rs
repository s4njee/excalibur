//! Native drag and drop: between our own windows (E24-S2), and out to Finder
//! as file promises (E24-S1).
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
//! Beside it, one `NSFilePromiseProvider` per item — a claim that a file *will*
//! exist, with no bytes behind it yet. One drag therefore ends in either place:
//! another Excalibur window reads the private type and never touches disk, and
//! Finder sees only the promises and asks for them one at a time.
//!
//! **Nothing is fetched until somebody asks.** That is the whole point of a
//! promise, and it is a property of the shape rather than a check: a drag
//! cancelled over the Desktop produces no [`PromiseRequest`], so there is no
//! path through this module on which a cancelled drag could queue anything.
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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::EntryKind;
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

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

/// One item offered to Finder as a file that does not exist yet (E24-S1).
///
/// A promise is the only honest way to drag something off a share. The
/// alternative — writing the file out first and dragging the copy — fetches
/// every byte of a selection the user may be about to drop back where it came
/// from, and on a 40 GB film that is not a slower version of the same feature,
/// it is a different one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Promise {
    /// The share-relative path this promises, so fulfilling it can name a source.
    pub path: String,
    /// What the file will be called where it lands.
    pub name: String,
    /// The Uniform Type Identifier it is offered under, from the kind table.
    pub uti: String,
}

/// The promises one selection makes, in the order the list had them.
///
/// `folders` is the subset of `selection.paths` that are directories, rather
/// than a parallel array of flags: the two travel from the webview over IPC and
/// a flag array that arrived one element short would silently promise a folder
/// as a file — which Finder accepts and then writes an empty *file* for, so the
/// tree never appears and nothing reports an error. A set of paths cannot
/// desynchronise from the paths it names.
#[must_use]
pub fn promises_for(selection: &Selection, folders: &[String]) -> Vec<Promise> {
    selection
        .paths
        .iter()
        .map(|path| {
            let is_dir = folders.iter().any(|folder| folder == path);
            let name = promised_name(path);
            let kind = classify_entry(
                &name,
                if is_dir {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            );
            Promise {
                path: path.clone(),
                name,
                uti: kind.category.uti().to_owned(),
            }
        })
        .collect()
}

/// The base name a promised path will land under.
///
/// Trailing slashes are trimmed first: a folder path that arrived as `Films/`
/// would otherwise promise a file called `""`, which `AppKit` turns into an
/// item Finder cannot name and drops on the floor without saying so.
fn promised_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if base.is_empty() {
        // The share root, which the list has no row for. Named rather than
        // empty, because an unnamed promise fails invisibly.
        "Untitled".to_owned()
    } else {
        base.to_owned()
    }
}

/// Finder asking for one promised item, at last.
///
/// This arrives **only** when something accepted the drop and wants the bytes.
/// A drag released over the Desktop and cancelled, or dropped on an app that
/// takes the private type instead, never produces one — which is what makes
/// "nothing is fetched until Finder asks" a property of the design rather than
/// a check somebody has to remember to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromiseRequest {
    /// The mount and window the drag came from.
    pub selection: Selection,
    /// The share-relative path being asked for.
    pub path: String,
    /// The full path Finder wants the item written to, name and all.
    ///
    /// The name is Finder's, not ours: a second `Arrival.mkv` on a Desktop that
    /// already has one arrives as `Arrival 2.mkv`, and a fulfilment that wrote
    /// the name it promised instead would overwrite the file already there.
    pub destination: PathBuf,
}

impl PromiseRequest {
    /// The folder Finder named — where the transfer is queued to.
    ///
    /// A destination with no parent cannot happen from `AppKit`, which always
    /// hands over a file inside a directory; `/` is returned rather than
    /// `Option` so no caller has to invent a behaviour for a case the OS does
    /// not produce.
    #[must_use]
    pub fn folder(&self) -> &Path {
        self.destination.parent().unwrap_or_else(|| Path::new("/"))
    }
}

/// What fulfils one promise: queue the transfer, and block until it lands.
///
/// **Blocking is the contract, not an accident.** `AppKit` calls this on the
/// operation queue the drag vends — never the main thread — and the promise is
/// not complete until the function returns, which is exactly how Finder knows
/// when to stop showing the progress it started. An implementation that queued
/// the transfer and returned immediately would tell Finder the file was there
/// before a byte of it was.
pub type PromiseSink = Arc<dyn Fn(&PromiseRequest) -> Result<()> + Send + Sync>;

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
/// `promises` is one [`Promise`] per item, and the pasteboard carries one file
/// promise for each of them *beside* [`SELECTION_TYPE`] — so the same drag can
/// end in another Excalibur window, which reads the private type and moves
/// nothing to disk, or in Finder, which sees only the promises. `fulfil` is
/// called once per item, and only if something asks; see [`PromiseSink`].
///
/// The drop targets installed by [`install_drop_target`] are removed
/// automatically when the session ends, however it ends.
///
/// # Errors
/// If the platform has no implementation, this is not the main thread, the view
/// is not in a window, or the image cannot be decoded.
pub fn start_drag(
    ns_view: *mut c_void,
    selection: &Selection,
    promises: &[Promise],
    image_png: &[u8],
    fulfil: &PromiseSink,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::start_drag(ns_view, selection, promises, image_png, fulfil)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ns_view, selection, promises, image_png, fulfil);
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
    use super::{
        promises_for, Modifiers, Path, PathBuf, Phase, PromiseRequest, Selection, SELECTION_TYPE,
    };

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
    fn every_item_is_promised_once_in_the_order_the_list_had_them() {
        // One promise per item is the AC, and the order matters because the
        // drag image's stack and the pasteboard's items are read positionally
        // by whatever receives them.
        let promises = promises_for(&selection(), &["Films/Dune".to_owned()]);
        assert_eq!(promises.len(), 2);
        assert_eq!(promises[0].path, "Films/Arrival.mkv");
        assert_eq!(promises[0].name, "Arrival.mkv");
        assert_eq!(promises[1].path, "Films/Dune");
        assert_eq!(promises[1].name, "Dune");
    }

    #[test]
    fn a_folder_is_promised_as_a_folder_and_a_file_by_its_extension() {
        // The failure this prevents: a folder promised by its *name* has no
        // extension, so the kind table calls it `public.data`, Finder accepts
        // the drag and writes an empty file, and the tree never arrives. That
        // is why `promises_for` is told which paths are directories rather
        // than guessing from the string.
        let promises = promises_for(&selection(), &["Films/Dune".to_owned()]);
        assert_eq!(promises[0].uti, "public.movie");
        assert_eq!(promises[1].uti, "public.folder");

        // And the same path with nothing said about it is a file.
        let as_file = promises_for(&selection(), &[]);
        assert_eq!(as_file[1].uti, "public.data");
    }

    #[test]
    fn a_trailing_slash_does_not_promise_a_nameless_file() {
        let selection = Selection {
            mount: "xsync:raid6:media".to_owned(),
            paths: vec!["Films/Dune/".to_owned(), String::new()].clone(),
            window: "main".to_owned(),
        };
        let promises = promises_for(&selection, &["Films/Dune/".to_owned()]);
        assert_eq!(promises[0].name, "Dune");
        // The share root has no row in the list, so this cannot arrive from the
        // app — but an empty name reaches AppKit as an item Finder silently
        // discards, and a named one is at least visible when it is wrong.
        assert_eq!(promises[1].name, "Untitled");
    }

    #[test]
    fn a_request_names_the_folder_finder_chose_and_not_the_file() {
        // The transfer is queued to the *folder*: this is the conversion, and
        // getting it wrong queues a copy into a path that is a file.
        let request = PromiseRequest {
            selection: selection(),
            path: "Films/Arrival.mkv".to_owned(),
            destination: PathBuf::from("/Users/sanjee/Desktop/Arrival 2.mkv"),
        };
        assert_eq!(request.folder(), Path::new("/Users/sanjee/Desktop"));
        // And the name Finder de-duplicated is kept, because writing the name
        // we promised instead would overwrite the file already on the Desktop.
        assert_eq!(
            request
                .destination
                .file_name()
                .and_then(|name| name.to_str()),
            Some("Arrival 2.mkv"),
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
