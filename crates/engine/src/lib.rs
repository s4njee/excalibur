//! The Excalibur engine: everything the app does that is not drawing.
//!
//! Two rules hold this apart from the shell, and both are load-bearing:
//!
//! - **No Tauri dependency.** Every backend and the transfer worker are
//!   testable headlessly, against a mock or a container, without a window.
//! - **File bytes never cross IPC.** The shell asks for paths and metadata;
//!   this crate does the reading and writing.
//!
//! The single most important type here is [`MountInfo::writable`]. The design
//! this implements gates every write affordance on one derived value, taken
//! from the protocol rather than from what the user asked for, so a UI can
//! never offer a write that the mount would refuse.

// `deny`, not `forbid`: the workspace's presumption, kept at the crate level so
// it is visible here, but overridable by the one module that has to write FFI.
// `drag::macos` allows it for itself (E24-S2), with a `// SAFETY:` note per
// block and a tested safe wrapper above it; `forbid` cannot be overridden that
// way, and holding it would mean shipping no drag at all rather than deciding
// anything about one. See plan.md "Decisions taken", `unsafe` code.
#![deny(unsafe_code)]

pub mod backend;
pub mod drag;
pub mod edit;
pub mod error;
pub mod hostkeys;
pub mod kind;
pub mod log;
pub mod secrets;
pub mod session;
pub mod transfer;

pub use backend::{
    Attrs, Capacity, Caps, ConnectionSpec, Entry, EntryKind, Health, Listing, MountInfo,
    MountStatus, Protocol, ShareBackend, WriteMode,
};
pub use edit::{EditSession, EditStore, SaveOutcome};
pub use error::{EngineError, Result};
pub use hostkeys::{HostKey, KnownHosts, Trust};
pub use log::{LogAction, LogCategory, LogLine, MessageLog, Resolution};
pub use secrets::{KeyringStore, Secret, SecretStore};
pub use session::{Connector, HealthPolicy, SessionEvent, SessionManager};
pub use transfer::{Direction, NamedQueue, Status as TransferStatus, Transfer};
