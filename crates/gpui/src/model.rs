//! GPUI-facing browser orchestration.
//!
//! [`crate::browser::BrowserState`] remains the pure, synchronous state for a
//! pane (history, filtering, sorting and path selection). This type adds the
//! service futures around it: every request receives a generation token, and a
//! late result is ignored. The model owns names and metadata only; file bytes
//! stay behind the engine service boundary.

use std::sync::Arc;

use excalibur_engine::backend::{ConnectionSpec, Entry, EntryKind, Listing, MountInfo};
use excalibur_engine::{EngineError, Result};

use crate::browser::{self, BrowserState, Modifiers, Sort, SortColumn};
use crate::services::{ConnectionTestResult, ServiceTask, Services};

/// A monotonically increasing identity for one mount/list operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestToken(u64);

impl RequestToken {
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.0
    }
}

/// What the list pane can render while an async operation is in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading { token: RequestToken },
    Ready,
    Failed { token: RequestToken, error: String },
}

/// State of the non-mounting connection test used by the New Connection
/// dialog. A spec is retained only in `Ready`, after the connector succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Idle,
    Testing {
        token: RequestToken,
    },
    Ready {
        token: RequestToken,
        spec: Box<ConnectionSpec>,
        mount: MountInfo,
    },
    Failed {
        token: RequestToken,
        error: String,
    },
}

/// A metadata-only projection for callers that want rows independent of the
/// pure browser state. It intentionally contains no file contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryRow {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub kind_label: String,
    pub kind_category: excalibur_engine::kind::Category,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub added_ms: Option<i64>,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
    pub hidden: bool,
}

impl From<Entry> for EntryRow {
    fn from(entry: Entry) -> Self {
        Self {
            name: entry.name,
            path: entry.path,
            kind: entry.attrs.kind,
            kind_label: entry.kind_label,
            kind_category: entry.kind_category,
            size: entry.attrs.size,
            modified_ms: entry.attrs.modified_ms,
            added_ms: entry.attrs.added_ms,
            mode: entry.attrs.mode,
            link_target: entry.attrs.link_target,
            hidden: entry.hidden,
        }
    }
}

/// A started mount operation. GPUI owns the continuation and applies it with
/// [`BrowserModel::apply_mount`].
pub struct MountRequest {
    token: RequestToken,
    task: ServiceTask<MountInfo>,
}

impl MountRequest {
    #[must_use]
    pub const fn token(&self) -> RequestToken {
        self.token
    }

    #[must_use]
    pub fn into_task(self) -> ServiceTask<MountInfo> {
        self.task
    }
}

/// A started listing operation. Keeping the task outside the model lets the
/// shell use its preferred GPUI continuation style.
pub struct ListingRequest {
    token: RequestToken,
    path: String,
    task: ServiceTask<Listing>,
}

/// A started connection test. The safe spec remains inside the task until a
/// successful result is applied, so callers cannot accidentally bookmark an
/// unverified connection.
pub struct ConnectionTestRequest {
    token: RequestToken,
    task: ServiceTask<ConnectionTestResult>,
}

impl ConnectionTestRequest {
    #[must_use]
    pub const fn token(&self) -> RequestToken {
        self.token
    }

    #[must_use]
    pub fn into_task(self) -> ServiceTask<ConnectionTestResult> {
        self.task
    }
}

impl ListingRequest {
    #[must_use]
    pub const fn token(&self) -> RequestToken {
        self.token
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn into_task(self) -> ServiceTask<Listing> {
        self.task
    }
}

/// Browser state plus async service orchestration for one native window.
pub struct BrowserModel {
    services: Arc<Services>,
    browser: BrowserState,
    mount: Option<MountInfo>,
    next_token: u64,
    load: LoadState,
    next_connection_token: u64,
    connection: ConnectionState,
}

pub type AppModel = BrowserModel;

impl BrowserModel {
    #[must_use]
    pub fn new(services: Arc<Services>) -> Self {
        Self {
            services,
            browser: BrowserState::new(""),
            mount: None,
            next_token: 0,
            load: LoadState::Idle,
            next_connection_token: 0,
            connection: ConnectionState::Idle,
        }
    }

    #[must_use]
    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    #[must_use]
    pub fn browser(&self) -> &BrowserState {
        &self.browser
    }

    #[must_use]
    pub fn mount_info(&self) -> Option<&MountInfo> {
        self.mount.as_ref()
    }

    #[must_use]
    pub fn path(&self) -> &str {
        self.browser.path()
    }

    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.browser.entries()
    }

    #[must_use]
    pub fn raw_entries(&self) -> &[Entry] {
        self.browser.raw_entries()
    }

    #[must_use]
    pub fn rows(&self) -> Vec<EntryRow> {
        self.entries().into_iter().map(EntryRow::from).collect()
    }

    #[must_use]
    pub const fn load_state(&self) -> &LoadState {
        &self.load
    }

    #[must_use]
    pub const fn state(&self) -> &LoadState {
        &self.load
    }

    #[must_use]
    pub const fn connection_state(&self) -> &ConnectionState {
        &self.connection
    }

    #[must_use]
    pub const fn sort(&self) -> Sort {
        self.browser.sort()
    }

    /// Apply the hidden-file preference to the pane without replacing its
    /// listing or selection.
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        self.browser.set_show_hidden(show_hidden);
    }

    #[must_use]
    pub const fn show_hidden(&self) -> bool {
        self.browser.show_hidden()
    }

    /// Apply the folders-first preference to the pane. Ordering is derived
    /// when rows are requested, so this is safe while a listing is visible.
    pub fn set_folders_first(&mut self, folders_first: bool) {
        self.browser.set_folders_first(folders_first);
    }

    #[must_use]
    pub const fn folders_first(&self) -> bool {
        self.browser.folders_first()
    }

    /// Select one of the persisted row presentations without touching the
    /// engine-facing listing or path-keyed selection.
    pub fn set_layout(&mut self, layout: browser::RowLayout) {
        self.browser.set_layout(layout);
    }

    #[must_use]
    pub const fn layout(&self) -> browser::RowLayout {
        self.browser.layout()
    }

    /// Apply the table's canonical sort state to the pure browser state.
    pub fn set_sort(&mut self, sort: Sort) {
        while self.browser.sort() != sort {
            self.browser.toggle_sort(sort.column);
        }
    }

    pub fn toggle_sort(&mut self, column: SortColumn) {
        self.browser.toggle_sort(column);
    }

    /// The spec is bookmark-ready only after a successful probe.
    #[must_use]
    pub fn successful_connection(&self) -> Option<&ConnectionSpec> {
        match &self.connection {
            ConnectionState::Ready { spec, .. } => Some(spec),
            _ => None,
        }
    }

    /// Start a mount and invalidate every listing started before it.
    pub fn mount(&mut self, spec: ConnectionSpec) -> MountRequest {
        let token = self.bump();
        self.browser = BrowserState::new("");
        self.mount = None;
        self.load = LoadState::Loading { token };
        MountRequest {
            token,
            task: self.services.mount(spec),
        }
    }

    /// Start loading `path`. Paths are share-relative and use `/`; malformed
    /// paths fail locally without asking a backend to normalise traversal.
    pub fn navigate(&mut self, path: &str) -> ListingRequest {
        let token = self.bump();
        let path = match normalise_path(path) {
            Ok(path) => path,
            Err(error) => {
                self.load = LoadState::Failed {
                    token,
                    error: error.to_string(),
                };
                return ListingRequest {
                    token,
                    path: self.path().to_owned(),
                    task: Box::pin(async move { Err(error) }),
                };
            }
        };
        self.browser.navigate(path.clone());
        self.load = LoadState::Loading { token };
        ListingRequest {
            token,
            path: path.clone(),
            task: self.services.list(path),
        }
    }

    /// Start a non-mounting connection probe.
    pub fn test_connection(&mut self, spec: ConnectionSpec) -> ConnectionTestRequest {
        self.next_connection_token = self.next_connection_token.wrapping_add(1).max(1);
        let token = RequestToken(self.next_connection_token);
        self.connection = ConnectionState::Testing { token };
        ConnectionTestRequest {
            token,
            task: self.services.test_connection(spec),
        }
    }

    /// Apply a test result if it still belongs to the current dialog request.
    /// Returns false for stale work and true when the state was updated.
    pub fn apply_connection_test(
        &mut self,
        token: RequestToken,
        result: Result<ConnectionTestResult>,
    ) -> bool {
        if !matches!(
            self.connection,
            ConnectionState::Testing { token: current } if current == token
        ) {
            return false;
        }
        self.connection = match result {
            Ok(result) => ConnectionState::Ready {
                token,
                spec: Box::new(result.spec),
                mount: result.mount,
            },
            Err(error) => ConnectionState::Failed {
                token,
                error: error.to_string(),
            },
        };
        true
    }

    pub fn refresh(&mut self) -> ListingRequest {
        let path = self.path().to_owned();
        self.navigate(&path)
    }

    pub fn parent(&mut self) -> Option<ListingRequest> {
        browser::can_ascend(self.path()).then(|| self.navigate(&browser::parent(self.path())))
    }

    pub fn back(&mut self) -> Option<ListingRequest> {
        if !self.browser.history().can_back() {
            return None;
        }
        self.browser.back();
        Some(self.begin_current_path())
    }

    pub fn forward(&mut self) -> Option<ListingRequest> {
        if !self.browser.history().can_forward() {
            return None;
        }
        self.browser.forward();
        Some(self.begin_current_path())
    }

    fn begin_current_path(&mut self) -> ListingRequest {
        let token = self.bump();
        self.load = LoadState::Loading { token };
        let path = self.path().to_owned();
        ListingRequest {
            token,
            path: path.clone(),
            task: self.services.list(path),
        }
    }

    pub fn apply_mount(&mut self, token: RequestToken, result: Result<MountInfo>) -> bool {
        if !self.is_current(token) {
            return false;
        }
        match result {
            Ok(info) => {
                self.mount = Some(info);
                self.load = LoadState::Ready;
            }
            Err(error) => self.fail(token, &error),
        }
        true
    }

    pub fn apply_listing(&mut self, token: RequestToken, result: Result<Listing>) -> bool {
        if !self.is_current(token) {
            return false;
        }
        match result {
            Ok(listing) => {
                self.mount = Some(listing.mount.clone());
                self.browser.set_listing(listing);
                self.load = LoadState::Ready;
            }
            Err(error) => self.fail(token, &error),
        }
        true
    }

    fn fail(&mut self, token: RequestToken, error: &EngineError) {
        self.load = LoadState::Failed {
            token,
            error: error.to_string(),
        };
    }

    pub fn click(&mut self, path: &str, modifiers: Modifiers) {
        self.browser.click(path, modifiers);
    }

    pub fn select_next(&mut self) -> Option<usize> {
        self.browser.step(1)
    }

    pub fn select_previous(&mut self) -> Option<usize> {
        self.browser.step(-1)
    }

    /// Activate the selected visible row. Files remain selected for the shell
    /// to preview; directories produce a new listing request.
    pub fn activate(&mut self, index: usize) -> Option<ListingRequest> {
        let entries = self.entries();
        let entry = entries.get(index)?;
        self.browser.click(&entry.path, Modifiers::default());
        (entry.attrs.kind == EntryKind::Dir).then(|| self.navigate(&entry.path))
    }

    pub fn cancel(&mut self) {
        self.bump();
        self.load = LoadState::Idle;
    }

    fn bump(&mut self) -> RequestToken {
        self.next_token = self.next_token.wrapping_add(1).max(1);
        RequestToken(self.next_token)
    }

    fn is_current(&self, token: RequestToken) -> bool {
        matches!(self.load, LoadState::Loading { token: current } if current == token)
    }
}

fn normalise_path(path: &str) -> Result<String> {
    if path.is_empty() {
        return Ok(String::new());
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(EngineError::NotFound(format!(
                "{path} is not a share-relative directory"
            )));
        }
        components.push(component);
    }
    Ok(components.join("/"))
}

#[cfg(test)]
mod tests {
    use super::{BrowserModel, ConnectionState, LoadState, Sort, SortColumn};
    use crate::browser::RowLayout;
    use crate::services::Services;
    use excalibur_engine::EngineError;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn fixture_mount_and_navigation_keep_bytes_out_of_model() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        let mount = model.mount(Services::mock_spec());
        let token = mount.token();
        assert!(model.apply_mount(token, mount.into_task().await));
        let request = model.navigate("");
        assert!(model.apply_listing(request.token(), request.into_task().await));
        assert_eq!(model.load_state(), &LoadState::Ready);
        assert!(model
            .raw_entries()
            .iter()
            .any(|entry| entry.name == "Series"));
        assert!(model.rows().iter().all(|row| !row.name.is_empty()));
    }

    #[test]
    fn stale_failure_cannot_replace_new_navigation() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        let old = model.navigate("old");
        let new = model.navigate("new");
        assert!(!model.apply_listing(
            old.token(),
            Err(EngineError::Disconnected("old request".to_owned()))
        ));
        assert!(
            matches!(model.load_state(), LoadState::Loading { token } if *token == new.token())
        );
    }

    #[test]
    fn stale_connection_failure_cannot_replace_new_test() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        let old = model.test_connection(Services::mock_spec());
        let new = model.test_connection(Services::mock_spec());
        assert!(!model.apply_connection_test(
            old.token(),
            Err(EngineError::Disconnected("old test".to_owned()))
        ));
        assert!(matches!(
            model.connection_state(),
            ConnectionState::Testing { token } if *token == new.token()
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn successful_connection_exposes_spec_only_after_test() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        assert!(model.successful_connection().is_none());
        let request = model.test_connection(Services::mock_spec());
        let token = request.token();
        let result = request.into_task().await.expect("connection test");
        assert!(model.apply_connection_test(token, Ok(result)));
        assert_eq!(model.successful_connection().unwrap().id, "mock:media");
    }

    #[test]
    fn navigation_uses_browser_history_and_rejects_traversal() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        let first = model.navigate("Series/Season-1");
        assert_eq!(first.path(), "Series/Season-1");
        let parent = model.parent().expect("parent");
        assert_eq!(parent.path(), "Series");
        let _ = model.navigate("../outside");
        assert!(matches!(model.load_state(), LoadState::Failed { .. }));
    }

    #[test]
    fn table_sort_changes_are_applied_to_canonical_browser_state() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        let sort = Sort {
            column: SortColumn::Size,
            ascending: false,
        };
        model.set_sort(sort);
        assert_eq!(model.sort(), sort);
        model.toggle_sort(SortColumn::Size);
        assert_ne!(model.sort(), sort);
    }

    #[test]
    fn presentation_preferences_are_exposed_without_replacing_the_model() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = BrowserModel::new(services);
        model.set_show_hidden(false);
        model.set_folders_first(false);
        model.set_layout(RowLayout::ColumnView);
        assert!(!model.show_hidden());
        assert!(!model.folders_first());
        assert_eq!(model.layout(), RowLayout::ColumnView);
        assert_eq!(model.path(), "");
        assert!(model.mount_info().is_none());
    }
}
