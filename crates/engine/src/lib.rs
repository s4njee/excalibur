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

#![forbid(unsafe_code)]

pub mod backend;
pub mod error;
pub mod kind;
pub mod secrets;
pub mod session;

pub use backend::{
    Attrs, Capacity, Caps, ConnectionSpec, Entry, EntryKind, Health, Listing, MountInfo,
    MountStatus, Protocol, ShareBackend, WriteMode,
};
pub use error::{EngineError, Result};
pub use secrets::{KeyringStore, Secret, SecretStore};
pub use session::{Connector, HealthPolicy, SessionEvent, SessionManager};
