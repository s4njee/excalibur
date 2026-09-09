//! The `invoke()` surface, one module per thing a window can ask for.
//!
//! Every function marked `#[tauri::command]` below is named by string from
//! TypeScript, so its name is an interface and renaming one breaks the app
//! with no compile error. `lib.rs` lists them all in `generate_handler!`.

pub(crate) mod edits;
pub(crate) mod fs;
pub(crate) mod mounts;
pub(crate) mod secrets;
pub(crate) mod transfers;
pub(crate) mod windows;
