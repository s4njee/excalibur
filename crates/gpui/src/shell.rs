//! The native Excalibur window shell.
//!
//! This is intentionally a small, useful first surface: it establishes the
//! client-decorated window chrome and the geometry that browser/model workers
//! can fill in.  The shell owns no filesystem or protocol policy.  Later
//! browser state can replace the sample rows through [`Shell::set_rows`].

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, relative, App, ClickEvent, Context, Div, Entity, FocusHandle, KeyDownEvent,
    SharedString, Stateful, Subscription, Window, WindowAppearance,
};

use crate::bookmarks::{
    builtin_favorites_from_env, BookmarksEvent, BookmarksSidebar, MountSummary,
};
use crate::browser::{parse_location, ConnectionDraft, Location, Modifiers, Sort};
use crate::column_view::{ColumnView, ColumnViewEvent};
use crate::connection_dialog::{ConnectionDialog, ConnectionDialogEvent, TestStatus};
use crate::file_table::{FileTable, FileTableEvent};
use crate::model::BrowserModel;
use crate::preferences::{Preferences, PreferencesEvent};
use crate::preview::{PreviewEvent, PreviewModel, PreviewTarget};
use crate::properties::{Properties, PropertiesEvent};
use crate::settings::{PaneMode, Settings, ThemeChoice};
use crate::theme::{self, Palette};
use crate::transfer_model::TransferModel;
use crate::transfers::{TransfersEvent, TransfersPanel};
use excalibur_engine::backend::{ConnectionSpec, Protocol};
use excalibur_engine::{KnownHosts, SecretStore};

/// A row in the file table.  Kept deliberately protocol-neutral so the model
/// module can map engine `Entry` values without making the shell know a backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub name: String,
    pub size: String,
    pub modified: String,
    pub kind: String,
    pub directory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocationEditState {
    Clean,
    Dirty,
}

impl FileRow {
    pub fn directory(name: impl Into<String>, modified: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            path: name.clone(),
            name,
            size: "—".to_string(),
            modified: modified.into(),
            kind: "Folder".to_string(),
            directory: true,
        }
    }

    pub fn file(
        name: impl Into<String>,
        size: impl Into<String>,
        modified: impl Into<String>,
        kind: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            path: name.clone(),
            name,
            size: size.into(),
            modified: modified.into(),
            kind: kind.into(),
            directory: false,
        }
    }
}

const SAMPLE_ROWS: &[(&str, &str, &str, &str, bool)] = &[
    ("Documents", "—", "31 Aug 2026, 18:24", "Folder", true),
    ("Movies", "—", "30 Aug 2026, 09:12", "Folder", true),
    ("Music", "—", "29 Aug 2026, 21:03", "Folder", true),
    (
        "notes.txt",
        "2 KB",
        "31 Aug 2026, 17:10",
        "SubRip Text",
        false,
    ),
    (
        "session.mkv",
        "48.2 GB",
        "31 Aug 2026, 16:48",
        "Matroska Video",
        false,
    ),
    (
        "transfer.log",
        "84 KB",
        "28 Aug 2026, 11:06",
        "Plain Text",
        false,
    ),
];

fn sample_rows() -> Vec<FileRow> {
    SAMPLE_ROWS
        .iter()
        .map(|(name, size, modified, kind, directory)| FileRow {
            path: (*name).to_string(),
            name: (*name).to_string(),
            size: (*size).to_string(),
            modified: (*modified).to_string(),
            kind: (*kind).to_string(),
            directory: *directory,
        })
        .collect()
}

/// The top-level browser window.
pub struct Shell {
    palette: Palette,
    dark: bool,
    rows: Vec<FileRow>,
    selected: Option<usize>,
    focus: FocusHandle,
    /// Optional engine-backed model. Keeping this as an entity lets the
    /// initial fixture paint immediately while a real listing is loading.
    browser_model: Option<Entity<BrowserModel>>,
    secondary_model: Option<Entity<BrowserModel>>,
    location: Entity<InputState>,
    location_committed: String,
    location_edit: LocationEditState,
    location_error: Option<String>,
    table: Entity<FileTable>,
    secondary_table: Option<Entity<FileTable>>,
    column_view: Option<Entity<ColumnView>>,
    secondary_column_view: Option<Entity<ColumnView>>,
    bookmarks: Entity<BookmarksSidebar>,
    transfers: Entity<TransfersPanel>,
    transfer_model: Option<Entity<TransferModel>>,
    preview: Option<Entity<PreviewModel>>,
    show_transfers: bool,
    show_preview: bool,
    preferences: Option<Entity<Preferences>>,
    properties: Option<Entity<Properties>>,
    settings: Settings,
    settings_error: Option<String>,
    bookmark_specs: Vec<ConnectionSpec>,
    settings_store: crate::settings::SettingsStore,
    active_connection: Option<ConnectionSpec>,
    secondary_connection_id: Option<String>,
    subscriptions: Vec<Subscription>,
    connection_dialog: Option<Entity<ConnectionDialog>>,
    secret_store: Arc<dyn SecretStore>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::from_settings(
            window,
            cx,
            Settings::default(),
            Vec::new(),
            crate::settings::SettingsStore::new("settings.json"),
        )
    }

    fn from_settings(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: Settings,
        bookmark_specs: Vec<ConnectionSpec>,
        settings_store: crate::settings::SettingsStore,
    ) -> Self {
        let dark = resolve_dark(&settings, window);
        let location =
            cx.new(|cx| InputState::new(window, cx).placeholder("smb://host/share or /path"));
        let table = cx.new(|cx| FileTable::new(window, cx));
        let bookmarks = cx.new(|cx| BookmarksSidebar::new(window, cx));
        let transfers = cx.new(|cx| TransfersPanel::new(window, cx));
        let palette = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        table.update(cx, |table, cx| table.set_palette(palette, cx));
        let mut shell = Self {
            palette,
            dark,
            rows: sample_rows(),
            selected: None,
            focus: cx.focus_handle(),
            browser_model: None,
            secondary_model: None,
            location,
            location_committed: String::new(),
            location_edit: LocationEditState::Clean,
            location_error: None,
            table,
            secondary_table: None,
            column_view: None,
            secondary_column_view: None,
            bookmarks,
            transfers,
            transfer_model: None,
            preview: None,
            show_transfers: false,
            show_preview: false,
            preferences: None,
            properties: None,
            settings,
            settings_error: None,
            bookmark_specs,
            settings_store,
            active_connection: None,
            secondary_connection_id: None,
            subscriptions: Vec::new(),
            connection_dialog: None,
            secret_store: Arc::new(excalibur_engine::KeyringStore),
        };
        shell.subscribe_location(window, cx);
        shell
            .subscriptions
            .push(cx.observe_window_appearance(window, |shell, window, cx| {
                shell.apply_palette(window, cx);
            }));
        shell
    }

    /// Construct a shell attached to the native browser model.
    pub fn new_with_model(
        window: &mut Window,
        cx: &mut Context<Self>,
        browser_model: Entity<BrowserModel>,
    ) -> Self {
        let mut shell = Self::new(window, cx);
        let services = browser_model.read(cx).services().clone();
        shell.browser_model = Some(browser_model);
        let palette = shell.palette;
        shell.preview = Some(cx.new(|_| PreviewModel::new(Arc::clone(&services), palette)));
        shell.ensure_secondary_pane(Arc::clone(&services), window, cx);
        shell.ensure_column_views(Arc::clone(&services), cx);
        shell.apply_browser_preferences(cx);
        shell.attach_transfer_model(services, cx);
        shell.observe_model(window, cx);
        shell.sync_table(cx);
        shell.sync_column_views(cx);
        shell.sync_bookmarks(cx);
        shell.subscribe_table(cx);
        shell.subscribe_column_views(cx);
        shell.subscribe_bookmarks(window, cx);
        shell.subscribe_transfers(cx);
        shell.subscribe_preview(cx);
        shell
    }

    /// Construct the shell from the persisted non-secret preferences.
    pub fn new_with_model_and_settings(
        window: &mut Window,
        cx: &mut Context<Self>,
        browser_model: Entity<BrowserModel>,
        settings: Settings,
        bookmark_specs: Vec<ConnectionSpec>,
        settings_store: crate::settings::SettingsStore,
        secret_store: Arc<dyn SecretStore>,
    ) -> Self {
        let mut shell = Self::from_settings(window, cx, settings, bookmark_specs, settings_store);
        let services = browser_model.read(cx).services().clone();
        shell.browser_model = Some(browser_model);
        let palette = shell.palette;
        shell.preview = Some(cx.new(|_| PreviewModel::new(Arc::clone(&services), palette)));
        shell.ensure_secondary_pane(Arc::clone(&services), window, cx);
        shell.ensure_column_views(Arc::clone(&services), cx);
        shell.apply_browser_preferences(cx);
        shell.attach_transfer_model(services, cx);
        shell.secret_store = secret_store;
        shell.observe_model(window, cx);
        shell.sync_table(cx);
        shell.sync_column_views(cx);
        shell.sync_bookmarks(cx);
        shell.subscribe_table(cx);
        shell.subscribe_column_views(cx);
        shell.subscribe_bookmarks(window, cx);
        shell.subscribe_transfers(cx);
        shell.subscribe_preview(cx);
        shell
    }

    fn observe_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = self.browser_model.clone() {
            self.subscriptions
                .push(cx.observe_in(&model, window, |shell, _, window, cx| {
                    shell.sync_table(cx);
                    shell.sync_column_views(cx);
                    shell.sync_bookmarks(cx);
                    shell.sync_location(window, cx);
                    cx.notify();
                }));
        }
        if let (Some(model), Some(table), Some(view)) = (
            self.secondary_model.clone(),
            self.secondary_table.clone(),
            self.secondary_column_view.clone(),
        ) {
            let observed_model = model.clone();
            let observed_table = table.clone();
            let observed_view = view.clone();
            self.subscriptions
                .push(cx.observe_in(&model, window, move |shell, _, _, cx| {
                    shell.sync_table_entity(&observed_model, &observed_table, cx);
                    shell.sync_column_view_entity(&observed_model, &observed_view, cx);
                    cx.notify();
                }));
        }
    }

    fn ensure_secondary_pane(
        &mut self,
        services: Arc<crate::services::Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.secondary_model.is_some() && self.secondary_table.is_some() {
            return;
        }
        let model = cx.new(|_| BrowserModel::new(services));
        let table = cx.new(|cx| FileTable::new(window, cx));
        let palette = self.palette;
        table.update(cx, |table, cx| table.set_palette(palette, cx));
        self.secondary_model = Some(model);
        self.secondary_table = Some(table);
    }

    fn ensure_column_views(
        &mut self,
        services: Arc<crate::services::Services>,
        cx: &mut Context<Self>,
    ) {
        if self.column_view.is_none() {
            let palette = self.palette;
            self.column_view =
                Some(cx.new(|cx| ColumnView::new(Arc::clone(&services), palette, cx)));
        }
        if self.secondary_column_view.is_none() {
            let palette = self.palette;
            self.secondary_column_view = Some(cx.new(|cx| ColumnView::new(services, palette, cx)));
        }
    }

    fn subscribe_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let location = self.location.clone();
        self.subscriptions.push(cx.subscribe_in(
            &location,
            window,
            |shell, input: &Entity<InputState>, event: &InputEvent, window, cx| match event {
                InputEvent::Change => shell.location_edit = LocationEditState::Dirty,
                InputEvent::PressEnter { .. } => {
                    let value = input.read(cx).value().to_string();
                    shell.submit_location(&value, window, cx);
                }
                InputEvent::Blur => {
                    if shell.location_edit == LocationEditState::Dirty {
                        shell.cancel_location(window, cx);
                    }
                }
                InputEvent::Focus => {}
            },
        ));
    }

    fn current_location(&self, cx: &App) -> Option<String> {
        let model = self.browser_model.as_ref()?.read(cx);
        model.mount_info()?;
        let path = model.path();
        let spec = self.active_connection.as_ref()?;
        if spec.protocol == excalibur_engine::backend::Protocol::Local {
            return Some(if path.is_empty() {
                format!("file://{}", spec.share)
            } else {
                format!("file://{}/{}", spec.share.trim_end_matches('/'), path)
            });
        }
        let port = spec
            .port
            .map_or_else(String::new, |port| format!(":{port}"));
        let share = spec.share.trim_matches('/');
        let username = spec
            .username
            .as_deref()
            .filter(|username| !username.is_empty())
            .map_or_else(String::new, |username| format!("{username}@"));
        let suffix = if path.is_empty() {
            share.to_owned()
        } else if share.is_empty() {
            path.to_owned()
        } else {
            format!("{share}/{path}")
        };
        Some(if suffix.is_empty() {
            format!(
                "{}://{}{}{}",
                spec.protocol.scheme(),
                username,
                spec.host,
                port
            )
        } else {
            format!(
                "{}://{}{}{}/{}",
                spec.protocol.scheme(),
                username,
                spec.host,
                port,
                suffix
            )
        })
    }

    fn sync_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.location_edit == LocationEditState::Dirty {
            return;
        }
        let Some(value) = self.current_location(cx) else {
            return;
        };
        if value == self.location_committed {
            return;
        }
        self.location_committed.clone_from(&value);
        self.location.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
    }

    fn cancel_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.location_edit = LocationEditState::Clean;
        self.location_error = None;
        let value = self.location_committed.clone();
        self.location.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
        cx.notify();
    }

    fn submit_location(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let home = dirs::home_dir().and_then(|path| path.to_str().map(str::to_owned));
        let Some(location) = parse_location(value, home.as_deref()) else {
            self.cancel_location(window, cx);
            self.location_error = Some("Location cannot be empty".to_owned());
            cx.notify();
            return;
        };
        match location {
            Location::Navigate { path } => {
                let Some(model) = self.browser_model.clone() else {
                    self.location_error = Some("No browser connection is mounted".to_owned());
                    return;
                };
                let path = match resolve_location_path(&path) {
                    Ok(path) => path,
                    Err(reason) => {
                        self.cancel_location(window, cx);
                        self.location_error = Some(reason);
                        cx.notify();
                        return;
                    }
                };
                self.location_edit = LocationEditState::Clean;
                self.location_error = None;
                let request = model.update(cx, |model, _| model.navigate(&path));
                self.run_listing(request, cx);
                self.sync_location(window, cx);
                cx.notify();
            }
            Location::Connect(draft) => {
                // Submitting a remote URI hands editing to the connection
                // dialog. Restore the last committed location first so a
                // cancelled dialog cannot leave an unmounted URL in the bar.
                self.cancel_location(window, cx);
                if draft.protocol == Protocol::Local {
                    let spec = crate::services::Services::local_spec(draft.share, false);
                    self.active_connection = Some(spec.clone());
                    self.mount(spec, cx);
                    self.sync_location(window, cx);
                    return;
                }
                if let Some(spec) = self.find_saved_location(&draft) {
                    self.active_connection = Some(spec.clone());
                    self.mount(spec, cx);
                    self.sync_location(window, cx);
                    return;
                }
                self.open_connection_dialog(window, cx);
                if let Some(dialog) = self.connection_dialog.clone() {
                    dialog.update(cx, |dialog, cx| dialog.set_draft(draft, window, cx));
                }
            }
            Location::Invalid { reason } => {
                self.cancel_location(window, cx);
                self.location_error = Some(reason);
                cx.notify();
            }
        }
    }

    fn find_saved_location(&self, draft: &ConnectionDraft) -> Option<ConnectionSpec> {
        self.bookmark_specs
            .iter()
            .find(|spec| {
                spec.protocol == draft.protocol
                    && spec.host == draft.host
                    && spec.share.trim_matches('/') == draft.share.trim_matches('/')
                    && location_user_matches(spec.username.as_ref(), draft.username.as_ref())
                    && location_ports_match(spec.protocol, spec.port, draft.port)
            })
            .cloned()
    }

    fn sync_table(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        let table = self.table.clone();
        self.sync_table_entity(&model, &table, cx);
    }

    fn sync_column_views(&self, cx: &mut Context<Self>) {
        let (Some(model), Some(view)) = (self.browser_model.as_ref(), self.column_view.as_ref())
        else {
            return;
        };
        self.sync_column_view_entity(model, view, cx);
        if let (Some(model), Some(view)) = (
            self.secondary_model.as_ref(),
            self.secondary_column_view.as_ref(),
        ) {
            self.sync_column_view_entity(model, view, cx);
        }
    }

    fn sync_column_view_entity(
        &self,
        model: &Entity<BrowserModel>,
        view: &Entity<ColumnView>,
        cx: &mut Context<Self>,
    ) {
        let (path, sort, show_hidden, folders_first, selection) = {
            let model = model.read(cx);
            (
                model.path().to_owned(),
                model.sort(),
                model.show_hidden(),
                model.folders_first(),
                model.browser().selection().paths.clone(),
            )
        };
        view.update(cx, |view, cx| {
            view.set_preferences(sort, show_hidden, folders_first, cx);
            view.set_selection(&selection, cx);
            view.set_path(path, cx);
            view.set_palette(self.palette, cx);
        });
    }

    fn sync_table_entity(
        &self,
        model: &Entity<BrowserModel>,
        table: &Entity<FileTable>,
        cx: &mut Context<Self>,
    ) {
        let (rows, sort, layout) = {
            let model = model.read(cx);
            (model.rows(), model.sort(), model.layout())
        };
        let palette = self.palette;
        table.update(cx, |table, cx| {
            table.set_rows(rows, cx);
            table.set_sort(sort, cx);
            table.set_layout(layout, cx);
            table.set_palette(palette, cx);
        });
    }

    fn apply_browser_preferences(&mut self, cx: &mut Context<Self>) {
        let sort = Sort {
            column: self.settings.sort.column.into(),
            ascending: self.settings.sort.ascending,
        };
        let show_hidden = self.settings.show_hidden;
        let folders_first = self.settings.folders_first;
        let layout: crate::browser::RowLayout = self.settings.row_layout.into();
        if let Some(model) = self.browser_model.clone() {
            model.update(cx, |model, _| {
                model.set_sort(sort);
                model.set_show_hidden(show_hidden);
                model.set_folders_first(folders_first);
                model.set_layout(layout);
            });
        }
        if let Some(model) = self.secondary_model.clone() {
            model.update(cx, |model, _| {
                model.set_sort(sort);
                model.set_show_hidden(show_hidden);
                model.set_folders_first(folders_first);
                model.set_layout(layout);
            });
        }
        self.sync_table(cx);
        if let (Some(model), Some(table)) =
            (self.secondary_model.clone(), self.secondary_table.clone())
        {
            self.sync_table_entity(&model, &table, cx);
        }
        self.sync_column_views(cx);
    }

    fn apply_pane_mode(&mut self, cx: &mut Context<Self>) {
        if self.settings.pane_mode != PaneMode::Dual {
            cx.notify();
            return;
        }
        let Some(model) = self.secondary_model.clone() else {
            cx.notify();
            return;
        };
        if let Some(spec) = self.active_connection.clone() {
            let same_connection = self
                .secondary_connection_id
                .as_deref()
                .is_some_and(|id| id == spec.id);
            let mounted = model.read(cx).mount_info().is_some();
            if !same_connection || !mounted {
                self.secondary_connection_id = Some(spec.id.clone());
                let request = model.update(cx, |model, _| model.mount(spec));
                Self::start_mount_for(model, request, cx);
            }
        }
        cx.notify();
    }

    fn sync_bookmarks(&mut self, cx: &mut Context<Self>) {
        let summary = self
            .active_connection
            .as_ref()
            .zip(self.browser_model.as_ref())
            .and_then(|(spec, model)| {
                model
                    .read(cx)
                    .mount_info()
                    .cloned()
                    .map(|info| MountSummary {
                        id: spec.id.clone(),
                        info: Some(info),
                        reachable: true,
                        reason: None,
                    })
            });
        let summaries = summary.into_iter().collect::<Vec<_>>();
        let active = self
            .active_connection
            .as_ref()
            .filter(|spec| self.bookmark_specs.iter().any(|saved| saved.id == spec.id))
            .map(|spec| spec.id.clone());
        let active_favorite = self.active_connection.as_ref().and_then(|spec| {
            (spec.protocol == Protocol::Local
                && !self.bookmark_specs.iter().any(|saved| saved.id == spec.id))
            .then_some(spec.share.clone())
        });
        let specs = self.bookmark_specs.clone();
        self.bookmarks.update(cx, |bookmarks, cx| {
            bookmarks.set_specs(specs, &summaries, cx);
            bookmarks.set_active(active, cx);
            bookmarks.set_active_favorite(active_favorite, cx);
            bookmarks.set_palette(self.palette, cx);
        });
    }

    fn persist_bookmark(&mut self, spec: ConnectionSpec) {
        if let Some(existing) = self
            .bookmark_specs
            .iter_mut()
            .find(|existing| existing.id == spec.id)
        {
            *existing = spec;
        } else {
            self.bookmark_specs.push(spec);
        }
        let mut document = self.settings_store.load().unwrap_or_default();
        document.bookmarks.clone_from(&self.bookmark_specs);
        if let Err(error) = self.settings_store.save(&document) {
            eprintln!("could not persist Excalibur bookmark: {error}");
        }
    }

    fn remove_bookmark(&mut self, id: &str) {
        self.bookmark_specs.retain(|spec| spec.id != id);
        let mut document = self.settings_store.load().unwrap_or_default();
        document.bookmarks.clone_from(&self.bookmark_specs);
        if let Err(error) = self.settings_store.save(&document) {
            eprintln!("could not persist Excalibur bookmark removal: {error}");
        }
    }

    fn subscribe_bookmarks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.bookmarks.clone();
        self.subscriptions.push(cx.subscribe_in(
            &sidebar,
            window,
            |shell, _, event: &BookmarksEvent, window, cx| {
                match event {
                    BookmarksEvent::Add => shell.open_connection_dialog(window, cx),
                    BookmarksEvent::Select { id } => {
                        shell.active_connection = shell
                            .bookmark_specs
                            .iter()
                            .find(|spec| spec.id == *id)
                            .cloned();
                        shell.sync_bookmarks(cx);
                        cx.notify();
                    }
                    BookmarksEvent::Activate { id } => {
                        if let Some(spec) = shell
                            .bookmark_specs
                            .iter()
                            .find(|spec| spec.id == *id)
                            .cloned()
                        {
                            shell.active_connection = Some(spec.clone());
                            shell.mount(spec, cx);
                        }
                    }
                    BookmarksEvent::ActivateFavorite { path } => {
                        shell.bookmarks.update(cx, |bookmarks, cx| {
                            bookmarks.set_active_favorite(Some(path.clone()), cx);
                        });
                        shell.mount_favorite(path, cx);
                    }
                    BookmarksEvent::Remove { id } => {
                        shell.remove_bookmark(id);
                        let _ = shell.secret_store.delete(id);
                        if shell
                            .active_connection
                            .as_ref()
                            .is_some_and(|spec| spec.id == *id)
                        {
                            shell.active_connection = None;
                        }
                        shell.sync_bookmarks(cx);
                        cx.notify();
                    }
                    BookmarksEvent::Edit { id } => {
                        // Reopen the dialog with non-secret bookmark fields. The
                        // password field is intentionally cleared by set_draft.
                        if let Some(spec) = shell
                            .bookmark_specs
                            .iter()
                            .find(|spec| spec.id == *id)
                            .cloned()
                        {
                            shell.open_connection_dialog(window, cx);
                            if let Some(dialog) = shell.connection_dialog.clone() {
                                dialog.update(cx, |dialog, cx| {
                                    dialog.set_draft(
                                        ConnectionDraft {
                                            protocol: spec.protocol,
                                            host: spec.host.clone(),
                                            port: spec.port,
                                            username: spec.username.clone(),
                                            share: spec.share.clone(),
                                        },
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                    }
                }
            },
        ));
    }

    fn subscribe_transfers(&mut self, cx: &mut Context<Self>) {
        let panel = self.transfers.clone();
        self.subscriptions.push(
            cx.subscribe(&panel, |shell, _, event: &TransfersEvent, cx| {
                shell.show_transfers = true;
                let Some(model) = shell.transfer_model.clone() else {
                    cx.notify();
                    return;
                };
                let event = event.clone();
                let refresh_model = model.clone();
                let task =
                    model.update(cx, |model: &mut TransferModel, _| model.handle_event(event));
                cx.spawn(async move |_, cx| {
                    let _ = task.await;
                    let request =
                        refresh_model.update(cx, |model: &mut TransferModel, _| model.refresh());
                    let token = request.token();
                    if let Ok(snapshot) = request.into_task().await {
                        refresh_model.update(cx, |model, cx| {
                            model.apply_snapshot(token, Ok(snapshot));
                            cx.notify();
                        });
                    }
                })
                .detach();
                cx.notify();
            }),
        );
    }

    fn attach_transfer_model(
        &mut self,
        services: Arc<crate::services::Services>,
        cx: &mut Context<Self>,
    ) {
        let known_hosts = KnownHosts::app_default().unwrap_or_else(|_| {
            KnownHosts::new(transfer_queue_path().with_file_name("known_hosts"))
        });
        let model = cx.new(|_| {
            TransferModel::open(transfer_queue_path(), services).with_native_worker(known_hosts, 2)
        });
        model.update(cx, TransferModel::subscribe);
        self.transfer_model = Some(model.clone());
        self.subscriptions.push(cx.observe(&model, |shell, _, cx| {
            shell.sync_transfers(cx);
            cx.notify();
        }));
        self.refresh_transfers(cx);
        // Drain queued rows without changing rows the user explicitly paused.
        // ResumeAll remains a user action and is never used as a launch hook.
        let startup = model.read(cx).start_worker();
        cx.spawn(async move |_, _| {
            let _ = startup.await;
        })
        .detach();
    }

    fn sync_transfers(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.transfer_model.clone() else {
            return;
        };
        let (transfers, queues) = {
            let model = model.read(cx);
            (model.transfers().to_vec(), model.queues().to_vec())
        };
        let palette = self.palette;
        self.transfers.update(cx, |panel, cx| {
            panel.set_transfers(transfers, cx);
            panel.set_queues(queues, cx);
            panel.set_palette(palette, cx);
        });
    }

    fn refresh_transfers(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.transfer_model.clone() else {
            return;
        };
        let request = model.update(cx, |model: &mut TransferModel, _| model.refresh());
        let token = request.token();
        cx.spawn(async move |_, cx| {
            if let Ok(snapshot) = request.into_task().await {
                model.update(cx, |model, cx| {
                    model.apply_snapshot(token, Ok(snapshot));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn open_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preferences.is_some() {
            return;
        }
        let preferences =
            cx.new(|cx| Preferences::new(window, cx, self.palette, self.settings.clone()));
        self.subscriptions.push(cx.subscribe_in(
            &preferences,
            window,
            |shell, _, event: &PreferencesEvent, window, cx| match event {
                PreferencesEvent::Apply(settings) => {
                    let mut document = shell.settings_store.load().unwrap_or_default();
                    document.settings = settings.clone();
                    if let Err(error) = shell.settings_store.save(&document) {
                        shell.settings_error = Some(format!("Could not save preferences: {error}"));
                        eprintln!("could not persist Excalibur preferences: {error}");
                    } else {
                        shell.settings = settings.clone();
                        shell.settings_error = None;
                        shell.apply_browser_preferences(cx);
                        shell.apply_pane_mode(cx);
                        shell.apply_palette(window, cx);
                        shell.preferences = None;
                    }
                    cx.notify();
                }
                PreferencesEvent::Cancel => {
                    shell.preferences = None;
                    cx.notify();
                }
                PreferencesEvent::Reset => cx.notify(),
            },
        ));
        self.preferences = Some(preferences);
        cx.notify();
    }

    fn apply_palette(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dark = resolve_dark(&self.settings, window);
        self.dark = dark;
        self.palette = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        let palette = self.palette;
        self.table
            .update(cx, |table, cx| table.set_palette(palette, cx));
        if let Some(table) = self.secondary_table.clone() {
            table.update(cx, |table, cx| table.set_palette(palette, cx));
        }
        if let Some(view) = self.column_view.clone() {
            view.update(cx, |view, cx| view.set_palette(palette, cx));
        }
        if let Some(view) = self.secondary_column_view.clone() {
            view.update(cx, |view, cx| view.set_palette(palette, cx));
        }
        self.bookmarks
            .update(cx, |bookmarks, cx| bookmarks.set_palette(palette, cx));
        self.transfers
            .update(cx, |transfers, cx| transfers.set_palette(palette, cx));
        if let Some(preferences) = self.preferences.clone() {
            preferences.update(cx, |preferences, cx| preferences.set_palette(palette, cx));
        }
        if let Some(preview) = self.preview.clone() {
            preview.update(cx, |_, cx| cx.notify());
        }
    }

    fn open_preview(&mut self, cx: &mut Context<Self>) {
        let (Some(model), Some(preview)) = (self.browser_model.clone(), self.preview.clone())
        else {
            return;
        };
        if self.show_preview {
            self.show_preview = false;
            preview.update(cx, PreviewModel::close);
            return;
        }
        let target = {
            let model = model.read(cx);
            let selection = &model.browser().selection().paths;
            if selection.len() != 1 {
                return;
            }
            model
                .rows()
                .into_iter()
                .find(|row| selection.contains(&row.path))
                .map(|row| PreviewTarget::from(&row))
        };
        let Some(target) = target else { return };
        let request = preview.update(cx, |preview, _| preview.open(target));
        let token = request.token();
        self.show_preview = true;
        cx.spawn(async move |_, cx| {
            let result = request.into_task().await;
            preview.update(cx, |preview, cx| {
                preview.apply(token, result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn subscribe_preview(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.clone() else {
            return;
        };
        self.subscriptions.push(
            cx.subscribe(&preview, |shell, _, event: &PreviewEvent, cx| match event {
                PreviewEvent::Close => {
                    shell.show_preview = false;
                    cx.notify();
                }
                PreviewEvent::Download { path } => {
                    // Downloads are owned by the transfer service. There is
                    // no implemented request API yet, so do not claim that a
                    // click started a transfer.
                    eprintln!("download unavailable for preview path {path}");
                }
                PreviewEvent::OpenInEditor { path } => {
                    // An editor launch needs an explicit local materialization
                    // policy. Keep this action observable but side-effect free
                    // until that service operation exists.
                    eprintln!("open in editor unavailable for preview path {path}");
                }
            }),
        );
    }

    fn open_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.properties.is_some() {
            return;
        }
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        let (entries, location, share_name, mount) = {
            let model = model.read(cx);
            let selected = &model.browser().selection().paths;
            let entries = model
                .entries()
                .into_iter()
                .filter(|entry| selected.contains(&entry.path))
                .collect::<Vec<_>>();
            let share_name = self
                .active_connection
                .as_ref()
                .map_or_else(|| "Share".to_owned(), |spec| spec.name.clone());
            (
                entries,
                model.path().to_owned(),
                share_name,
                model.mount_info().cloned(),
            )
        };
        if entries.is_empty() {
            return;
        }
        let properties = cx.new(|cx| {
            Properties::new(
                window,
                cx,
                self.palette,
                entries,
                location,
                share_name,
                mount,
            )
        });
        self.subscriptions.push(cx.subscribe(
            &properties,
            |shell, _, event: &PropertiesEvent, cx| match event {
                PropertiesEvent::Close => {
                    shell.properties = None;
                    cx.notify();
                }
                PropertiesEvent::Reveal { paths } => {
                    eprintln!("reveal unavailable for share paths: {}", paths.join(", "));
                }
                PropertiesEvent::RenameRequested { path, .. } => {
                    eprintln!("rename unavailable for share path: {path}");
                }
            },
        ));
        self.properties = Some(properties);
        cx.notify();
    }

    fn subscribe_table(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        let table = self.table.clone();
        self.subscribe_table_entity(&model, &table, cx);
        if let (Some(model), Some(table)) =
            (self.secondary_model.clone(), self.secondary_table.clone())
        {
            self.subscribe_table_entity(&model, &table, cx);
        }
    }

    fn subscribe_column_views(&mut self, cx: &mut Context<Self>) {
        if let (Some(model), Some(view)) = (self.browser_model.clone(), self.column_view.clone()) {
            self.subscribe_column_view_entity(model, view, cx);
        }
        if let (Some(model), Some(view)) = (
            self.secondary_model.clone(),
            self.secondary_column_view.clone(),
        ) {
            self.subscribe_column_view_entity(model, view, cx);
        }
    }

    fn subscribe_column_view_entity(
        &mut self,
        model: Entity<BrowserModel>,
        view: Entity<ColumnView>,
        cx: &mut Context<Self>,
    ) {
        let subscribed_view = view.clone();
        let callback_view = view;
        self.subscriptions.push(cx.subscribe(
            &subscribed_view,
            move |shell, _, event: &ColumnViewEvent, cx| match event {
                ColumnViewEvent::SelectionChanged { paths, modifiers } => {
                    if let Some(path) = selection_target(
                        &model.read(cx).browser().selection().paths,
                        paths,
                        None,
                        *modifiers,
                    ) {
                        model.update(cx, |model, _| model.click(&path, *modifiers));
                    }
                    cx.notify();
                }
                ColumnViewEvent::Navigate { path } => {
                    let request = model.update(cx, |model, _| model.navigate(path));
                    Self::run_listing_for(model.clone(), request, cx);
                    shell.sync_column_view_entity(&model, &callback_view, cx);
                    cx.notify();
                }
            },
        ));
    }

    fn subscribe_table_entity(
        &mut self,
        model: &Entity<BrowserModel>,
        table: &Entity<FileTable>,
        cx: &mut Context<Self>,
    ) {
        let subscribed_table = table.clone();
        let subscribed_model = model.clone();
        self.subscriptions.push(cx.subscribe(
            table,
            move |shell, _, event: &FileTableEvent, cx| match event {
                FileTableEvent::SelectionChanged {
                    paths,
                    anchor,
                    modifiers,
                } => {
                    // BrowserModel owns the canonical path-keyed selection.
                    // FileTable has already applied range/toggle semantics;
                    // replaying the event's anchor path keeps that state
                    // aligned without introducing a second local selection.
                    let target = {
                        let model_ref = subscribed_model.read(cx);
                        selection_target(
                            &model_ref.browser().selection().paths,
                            paths,
                            anchor.as_deref(),
                            *modifiers,
                        )
                    };
                    if let Some(path) = target {
                        subscribed_model.update(cx, |model, _| model.click(&path, *modifiers));
                    }
                    cx.notify();
                }
                FileTableEvent::Activated { path, directory } => {
                    let request = subscribed_model.update(cx, |model, _| {
                        let index = model.entries().iter().position(|entry| entry.path == *path);
                        if *directory {
                            index.and_then(|index| model.activate(index))
                        } else {
                            None
                        }
                    });
                    if let Some(request) = request {
                        Self::run_listing_for(subscribed_model.clone(), request, cx);
                    }
                }
                FileTableEvent::SortChanged { sort } => {
                    subscribed_model.update(cx, |model, _| model.set_sort(*sort));
                    shell.sync_table_entity(&subscribed_model, &subscribed_table, cx);
                }
            },
        ));
    }

    fn open_connection_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog.is_some() {
            return;
        }
        let store = Arc::clone(&self.secret_store);
        let dialog =
            cx.new(|cx| ConnectionDialog::with_secret_store(window, cx, self.palette, Some(store)));
        self.subscriptions.push(cx.subscribe(
            &dialog,
            |shell, _, event: &ConnectionDialogEvent, cx| match event {
                ConnectionDialogEvent::Cancel => {
                    shell.connection_dialog = None;
                    cx.notify();
                }
                ConnectionDialogEvent::Connect {
                    spec,
                    secret,
                    save_in_keychain,
                } => {
                    if *save_in_keychain {
                        if let Some(secret) = secret {
                            if let Err(error) = shell.secret_store.set(&spec.id, secret) {
                                eprintln!("could not save Excalibur credential: {error}");
                            }
                        }
                    } else if let Err(error) = shell.secret_store.delete(&spec.id) {
                        eprintln!("could not remove Excalibur credential: {error}");
                    }
                    let spec = spec.clone();
                    shell.persist_bookmark(spec.clone());
                    shell.active_connection = Some(spec.clone());
                    shell.sync_bookmarks(cx);
                    shell.connection_dialog = None;
                    shell.mount(spec, cx);
                }
                ConnectionDialogEvent::Test { spec, secret } => {
                    shell.test_connection(spec.clone(), secret.as_ref(), cx);
                }
            },
        ));
        self.connection_dialog = Some(dialog);
        cx.notify();
    }

    fn test_connection(
        &mut self,
        spec: ConnectionSpec,
        secret: Option<&excalibur_engine::Secret>,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        // The connector resolves credentials through the same store as mounts.
        // A test-only password is staged for the probe and removed afterwards.
        let transient_secret = secret.is_some();
        if let Some(secret) = secret {
            if let Err(error) = self.secret_store.set(&spec.id, secret) {
                eprintln!("could not stage Excalibur credential for test: {error}");
            }
        }
        let test_id = spec.id.clone();
        let request = model.update(cx, |model, _| model.test_connection(spec));
        let token = request.token();
        let dialog = self.connection_dialog.clone();
        let store = Arc::clone(&self.secret_store);
        cx.spawn(async move |_, cx| {
            let result = request.into_task().await;
            let status = match &result {
                Ok(value) => {
                    TestStatus::Passed(format!("{} reachable", value.mount.protocol_label))
                }
                Err(error) => TestStatus::Failed(error.to_string()),
            };
            let applied = model.update(cx, |model, cx| {
                let applied = model.apply_connection_test(token, result);
                cx.notify();
                applied
            });
            if transient_secret {
                let _ = store.delete(&test_id);
            }
            if applied {
                if let Some(dialog) = dialog {
                    dialog.update(cx, |dialog, cx| dialog.set_test_status(status, cx));
                }
            }
        })
        .detach();
    }

    fn mount(&mut self, spec: excalibur_engine::backend::ConnectionSpec, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        self.active_connection = Some(spec.clone());
        self.sync_bookmarks(cx);
        let request = model.update(cx, |model, _| model.mount(spec));
        Self::start_mount_for(model, request, cx);
        if self.settings.pane_mode == PaneMode::Dual {
            if let Some(secondary) = self.secondary_model.clone() {
                let secondary_spec = self
                    .active_connection
                    .clone()
                    .expect("active connection set above");
                self.secondary_connection_id = Some(secondary_spec.id.clone());
                let request = secondary.update(cx, |model, _| model.mount(secondary_spec));
                Self::start_mount_for(secondary, request, cx);
            }
        }
    }

    fn start_mount_for(
        model: Entity<BrowserModel>,
        request: crate::model::MountRequest,
        cx: &mut Context<Self>,
    ) {
        let token = request.token();
        cx.spawn(async move |_, cx| {
            let result = request.into_task().await;
            let listing = model.update(cx, |model, cx| {
                let mounted = model.apply_mount(token, result) && model.mount_info().is_some();
                cx.notify();
                mounted.then(|| model.navigate(""))
            });
            let Some(listing) = listing else {
                return;
            };
            let listing_token = listing.token();
            let listing_result = listing.into_task().await;
            model.update(cx, |model, cx| {
                model.apply_listing(listing_token, listing_result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Open a built-in local favorite through the same model mount flow as a
    /// Local connection from the dialog. It never enters `bookmark_specs`, so
    /// the persisted remote bookmark document cannot acquire this convenience
    /// row by accident.
    fn mount_favorite(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(favorite) = builtin_favorites_from_env()
            .into_iter()
            .find(|favorite| favorite.path == path)
        else {
            return;
        };
        let mut spec = crate::services::Services::local_spec(favorite.path, false);
        spec.name = favorite.name;
        self.mount(spec, cx);
    }

    /// Replace the presentation rows when the browser model has a listing.
    pub fn set_rows(&mut self, rows: Vec<FileRow>, cx: &mut Context<Self>) {
        self.rows = rows;
        self.selected = None;
        cx.notify();
    }

    /// Change palette in response to a future appearance command.
    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        if self.dark != dark {
            self.dark = dark;
            self.palette = if dark {
                Palette::dark()
            } else {
                Palette::light()
            };
            cx.notify();
        }
    }

    fn run_listing(&self, request: crate::model::ListingRequest, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        Self::run_listing_for(model, request, cx);
    }

    fn run_listing_for(
        model: Entity<BrowserModel>,
        request: crate::model::ListingRequest,
        cx: &mut Context<Self>,
    ) {
        let token = request.token();
        cx.spawn(async move |_, cx| {
            let result = request.into_task().await;
            model.update(cx, |model, cx| {
                model.apply_listing(token, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        if let Some(request) = model.update(cx, |model, _| model.back()) {
            self.run_listing(request, cx);
            self.sync_location(window, cx);
        }
    }

    fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        if let Some(request) = model.update(cx, |model, _| model.forward()) {
            self.run_listing(request, cx);
            self.sync_location(window, cx);
        }
    }

    fn go_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        if let Some(request) = model.update(cx, |model, _| model.parent()) {
            self.run_listing(request, cx);
            self.sync_location(window, cx);
        }
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.browser_model.clone() else {
            return;
        };
        let request = model.update(cx, |model, _| model.refresh());
        self.run_listing(request, cx);
        self.sync_location(window, cx);
    }

    fn title_bar(&self, cx: &App) -> impl IntoElement {
        let palette = self.palette;
        let title = self
            .browser_model
            .as_ref()
            .and_then(|model| {
                let model = model.read(cx);
                model.mount_info().map(|mount| {
                    if model.path().is_empty() {
                        mount.protocol_label.clone()
                    } else {
                        format!("{} — {}", model.path(), mount.protocol_label)
                    }
                })
            })
            .unwrap_or_else(|| "Share Browser — nas.local".to_owned());
        TitleBar::new()
            .h(px(theme::geometry::TITLE_BAR))
            .flex_none()
            .bg(theme::color(palette.title_bar))
            .border_b_1()
            .border_color(theme::color(palette.chrome_line))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .pl(px(12.))
                    .child(traffic_light(palette))
                    .child(theme::text(
                        "EXCALIBUR",
                        12.,
                        theme::color(palette.text_primary),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(theme::text(
                        title,
                        12.,
                        theme::color(palette.text_secondary),
                    )),
            )
    }

    fn toolbar_state(&self, cx: &App) -> (bool, bool, String, bool, bool) {
        self.browser_model.as_ref().map_or(
            (false, false, "media".to_owned(), true, false),
            |model| {
                let model = model.read(cx);
                (
                    model.browser().history().can_back(),
                    model.browser().history().can_forward(),
                    model.path().to_owned(),
                    model.mount_info().is_some_and(|mount| mount.writable),
                    model.browser().selection().paths.len() == 1,
                )
            },
        )
    }

    fn toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let (can_back, can_forward, path, writable, can_preview) = self.toolbar_state(cx);
        let back = toolbar_button("back", "‹", p, can_back).on_click(cx.listener(
            |this, _, window, cx| {
                this.go_back(window, cx);
            },
        ));
        let forward = toolbar_button("forward", "›", p, can_forward).on_click(cx.listener(
            |this, _, window, cx| {
                this.go_forward(window, cx);
            },
        ));
        div()
            .h(px(theme::geometry::TOOLBAR))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .bg(theme::color(p.toolbar))
            .border_b_1()
            .border_color(theme::color(p.chrome_line))
            .child(back)
            .child(forward)
            .child(
                toolbar_button("up", "↑", p, !path.is_empty()).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.go_parent(window, cx);
                    },
                )),
            )
            .child(
                toolbar_button("new-connection", "New", p, true).on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.open_connection_dialog(window, cx);
                    },
                )),
            )
            .child(
                toolbar_button("transfers", "Transfers", p, true).on_click(cx.listener(
                    |shell, _: &ClickEvent, _, cx| {
                        shell.show_transfers = !shell.show_transfers;
                        cx.notify();
                    },
                )),
            )
            .child(self.location_field(p, cx))
            .child(
                toolbar_button("refresh", "↻", p, true).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.refresh(window, cx);
                    },
                )),
            )
            .child(
                toolbar_button("quick-look", "Quick Look", p, can_preview)
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| shell.open_preview(cx))),
            )
            .child(
                toolbar_button("properties", "Properties", p, can_preview).on_click(cx.listener(
                    |shell, _: &ClickEvent, window, cx| shell.open_properties(window, cx),
                )),
            )
            .child(toolbar_button("edit", "Edit", p, writable))
            .child(toolbar_button("download", "Download", p, true))
            .child(primary_button("upload", "Upload", p, writable))
            .child(
                toolbar_button("preferences", "Preferences", p, true).on_click(cx.listener(
                    |shell, _: &ClickEvent, window, cx| {
                        shell.open_preferences(window, cx);
                    },
                )),
            )
            .child(
                div()
                    .id("filter-field")
                    .w(px(150.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .bg(theme::color(p.field))
                    .border_1()
                    .border_color(theme::color(p.control_border))
                    .rounded(px(5.))
                    .child(theme::text("Filter", 11., theme::color(p.text_disabled))),
            )
    }

    fn location_field(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let border = if self.location_error.is_some() {
            p.danger
        } else {
            p.control_border
        };
        div()
            .id("path-field")
            .h(px(24.))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .px(px(9.))
            .bg(theme::color(p.field))
            .border_1()
            .border_color(theme::color(border))
            .rounded(px(5.))
            .on_key_down(cx.listener(|shell, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    shell.cancel_location(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                Input::new(&self.location)
                    .appearance(false)
                    .small()
                    .text_size(px(10.)),
            )
    }

    /// The model-backed table and its connection/status footer. `FileTable`
    /// owns row virtualization and input semantics; Shell only coordinates
    /// the surrounding browser chrome and status presentation.
    fn file_table(&self, cx: &App) -> Stateful<Div> {
        self.file_table_named("workspace-pane", self.browser_model.as_ref(), cx)
    }

    fn file_table_named(
        &self,
        id: &'static str,
        model: Option<&Entity<BrowserModel>>,
        cx: &App,
    ) -> Stateful<Div> {
        let p = self.palette;
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::color(p.list))
            .child(div().flex_1().min_h_0().child(if model.is_some() {
                if id == "workspace-pane" {
                    self.table.clone()
                } else {
                    self.secondary_table
                        .clone()
                        .unwrap_or_else(|| self.table.clone())
                }
            } else {
                self.table.clone()
            }))
            .child(self.status_bar_for(model, cx))
    }

    fn column_view_named(
        &self,
        id: &'static str,
        model: Option<&Entity<BrowserModel>>,
        view: Option<&Entity<ColumnView>>,
        cx: &App,
    ) -> Stateful<Div> {
        let p = self.palette;
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::color(p.list))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(view.cloned().unwrap_or_else(|| {
                        self.column_view
                            .clone()
                            .expect("column view is initialized with a browser model")
                    })),
            )
            .child(self.status_bar_for(model, cx))
    }

    fn status_snapshot_for(
        &self,
        browser_model: Option<&Entity<BrowserModel>>,
        cx: &App,
    ) -> (
        bool,
        String,
        String,
        String,
        Option<excalibur_engine::backend::Capacity>,
        bool,
        Option<String>,
        usize,
        usize,
    ) {
        browser_model.map_or_else(
            || {
                (
                    !self.dark,
                    String::new(),
                    if self.dark { "NFSv4.2" } else { "SMB3" }.to_owned(),
                    if self.dark { "krb5p" } else { "aes-128-gcm" }.to_owned(),
                    None,
                    false,
                    None,
                    self.rows.len(),
                    self.selected.map_or(0, |_| 1),
                )
            },
            |model| {
                let model = model.read(cx);
                let selected = model.browser().selection().paths.len();
                let (protocol, security, writable, reason, capacity) =
                    model.mount_info().map_or_else(
                        || (String::new(), String::new(), true, String::new(), None),
                        |mount| {
                            (
                                mount.protocol_label.clone(),
                                mount.security_label.clone(),
                                mount.writable,
                                mount.reason.clone(),
                                mount.capacity,
                            )
                        },
                    );
                let (loading, error) = match model.load_state() {
                    crate::model::LoadState::Loading { .. } => (true, None),
                    crate::model::LoadState::Failed { error, .. } => (false, Some(error.clone())),
                    _ => (false, None),
                };
                (
                    writable,
                    reason,
                    protocol,
                    security,
                    capacity,
                    loading,
                    error,
                    model.entries().len(),
                    selected,
                )
            },
        )
    }

    fn status_bar_for(
        &self,
        browser_model: Option<&Entity<BrowserModel>>,
        cx: &App,
    ) -> impl IntoElement {
        let p = self.palette;
        let (writable, reason, protocol, security, capacity, loading, error, items, selected) =
            self.status_snapshot_for(browser_model, cx);
        let error = error
            .or_else(|| self.location_error.clone())
            .or_else(|| self.settings_error.clone());
        let permission = if writable { "RW" } else { "RO" };
        let permission_text = if writable {
            "Read & write on /media".to_owned()
        } else if reason.is_empty() {
            "Read-only on /media".to_owned()
        } else {
            format!("Read-only — {reason}")
        };
        let protocol_line = if protocol.is_empty() {
            String::new()
        } else {
            format!("{protocol} · {security}")
        };
        let capacity_line = capacity.map_or_else(String::new, |capacity| {
            format!(
                "{} free of {}",
                format_size(capacity.free),
                format_size(capacity.total)
            )
        });
        div()
            .h(px(theme::geometry::STATUS_BAR))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .bg(theme::color(if self.dark {
                0x0030_3135
            } else {
                0x00f7_f7f8
            }))
            .border_t_1()
            .border_color(theme::color(p.chrome_line))
            .child(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(3.))
                    .bg(theme::color(if writable { p.accent } else { 0x004a_4b50 }))
                    .child(theme::mono(permission, 9.5, theme::color(0x00ff_ffff))),
            )
            .child(theme::text(
                permission_text,
                11.,
                theme::color(p.text_secondary),
            ))
            .child(div().w(px(1.)).h(px(12.)).bg(theme::color(p.chrome_line)))
            .child(theme::text(
                if let Some(error) = error {
                    error
                } else if loading {
                    "Loading…".to_owned()
                } else {
                    format!("{items} items · {selected} selected")
                },
                11.,
                theme::color(p.text_secondary),
            ))
            .child(div().w(px(1.)).h(px(12.)).bg(theme::color(p.chrome_line)))
            .child(theme::mono(
                capacity_line,
                10.,
                theme::color(p.text_secondary),
            ))
            .child(div().flex_1())
            .child(theme::mono(
                protocol_line,
                10.,
                theme::color(p.text_secondary),
            ))
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(theme::color(p.success)),
            )
    }

    fn browser_content(&self, cx: &App) -> Stateful<Div> {
        if self.show_transfers {
            return div()
                .id("browser-content")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(self.transfers.clone());
        }

        if self.settings.pane_mode != PaneMode::Dual {
            let column_view = self.browser_model.as_ref().is_some_and(|model| {
                model.read(cx).layout() == crate::browser::RowLayout::ColumnView
            });
            return div()
                .id("browser-content")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(if column_view {
                    self.column_view_named(
                        "workspace-column-view",
                        self.browser_model.as_ref(),
                        self.column_view.as_ref(),
                        cx,
                    )
                } else {
                    self.file_table(cx)
                });
        }

        let split = self.settings.split_ratio.clamp(0.2, 0.8);
        let left_column_view = self
            .browser_model
            .as_ref()
            .is_some_and(|model| model.read(cx).layout() == crate::browser::RowLayout::ColumnView);
        let right_column_view = self
            .secondary_model
            .as_ref()
            .is_some_and(|model| model.read(cx).layout() == crate::browser::RowLayout::ColumnView);
        let left = div()
            .w(relative(split))
            .min_w_0()
            .h_full()
            .child(if left_column_view {
                self.column_view_named(
                    "workspace-pane-column-view",
                    self.browser_model.as_ref(),
                    self.column_view.as_ref(),
                    cx,
                )
            } else {
                self.file_table_named("workspace-pane", self.browser_model.as_ref(), cx)
            });
        let mut right = div().w(relative(1. - split)).min_w_0().h_full();
        if let Some(model) = self.secondary_model.as_ref() {
            right = right.child(if right_column_view {
                self.column_view_named(
                    "workspace-pane-secondary-column-view",
                    Some(model),
                    self.secondary_column_view.as_ref(),
                    cx,
                )
            } else {
                self.file_table_named("workspace-pane-secondary", Some(model), cx)
            });
        } else {
            right = right.child(theme::text(
                "Second pane unavailable",
                11.,
                theme::color(self.palette.text_disabled),
            ));
        }
        div()
            .id("browser-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .child(left)
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .bg(theme::color(self.palette.chrome_line)),
            )
            .child(right)
    }
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    // A filesystem size is a 64-bit integer while the compact display is a
    // floating-point value; precision beyond a tenth of a unit is irrelevant.
    #[allow(clippy::cast_precision_loss)]
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Recover the row endpoint from a table selection event so the canonical
/// `BrowserModel` can apply the same path-keyed operation. Toggle events need a
/// diff against the previous model selection (the clicked row may have just
/// been removed); shift events use the visible range endpoint away from the
/// anchor.
fn selection_target(
    current: &std::collections::HashSet<String>,
    paths: &[String],
    anchor: Option<&str>,
    modifiers: Modifiers,
) -> Option<String> {
    if modifiers.toggle {
        paths
            .iter()
            .find(|path| !current.contains(path.as_str()))
            .or_else(|| {
                current
                    .iter()
                    .find(|path| !paths.iter().any(|candidate| candidate == *path))
            })
            .cloned()
    } else if modifiers.shift {
        paths
            .iter()
            .rev()
            .find(|path| anchor != Some(path.as_str()))
            .cloned()
            .or_else(|| paths.first().cloned())
    } else {
        paths.first().cloned()
    }
}

fn transfer_queue_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Excalibur")
        .join("transfers.json")
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings_error = self.settings_error.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|shell, event: &KeyDownEvent, _, cx| {
                let toggles_preview = event.keystroke.key == "space"
                    || (event.keystroke.key == "escape" && shell.show_preview);
                if toggles_preview {
                    cx.stop_propagation();
                    shell.open_preview(cx);
                }
            }))
            .font_family(theme::SANS)
            .text_color(theme::color(self.palette.text_primary))
            .child(self.title_bar(cx))
            .child(self.toolbar(cx))
            .child(
                div()
                    .id("browser-body")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .id("sidebar")
                            .w(px(f32::from(
                                u16::try_from(self.settings.sidebar_width).unwrap_or(u16::MAX),
                            )))
                            .h_full()
                            .flex_none()
                            .border_r_1()
                            .border_color(theme::color(self.palette.chrome_line))
                            .child(self.bookmarks.clone()),
                    )
                    .child(self.browser_content(cx)),
            )
            .children(self.connection_dialog.clone())
            .children(self.preferences.clone())
            .children(self.properties.clone())
            .when(self.show_preview, |window| {
                window.children(self.preview.clone())
            })
            .when(settings_error.is_some(), move |window| {
                window.child(
                    div()
                        .id("settings-error-toast")
                        .absolute()
                        .top(px(8.))
                        .right(px(12.))
                        .max_w(px(460.))
                        .px(px(10.))
                        .py(px(6.))
                        .bg(theme::color(0x00a8_3434))
                        .border_1()
                        .border_color(theme::color(0x00df_7777))
                        .rounded(px(4.))
                        .child(theme::text(
                            settings_error.unwrap_or_default(),
                            11.,
                            theme::color(0x00ff_ffff),
                        )),
                )
            })
    }
}

/// Resolve the loose spelling accepted by the location field into a safe
/// share-relative path. A leading slash, duplicate separators, and dot
/// segments are presentation details; traversal above the share root is
/// clamped at the root, matching the browser's path-field contract.
fn resolve_location_path(input: &str) -> Result<String, String> {
    if input.contains('\0') {
        return Err("A location cannot contain a NUL character".to_owned());
    }
    let mut components = Vec::new();
    for component in input.trim().split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            components.pop();
            continue;
        }
        components.push(component);
    }
    Ok(components.join("/"))
}

fn location_ports_match(protocol: Protocol, saved: Option<u16>, typed: Option<u16>) -> bool {
    let default = match protocol {
        Protocol::Smb => 445,
        Protocol::Nfs => 2049,
        Protocol::Sftp | Protocol::Xsync => 22,
        Protocol::Local => 0,
    };
    saved.unwrap_or(default) == typed.unwrap_or(default)
}

fn location_user_matches(saved: Option<&String>, typed: Option<&String>) -> bool {
    typed.is_none_or(|typed| saved == Some(typed))
}

fn resolve_dark(settings: &Settings, window: &Window) -> bool {
    match settings.theme {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => true,
            WindowAppearance::Light | WindowAppearance::VibrantLight => false,
        },
    }
}

fn traffic_light(p: Palette) -> Div {
    div()
        .flex()
        .gap(px(5.))
        .children((0..3).map(move |index| {
            div()
                .id(SharedString::from(format!("traffic-{index}")))
                .size(px(9.))
                .rounded_full()
                .bg(theme::color(if index == 0 {
                    0x00c8_c8cc
                } else if index == 1 {
                    0x00bd_bdc2
                } else {
                    0x00b2_b2b8
                }))
        }))
        .text_color(theme::color(p.text_primary))
}

fn toolbar_button(
    id: &'static str,
    label: &'static str,
    p: Palette,
    enabled: bool,
) -> Stateful<Div> {
    let foreground = if enabled {
        p.text_primary
    } else {
        p.text_disabled
    };
    div()
        .id(id)
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(if label.len() < 2 { 9. } else { 8. }))
        .rounded(px(4.))
        .border_1()
        .border_color(theme::color(p.control_border))
        .bg(theme::color(p.control))
        .cursor_pointer()
        .hover(|style| style.bg(theme::color(p.accent_soft)))
        .when(!enabled, |button| {
            button.bg(theme::color(if p == Palette::dark() {
                0x002e_2f32
            } else {
                p.control
            }))
        })
        .child(theme::text(label, 11.5, theme::color(foreground)))
}

fn primary_button(
    id: &'static str,
    label: &'static str,
    p: Palette,
    enabled: bool,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(10.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme::color(if enabled {
            p.accent
        } else {
            p.control_border
        }))
        .bg(theme::color(if enabled { p.accent } else { 0x002e_2f32 }))
        .cursor_pointer()
        .child(theme::text(
            label,
            11.5,
            theme::color(if enabled {
                0x00ff_ffff
            } else {
                p.text_disabled
            }),
        ))
}

#[cfg(test)]
mod tests {
    use super::{location_ports_match, location_user_matches, resolve_location_path};
    use excalibur_engine::backend::Protocol;

    #[test]
    fn location_paths_are_share_relative_and_root_clamped() {
        assert_eq!(
            resolve_location_path("/docs/./reports//").expect("valid path"),
            "docs/reports"
        );
        assert_eq!(
            resolve_location_path("docs/reports/../").expect("valid path"),
            "docs"
        );
        assert_eq!(
            resolve_location_path("../../etc").expect("valid path"),
            "etc"
        );
    }

    #[test]
    fn location_paths_reject_nul() {
        assert!(resolve_location_path("docs/\0/private").is_err());
    }

    #[test]
    fn location_matching_accepts_omitted_default_port() {
        assert!(location_ports_match(Protocol::Smb, Some(445), None));
        assert!(location_ports_match(Protocol::Sftp, None, Some(22)));
        assert!(!location_ports_match(Protocol::Smb, Some(8445), None));
    }

    #[test]
    fn location_matching_allows_omitted_username_but_not_wrong_username() {
        let saved = Some("alice".to_owned());
        assert!(location_user_matches(saved.as_ref(), None));
        assert!(location_user_matches(saved.as_ref(), saved.as_ref()));
        let other = "bob".to_owned();
        assert!(!location_user_matches(saved.as_ref(), Some(&other)));
    }
}
