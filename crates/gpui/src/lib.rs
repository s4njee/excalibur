//! Native GPUI application library for Excalibur.
//!
//! The binary is deliberately thin. Keeping the port's models, services and
//! views in a library makes their public contracts testable and prevents the
//! compiler from treating not-yet-wired parity surfaces as dead binary code.

pub mod assets;
pub mod backend_source;
pub mod bookmarks;
pub mod browser;
pub mod column_view;
pub mod connection_dialog;
pub mod connector;
pub mod file_table;
pub mod media_player;
pub mod media_stream;
pub mod model;
pub mod preferences;
pub mod preview;
pub mod properties;
pub mod services;
pub mod settings;
pub mod shell;
pub mod theme;
pub mod transfer_model;
pub mod transfers;
