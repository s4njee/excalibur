//! Durable, non-secret settings for the GPUI shell.
//!
//! The Tauri frontend stores bookmarks and preferences in two keyed stores.
//! The native shell keeps one small, versioned JSON document instead. Only
//! [`excalibur_engine::ConnectionSpec`] is persisted for a connection: it contains
//! the identity and endpoint, while passwords and other credentials remain in
//! the engine's keychain boundary and cannot be represented here.
//!
//! The file is user-editable and survives upgrades. Serde's defaults make
//! missing fields harmless and unknown fields are ignored. Syntax/type errors
//! are reported with the path and leave the existing file untouched. Saves use
//! a temporary file in the destination directory, [`sync_all`], and an atomic
//! rename so a crash cannot leave a half-written document at the final path.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::browser::{RowLayout, SortColumn};
use excalibur_engine::ConnectionSpec;

/// The persisted theme choice, including following the operating system.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// One or two browser panes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaneMode {
    #[default]
    Single,
    Dual,
}

/// Serde-friendly persisted representation of the browser's sort choice.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SortPreference {
    pub column: SortColumnPreference,
    pub ascending: bool,
}

impl Default for SortPreference {
    fn default() -> Self {
        Self {
            column: SortColumnPreference::Name,
            ascending: true,
        }
    }
}

/// A serialised sort column. Kept separate from browser state so the browser
/// can continue to use an engine-facing enum without adding serde concerns to
/// its interaction API.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortColumnPreference {
    #[default]
    Name,
    Size,
    Added,
    Modified,
    Kind,
}

impl From<SortColumn> for SortColumnPreference {
    fn from(column: SortColumn) -> Self {
        match column {
            SortColumn::Name => Self::Name,
            SortColumn::Size => Self::Size,
            SortColumn::Added => Self::Added,
            SortColumn::Modified => Self::Modified,
            SortColumn::Kind => Self::Kind,
        }
    }
}

impl From<SortColumnPreference> for SortColumn {
    fn from(column: SortColumnPreference) -> Self {
        match column {
            SortColumnPreference::Name => Self::Name,
            SortColumnPreference::Size => Self::Size,
            SortColumnPreference::Added => Self::Added,
            SortColumnPreference::Modified => Self::Modified,
            SortColumnPreference::Kind => Self::Kind,
        }
    }
}

/// Remembered native window dimensions. Values are advisory; the shell still
/// clamps them against the platform's usable display area on startup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

/// The preferences needed to reconstruct the browser's view.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub pane_mode: PaneMode,
    pub row_layout: RowLayoutPreference,
    pub sort: SortPreference,
    pub show_hidden: bool,
    pub folders_first: bool,
    pub sidebar_width: u32,
    pub split_ratio: f32,
    pub window_size: Option<WindowSize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            pane_mode: PaneMode::Single,
            row_layout: RowLayoutPreference::Columns,
            sort: SortPreference::default(),
            show_hidden: true,
            folders_first: true,
            sidebar_width: 206,
            split_ratio: 0.5,
            window_size: None,
        }
    }
}

/// Serde representation of the five row layouts in the View menu.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowLayoutPreference {
    #[default]
    Columns,
    NameLed,
    Mono,
    Icons,
    ColumnView,
}

impl From<RowLayout> for RowLayoutPreference {
    fn from(layout: RowLayout) -> Self {
        match layout {
            RowLayout::Columns => Self::Columns,
            RowLayout::NameLed => Self::NameLed,
            RowLayout::Mono => Self::Mono,
            RowLayout::Icons => Self::Icons,
            RowLayout::ColumnView => Self::ColumnView,
        }
    }
}

impl From<RowLayoutPreference> for RowLayout {
    fn from(layout: RowLayoutPreference) -> Self {
        match layout {
            RowLayoutPreference::Columns => Self::Columns,
            RowLayoutPreference::NameLed => Self::NameLed,
            RowLayoutPreference::Mono => Self::Mono,
            RowLayoutPreference::Icons => Self::Icons,
            RowLayoutPreference::ColumnView => Self::ColumnView,
        }
    }
}

/// The complete JSON document. Runtime properties such as mounted/reachable
/// are deliberately absent: they describe the current process, not a saved
/// connection, and stale values would make the sidebar lie after restart.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsDocument {
    pub schema_version: u32,
    pub bookmarks: Vec<ConnectionSpec>,
    pub settings: Settings,
}

/// An error reading or atomically writing the settings file.
#[derive(Debug)]
pub enum SettingsError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Encode(serde_json::Error),
    Decode {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} settings {}: {source}",
                path.display()
            ),
            Self::Encode(source) => write!(formatter, "could not encode settings: {source}"),
            Self::Decode { path, source } => write!(
                formatter,
                "could not read settings {}: invalid JSON ({source})",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Encode(source) | Self::Decode { source, .. } => Some(source),
        }
    }
}

/// A JSON settings file at a caller-selected path.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// Open a store. Construction does no I/O, which makes it safe to create
    /// before the GPUI app has finished initialising its directories.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the document, using defaults when this is the first launch.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the file cannot be read or decoded.
    pub fn load(&self) -> Result<SettingsDocument, SettingsError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SettingsDocument::default())
            }
            Err(source) => {
                return Err(SettingsError::Io {
                    operation: "read",
                    path: self.path.clone(),
                    source,
                })
            }
        };
        serde_json::from_slice(&bytes).map_err(|source| SettingsError::Decode {
            path: self.path.clone(),
            source,
        })
    }

    /// Save by replacing the final path only after the complete temporary file
    /// has been written and synced.
    ///
    /// # Errors
    ///
    /// Returns a structured error when the directory, temporary file, encode,
    /// sync, or replacement operation fails.
    pub fn save(&self, document: &SettingsDocument) -> Result<(), SettingsError> {
        let bytes = serde_json::to_vec_pretty(document).map_err(SettingsError::Encode)?;
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| SettingsError::Io {
            operation: "create settings directory",
            path: parent.to_owned(),
            source,
        })?;

        let temporary = temporary_path(&self.path);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|source| SettingsError::Io {
                    operation: "create temporary settings file",
                    path: temporary.clone(),
                    source,
                })?;
            file.write_all(&bytes).map_err(|source| SettingsError::Io {
                operation: "write temporary settings file",
                path: temporary.clone(),
                source,
            })?;
            file.sync_all().map_err(|source| SettingsError::Io {
                operation: "sync temporary settings file",
                path: temporary.clone(),
                source,
            })?;
            drop(file);
            fs::rename(&temporary, &self.path).map_err(|source| SettingsError::Io {
                operation: "replace settings file",
                path: self.path.clone(),
                source,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp-{}-{stamp}", std::process::id()));
    PathBuf::from(temporary)
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionSpec, PaneMode, RowLayoutPreference, Settings, SettingsDocument, SettingsStore,
        SortColumnPreference, SortPreference, ThemeChoice, WindowSize,
    };
    use excalibur_engine::backend::{Access, Protocol};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempSettingsDir(PathBuf);

    impl TempSettingsDir {
        fn new(label: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "excalibur-gpui-settings-{label}-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("temporary settings directory");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }

    impl Drop for TempSettingsDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_path(label: &str) -> (TempSettingsDir, PathBuf) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = TempSettingsDir::new(&format!("{label}-{stamp}"));
        let path = directory.file();
        (directory, path)
    }

    fn bookmark() -> ConnectionSpec {
        ConnectionSpec {
            id: "xsync:nas.local:media".to_owned(),
            name: "media".to_owned(),
            protocol: Protocol::Xsync,
            host: "nas.local".to_owned(),
            port: None,
            share: "media".to_owned(),
            username: Some("alice".to_owned()),
            domain: None,
            mount_preference: Access::Rw,
        }
    }

    #[test]
    fn missing_file_loads_safe_defaults() {
        let (_directory, path) = temp_path("defaults");
        let loaded = SettingsStore::new(&path).load().expect("defaults");
        assert_eq!(loaded, SettingsDocument::default());
    }

    #[test]
    fn round_trip_preserves_bookmarks_and_view_preferences() {
        let (_directory, path) = temp_path("round-trip");
        let store = SettingsStore::new(&path);
        let document = SettingsDocument {
            schema_version: 2,
            bookmarks: vec![bookmark()],
            settings: Settings {
                theme: ThemeChoice::Dark,
                pane_mode: PaneMode::Dual,
                row_layout: RowLayoutPreference::Mono,
                sort: SortPreference {
                    column: SortColumnPreference::Modified,
                    ascending: false,
                },
                show_hidden: false,
                folders_first: false,
                sidebar_width: 240,
                split_ratio: 0.6,
                window_size: Some(WindowSize {
                    width: 1100,
                    height: 720,
                }),
            },
        };
        store.save(&document).expect("save");
        assert_eq!(store.load().expect("load"), document);
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_nested_settings_default() {
        let (_directory, path) = temp_path("forward-compatible");
        fs::write(
            &path,
            br#"{"schemaVersion":9,"newField":{"future":true},"settings":{"showHidden":false,"futurePreference":"x"}}"#,
        )
        .expect("fixture");
        let loaded = SettingsStore::new(&path)
            .load()
            .expect("forward compatible");
        assert_eq!(loaded.schema_version, 9);
        assert!(!loaded.settings.show_hidden);
        assert_eq!(loaded.settings.pane_mode, PaneMode::Single);
        assert_eq!(loaded.settings.row_layout, RowLayoutPreference::Columns);
    }

    #[test]
    fn corrupt_file_returns_pathful_error_and_leaves_bytes_untouched() {
        let (_directory, path) = temp_path("corrupt");
        fs::write(&path, b"not json").expect("fixture");
        let store = SettingsStore::new(&path);
        let error = store.load().expect_err("corrupt must be reported");
        assert!(error.to_string().contains("invalid JSON"));
        assert!(error.to_string().contains(path.to_string_lossy().as_ref()));
        assert_eq!(fs::read(&path).expect("original"), b"not json");
    }

    #[test]
    fn serialised_bookmarks_contain_no_secret_field() {
        let encoded = serde_json::to_string(&SettingsDocument {
            bookmarks: vec![bookmark()],
            ..SettingsDocument::default()
        })
        .expect("encode");
        assert!(!encoded.to_ascii_lowercase().contains("password"));
        assert!(!encoded.to_ascii_lowercase().contains("secret"));
        assert!(encoded.contains("nas.local"));
    }
}
