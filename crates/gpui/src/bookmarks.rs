//! Native sidebar for saved connections and live mount summaries.
//!
//! A [`BookmarkItem`] owns only a saved [`ConnectionSpec`] and current mount
//! facts. Credentials are absent by construction: passwords remain at the
//! engine keychain boundary. The sidebar emits intent; the shell/model owns
//! mounting, persistence, and removal.

use std::sync::Arc;
use std::{path::Path, path::PathBuf};

use excalibur_engine::backend::{ConnectionSpec, MountInfo, MountStatus};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, img, px, uniform_list, AnyElement, App, ClickEvent, Context, Div, EventEmitter,
    FocusHandle, FontWeight, MouseButton, MouseDownEvent, SharedString, Stateful,
    UniformListScrollHandle, Window,
};

use crate::theme::{self, Palette};

const SECTION_HEADER: f32 = 28.;
const ROW_HEIGHT: f32 = 38.;
const FOOTER_HEIGHT: f32 = 30.;
const FAVORITE_ICON_SIZE: f32 = 28.;

/// A built-in folder on this computer. Favorites are view-only conveniences;
/// they are not part of the persisted remote bookmark list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteItem {
    pub name: String,
    pub path: String,
}

/// Build the native sidebar's stable Favorites ordering for a home directory.
///
/// The paths intentionally mirror Finder/Tauri's built-ins. Keeping this pure
/// lets the shell seed the rows without a store round trip and keeps tests
/// deterministic; a failed mount remains an ordinary model error.
#[must_use]
pub fn builtin_favorites(home: &str) -> Vec<FavoriteItem> {
    let home = home.trim_end_matches('/');
    let home = if home.is_empty() { "/" } else { home };
    let child = |name: &str| FavoriteItem {
        name: name.to_owned(),
        path: Path::new(home).join(name).to_string_lossy().into_owned(),
    };
    vec![
        FavoriteItem {
            name: "Home".to_owned(),
            path: home.to_owned(),
        },
        FavoriteItem {
            name: "Applications".to_owned(),
            path: "/Applications".to_owned(),
        },
        child("Desktop"),
        child("Documents"),
        child("Pictures"),
        child("Movies"),
    ]
}

/// Build built-ins from the process home directory for the native shell.
#[must_use]
pub fn builtin_favorites_from_env() -> Vec<FavoriteItem> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    builtin_favorites(&home.to_string_lossy())
}

/// Live facts supplied by the model for one saved connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSummary {
    pub id: String,
    pub info: Option<MountInfo>,
    pub reachable: bool,
    pub reason: Option<String>,
}

impl MountSummary {
    #[must_use]
    pub fn mounted(id: impl Into<String>, info: MountInfo) -> Self {
        Self {
            id: id.into(),
            info: Some(info),
            reachable: true,
            reason: None,
        }
    }

    #[must_use]
    pub fn unreachable(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            info: None,
            reachable: false,
            reason: Some(reason.into()),
        }
    }
}

/// A saved connection plus the latest model-owned reachability/mount facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkItem {
    pub spec: ConnectionSpec,
    pub mount: Option<MountInfo>,
    pub reachable: bool,
    pub unreachable_reason: Option<String>,
}

impl BookmarkItem {
    #[must_use]
    pub fn saved(spec: ConnectionSpec) -> Self {
        Self {
            spec,
            mount: None,
            reachable: true,
            unreachable_reason: None,
        }
    }

    #[must_use]
    pub fn from_summary(spec: ConnectionSpec, summary: Option<&MountSummary>) -> Self {
        let mut item = Self::saved(spec);
        if let Some(summary) = summary {
            item.mount.clone_from(&summary.info);
            item.reachable = summary.reachable;
            item.unreachable_reason.clone_from(&summary.reason);
        }
        item
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.spec.id
    }

    #[must_use]
    pub fn mounted(&self) -> bool {
        self.mount.is_some()
    }
}

/// The row state used for labels and styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkStatus {
    Mounted,
    ReadOnly,
    Unmounted,
    Unreachable,
    Connecting,
}

#[must_use]
pub fn status(item: &BookmarkItem) -> BookmarkStatus {
    match item.mount.as_ref() {
        Some(info) if info.status == MountStatus::Mounting => BookmarkStatus::Connecting,
        Some(info) if info.writable => BookmarkStatus::Mounted,
        Some(_) => BookmarkStatus::ReadOnly,
        None if !item.reachable => BookmarkStatus::Unreachable,
        None => BookmarkStatus::Unmounted,
    }
}

/// The subtitle shown beneath a saved connection.
#[must_use]
pub fn subtitle(item: &BookmarkItem) -> String {
    if status(item) == BookmarkStatus::Unreachable {
        return item
            .unreachable_reason
            .clone()
            .unwrap_or_else(|| "unreachable".to_owned());
    }
    if item.spec.protocol == excalibur_engine::backend::Protocol::Local {
        format!("LOCAL · {}", item.spec.share)
    } else {
        format!(
            "{} · {}",
            item.spec.protocol.scheme().to_ascii_uppercase(),
            item.spec.host
        )
    }
}

/// A short status label for badges and accessibility descriptions.
#[must_use]
pub const fn status_label(value: BookmarkStatus) -> &'static str {
    match value {
        BookmarkStatus::Mounted => "mounted",
        BookmarkStatus::ReadOnly => "read-only",
        BookmarkStatus::Unmounted => "not mounted",
        BookmarkStatus::Unreachable => "unreachable",
        BookmarkStatus::Connecting => "connecting",
    }
}

/// Split in stable input order: mounted rows appear in MOUNTED and all other
/// saved rows appear in BOOKMARKS.
#[must_use]
pub fn partition(items: &[BookmarkItem]) -> (Vec<BookmarkItem>, Vec<BookmarkItem>) {
    let mut mounted = Vec::new();
    let mut saved = Vec::new();
    for item in items {
        if item.mounted() {
            mounted.push(item.clone());
        } else {
            saved.push(item.clone());
        }
    }
    (mounted, saved)
}

/// Events emitted by the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarksEvent {
    Add,
    Select { id: String },
    Remove { id: String },
    Edit { id: String },
    Activate { id: String },
    ActivateFavorite { path: String },
}

impl EventEmitter<BookmarksEvent> for BookmarksSidebar {}

/// Saved connection sidebar with separate mounted and unmounted sections.
pub struct BookmarksSidebar {
    items: Arc<[BookmarkItem]>,
    favorites: Arc<[FavoriteItem]>,
    active_id: Option<String>,
    active_favorite: Option<String>,
    selected_id: Option<String>,
    palette: Palette,
    focus: FocusHandle,
    mounted_scroll: UniformListScrollHandle,
    saved_scroll: UniformListScrollHandle,
}

/// Alias for callers that use the shorter view name.
pub type Bookmarks = BookmarksSidebar;

impl BookmarksSidebar {
    /// Construct an empty sidebar. Install settings-backed rows with
    /// [`set_items`], then update live mount facts as sessions change.
    #[must_use]
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            items: Arc::from(Vec::new()),
            favorites: Arc::from(builtin_favorites_from_env()),
            active_id: None,
            active_favorite: None,
            selected_id: None,
            palette: Palette::light(),
            focus: cx.focus_handle(),
            mounted_scroll: UniformListScrollHandle::new(),
            saved_scroll: UniformListScrollHandle::new(),
        }
    }

    #[must_use]
    pub fn with_items(
        window: &mut Window,
        cx: &mut Context<Self>,
        items: Vec<BookmarkItem>,
    ) -> Self {
        let mut sidebar = Self::new(window, cx);
        sidebar.items = Arc::from(items);
        sidebar
    }

    pub fn set_items(&mut self, items: Vec<BookmarkItem>, cx: &mut Context<Self>) {
        self.items = Arc::from(items);
        self.retain_selection();
        cx.notify();
    }

    /// Replace built-ins for a host-specific environment or deterministic UI
    /// fixture. This never writes to the settings store.
    pub fn set_favorites(&mut self, favorites: Vec<FavoriteItem>, cx: &mut Context<Self>) {
        self.favorites = Arc::from(favorites);
        self.retain_active_favorite();
        cx.notify();
    }

    /// Rebuild view rows from saved specs and the model's current mount map.
    pub fn set_specs(
        &mut self,
        specs: Vec<ConnectionSpec>,
        summaries: &[MountSummary],
        cx: &mut Context<Self>,
    ) {
        let items = specs
            .into_iter()
            .map(|spec| {
                let summary = summaries.iter().find(|summary| summary.id == spec.id);
                BookmarkItem::from_summary(spec, summary)
            })
            .collect();
        self.set_items(items, cx);
    }

    pub fn set_active(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let clear_favorite = id.is_some() && self.active_favorite.is_some();
        if self.active_id != id || clear_favorite {
            self.active_id = id;
            if clear_favorite {
                self.active_favorite = None;
            }
            cx.notify();
        }
    }

    /// Set the path highlighted in the built-in Favorites section.
    ///
    /// A favorite is not a persisted bookmark and therefore has no bookmark
    /// ID. The path is the stable identity used by both the sidebar and the
    /// shell's local mount request.
    pub fn set_active_favorite(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let clear_bookmark = path.is_some() && self.active_id.is_some();
        if self.active_favorite != path || clear_bookmark {
            self.active_favorite = path;
            if clear_bookmark {
                self.active_id = None;
            }
            cx.notify();
        }
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.selected_id.as_deref()
    }

    #[must_use]
    pub fn active_favorite(&self) -> Option<&str> {
        self.active_favorite.as_deref()
    }

    #[must_use]
    pub fn items(&self) -> &[BookmarkItem] {
        &self.items
    }

    fn retain_selection(&mut self) {
        if self
            .selected_id
            .as_ref()
            .is_some_and(|id| !self.items.iter().any(|item| item.id() == id))
        {
            self.selected_id = None;
        }
        if self
            .active_id
            .as_ref()
            .is_some_and(|id| !self.items.iter().any(|item| item.id() == id))
        {
            self.active_id = None;
        }
        self.retain_active_favorite();
    }

    fn retain_active_favorite(&mut self) {
        if self
            .active_favorite
            .as_ref()
            .is_some_and(|path| !self.favorites.iter().any(|favorite| favorite.path == *path))
        {
            self.active_favorite = None;
        }
    }

    fn select(&mut self, item: &BookmarkItem, cx: &mut Context<Self>) {
        self.selected_id = Some(item.id().to_owned());
        self.active_favorite = None;
        cx.emit(BookmarksEvent::Select {
            id: item.id().to_owned(),
        });
        cx.notify();
    }

    fn activate(item: &BookmarkItem, cx: &mut Context<Self>) {
        cx.emit(BookmarksEvent::Activate {
            id: item.id().to_owned(),
        });
    }

    fn emit(event: BookmarksEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    fn render_favorite_row(
        &self,
        favorite: &FavoriteItem,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = self.palette;
        let active = self.active_favorite.as_deref() == Some(favorite.path.as_str());
        let path = favorite.path.clone();
        div()
            .id(SharedString::from(format!("favorite-row-{path}")))
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(favorite_row_inset(active)))
            .border_l_3()
            .border_color(theme::color(if active {
                palette.accent
            } else {
                palette.sidebar
            }))
            .cursor_pointer()
            .when(active, |row| row.bg(theme::color(palette.accent_soft)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.set_active_favorite(Some(path.clone()), cx);
                    Self::emit(BookmarksEvent::ActivateFavorite { path: path.clone() }, cx);
                    cx.stop_propagation();
                }),
            )
            .child(favorite_icon(&favorite.name, palette))
            .child(
                theme::text(
                    favorite.name.clone(),
                    12.,
                    theme::color(palette.text_primary),
                )
                .truncate(),
            )
    }

    fn render_favorites(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("favorites-section")
            .flex()
            .flex_col()
            .child(
                theme::mono("FAVORITES", 9.5, theme::color(self.palette.text_secondary))
                    .h(px(SECTION_HEADER))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .font_weight(FontWeight(600.)),
            )
            .children(
                self.favorites
                    .iter()
                    .map(|favorite| self.render_favorite_row(favorite, cx)),
            )
    }

    fn render_row(
        &mut self,
        item: &BookmarkItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = self.palette;
        let selected = self.selected_id.as_deref() == Some(item.id());
        let active = self.active_id.as_deref() == Some(item.id());
        let unreachable = status(item) == BookmarkStatus::Unreachable;
        let read_only = status(item) == BookmarkStatus::ReadOnly;
        let id = item.id().to_owned();
        let mut row = div()
            .id(SharedString::from(format!("bookmark-row-{id}")))
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(if active { 9. } else { 12. }))
            .border_l_3()
            .border_color(theme::color(if active {
                palette.accent
            } else {
                palette.sidebar
            }))
            .opacity(if unreachable { 0.58 } else { 1. })
            .when(selected || active, |row| {
                row.bg(theme::color(palette.accent_soft))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let Some(item) = this.items.iter().find(|item| item.id() == id).cloned() else {
                        return;
                    };
                    this.focus.focus(window, cx);
                    this.select(&item, cx);
                    if event.click_count >= 2 {
                        Self::activate(&item, cx);
                    }
                    cx.stop_propagation();
                }),
            );
        if item.spec.protocol == excalibur_engine::backend::Protocol::Local {
            row = row.child(
                div()
                    .w(px(14.))
                    .h(px(10.))
                    .rounded(px(2.))
                    .bg(theme::color(palette.folder)),
            );
        } else {
            row = row.child(
                theme::text(
                    "▪",
                    12.,
                    theme::color(if active { palette.accent } else { palette.file }),
                )
                .w(px(14.))
                .flex_none(),
            );
        }
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .justify_center()
            .child(
                theme::text(
                    item.spec.name.clone(),
                    12.,
                    theme::color(if unreachable {
                        palette.text_disabled
                    } else {
                        palette.text_primary
                    }),
                )
                .truncate(),
            )
            .child(theme::mono(
                subtitle(item),
                9.5,
                theme::color(if active {
                    palette.accent
                } else {
                    palette.text_tertiary
                }),
            ));
        if read_only {
            text = text.child(theme::mono("RO", 8.5, theme::color(palette.danger)));
        }
        row.child(text)
    }

    fn render_section(
        &mut self,
        label: &'static str,
        rows: Vec<BookmarkItem>,
        id: &'static str,
        scroll: &UniformListScrollHandle,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let rows: Arc<[BookmarkItem]> = Arc::from(rows);
        let row_count = u16::try_from(rows.len()).unwrap_or(u16::MAX);
        let row_height = f32::from(row_count) * ROW_HEIGHT;
        div()
            .id(id)
            .flex()
            .flex_col()
            .child(
                theme::mono(label, 9.5, theme::color(self.palette.text_secondary))
                    .h(px(SECTION_HEADER))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .font_weight(FontWeight(600.)),
            )
            .child(
                uniform_list(
                    SharedString::from(format!("{id}-rows")),
                    rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        range
                            .filter_map(|index| rows.get(index))
                            .map(|item| this.render_row(item, window, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(scroll)
                .h(px(row_height))
                .min_h_0(),
            )
    }
}

fn favorite_row_inset(active: bool) -> f32 {
    if active {
        9.
    } else {
        12.
    }
}

fn favorite_icon(name: &str, palette: Palette) -> AnyElement {
    if let Some(asset) = crate::assets::favorite_asset(name) {
        return img(asset)
            .w(px(FAVORITE_ICON_SIZE))
            .h(px(FAVORITE_ICON_SIZE))
            .into_any_element();
    }
    theme::text(favorite_glyph(name), 13., theme::color(palette.folder))
        .w(px(14.))
        .flex_none()
        .into_any_element()
}

fn favorite_glyph(name: &str) -> &'static str {
    match name {
        "Home" => "⌂",
        "Applications" => "▦",
        "Desktop" => "▣",
        "Documents" => "▤",
        "Pictures" => "▧",
        "Movies" => "▶",
        _ => "▰",
    }
}

impl Render for BookmarksSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (mounted, saved) = partition(&self.items);
        let palette = self.palette;
        let mounted_scroll = self.mounted_scroll.clone();
        let saved_scroll = self.saved_scroll.clone();
        div()
            .id("bookmarks-sidebar")
            .h_full()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme::color(palette.sidebar))
            .track_focus(&self.focus)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_section(
                        "MOUNTED",
                        mounted,
                        "mounted-bookmarks",
                        &mounted_scroll,
                        cx,
                    ))
                    .child(self.render_favorites(cx))
                    .child(self.render_section(
                        "BOOKMARKS",
                        saved,
                        "saved-bookmarks",
                        &saved_scroll,
                        cx,
                    )),
            )
            .child(self.render_footer(palette, cx))
    }
}

impl BookmarksSidebar {
    fn render_footer(&self, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let remove_selected = self.selected_id.clone();
        let edit_selected = self.selected_id.clone();
        div()
            .id("bookmarks-footer")
            .h(px(FOOTER_HEIGHT))
            .flex_none()
            .flex()
            .border_t_1()
            .border_color(theme::color(palette.chrome_line))
            .child(footer_button(
                "+",
                true,
                palette,
                cx.listener(|_, _: &ClickEvent, _, cx| Self::emit(BookmarksEvent::Add, cx)),
            ))
            .child(footer_button(
                "−",
                remove_selected.is_some(),
                palette,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    if let Some(id) = remove_selected.clone() {
                        Self::emit(BookmarksEvent::Remove { id }, cx);
                    }
                }),
            ))
            .child(footer_button(
                "Edit",
                self.selected_id.is_some(),
                palette,
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    if let Some(id) = edit_selected.clone() {
                        Self::emit(BookmarksEvent::Edit { id }, cx);
                    }
                }),
            ))
    }
}

fn footer_button(
    label: &'static str,
    enabled: bool,
    palette: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!("bookmarks-footer-{label}")))
        .flex_1()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .border_r_1()
        .border_color(theme::color(palette.chrome_line))
        .child(theme::text(
            label,
            11.,
            theme::color(if enabled {
                palette.text_secondary
            } else {
                palette.text_disabled
            }),
        ));
    if enabled {
        button = button
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(palette.accent_soft)))
            .on_click(on_click);
    }
    button
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_favorites, favorite_row_inset, partition, status, subtitle, BookmarkItem,
        BookmarkStatus,
    };
    use excalibur_engine::backend::{Access, ConnectionSpec, MountInfo, MountStatus, Protocol};

    fn spec(id: &str, protocol: Protocol) -> ConnectionSpec {
        ConnectionSpec {
            id: id.to_owned(),
            name: id.to_owned(),
            protocol,
            host: "nas.local".to_owned(),
            port: None,
            share: "media".to_owned(),
            username: None,
            domain: None,
            mount_preference: Access::Rw,
        }
    }

    fn mount(writable: bool, status: MountStatus) -> MountInfo {
        MountInfo {
            status,
            writable,
            reason: String::new(),
            protocol_label: "SMB3".to_owned(),
            security_label: "aes".to_owned(),
            options: String::new(),
            capacity: None,
            caps: excalibur_engine::backend::Caps {
                random_access: false,
                resume_write: false,
                atomic_rename: false,
                symlinks: false,
                watch: false,
            },
        }
    }

    #[test]
    fn partition_preserves_order_and_excludes_mounted_from_bookmarks() {
        let mounted = BookmarkItem {
            spec: spec("mounted", Protocol::Smb),
            mount: Some(mount(true, MountStatus::Ready)),
            reachable: true,
            unreachable_reason: None,
        };
        let saved = BookmarkItem::saved(spec("saved", Protocol::Sftp));
        let (mounted_rows, saved_rows) = partition(&[saved.clone(), mounted, saved]);
        assert_eq!(
            mounted_rows
                .iter()
                .map(BookmarkItem::id)
                .collect::<Vec<_>>(),
            ["mounted"]
        );
        assert_eq!(
            saved_rows.iter().map(BookmarkItem::id).collect::<Vec<_>>(),
            ["saved", "saved"]
        );
    }

    #[test]
    fn labels_cover_local_remote_and_unreachable_rows() {
        let remote = BookmarkItem::saved(spec("remote", Protocol::Smb));
        assert_eq!(subtitle(&remote), "SMB · nas.local");
        let local = BookmarkItem::saved(spec("local", Protocol::Local));
        assert_eq!(subtitle(&local), "LOCAL · media");
        let unreachable = BookmarkItem {
            unreachable_reason: Some("host offline".to_owned()),
            reachable: false,
            ..remote
        };
        assert_eq!(status(&unreachable), BookmarkStatus::Unreachable);
        assert_eq!(subtitle(&unreachable), "host offline");
    }

    #[test]
    fn read_only_and_connecting_are_distinct_statuses() {
        let read_only = BookmarkItem {
            spec: spec("ro", Protocol::Smb),
            mount: Some(mount(false, MountStatus::Ready)),
            reachable: true,
            unreachable_reason: None,
        };
        assert_eq!(status(&read_only), BookmarkStatus::ReadOnly);
        let connecting = BookmarkItem {
            mount: Some(mount(true, MountStatus::Mounting)),
            ..read_only
        };
        assert_eq!(status(&connecting), BookmarkStatus::Connecting);
    }

    #[test]
    fn built_in_favorites_match_finder_order_and_paths() {
        let favorites = builtin_favorites("/Users/dorian/");
        assert_eq!(
            favorites
                .iter()
                .map(|favorite| favorite.name.as_str())
                .collect::<Vec<_>>(),
            [
                "Home",
                "Applications",
                "Desktop",
                "Documents",
                "Pictures",
                "Movies",
            ]
        );
        assert_eq!(favorites[0].path, "/Users/dorian");
        assert_eq!(favorites[1].path, "/Applications");
        assert_eq!(favorites[2].path, "/Users/dorian/Desktop");
        assert_eq!(favorites[3].path, "/Users/dorian/Documents");
        assert_eq!(favorites[4].path, "/Users/dorian/Pictures");
        assert_eq!(favorites[5].path, "/Users/dorian/Movies");
    }

    #[test]
    fn built_in_favorites_are_not_remote_bookmark_rows() {
        let favorites = builtin_favorites("/Users/dorian");
        assert!(favorites.iter().all(|favorite| !favorite.name.is_empty()));
        assert!(favorites
            .iter()
            .all(|favorite| favorite.path.starts_with('/')));
    }

    #[test]
    fn root_fallback_does_not_create_double_slashes() {
        let favorites = builtin_favorites("/");
        assert_eq!(favorites[0].path, "/");
        assert_eq!(favorites[2].path, "/Desktop");
    }

    #[test]
    fn active_favorite_uses_the_mounted_row_alignment() {
        assert!((favorite_row_inset(true) - 9.).abs() < f32::EPSILON);
        assert!((favorite_row_inset(false) - 12.).abs() < f32::EPSILON);
    }
}
