//! Virtualised metadata-only file table for the native shell.
//!
//! The table deliberately accepts [`crate::model::EntryRow`] rather than an
//! engine backend. It therefore cannot read file bytes, and a model can swap
//! listings without making the view aware of whether a row came from SMB,
//! SFTP, xsync, or the local filesystem. Filtering and ordering remain the
//! responsibility of [`crate::browser::BrowserState`]; header gestures emit a
//! typed event for the model to apply there.

use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    canvas, div, px, uniform_list, ClickEvent, Context, Div, DragMoveEvent, EventEmitter,
    FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, ScrollStrategy,
    SharedString, Stateful, UniformListScrollHandle, Window,
};

use crate::browser::{self, Modifiers, RowLayout, Selection, Sort, SortColumn};
use crate::model::EntryRow;
use crate::theme::{self, Palette};
use excalibur_engine::backend::EntryKind;

/// Height of one virtual list row.
pub const ROW_HEIGHT: f32 = theme::geometry::ROW;
const ICON_CARD_WIDTH: f32 = 128.;
const ICON_CARD_HEIGHT: f32 = 92.;
const ICON_GRID_GAP: f32 = 14.;
const ICON_GRID_PADDING: f32 = 16.;
const ICON_ROW_HEIGHT: f32 = ICON_CARD_HEIGHT + ICON_GRID_GAP;
const HEADER_HEIGHT: f32 = theme::geometry::COLUMN_HEADER;
const SIZE_WIDTH: f32 = 92.;
const MODIFIED_WIDTH: f32 = 150.;
const KIND_WIDTH: f32 = 120.;
const PARENT_WIDTH: f32 = 180.;
const MIN_COLUMN_WIDTH: f32 = 54.;
const MAX_COLUMN_WIDTH: f32 = 900.;
const RESIZE_EDGE: f32 = 7.;

/// A stable identity for a visible header column. `SortColumn` is not enough:
/// DETAILS and PARENT are useful Finder-style columns but are not sortable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderColumn {
    Name,
    Size,
    Modified,
    Kind,
    Parent,
    Details,
}

impl HeaderColumn {
    const fn index(self) -> usize {
        match self {
            Self::Name => 0,
            Self::Size => 1,
            Self::Modified => 2,
            Self::Kind => 3,
            Self::Parent => 4,
            Self::Details => 5,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HeaderDrag;

struct NoopDrag;

impl Render for NoopDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

enum HeaderGesture {
    Resize {
        column: HeaderColumn,
        start_x: f32,
        start_width: f32,
    },
}

/// The events a shell or model can subscribe to from a [`FileTable`] entity.
///
/// Selection paths are returned in visible row order, rather than hash-set
/// order, so consumers can use the event directly for status text and actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTableEvent {
    /// A click or keyboard move changed the selected paths.
    SelectionChanged {
        paths: Vec<String>,
        anchor: Option<String>,
        modifiers: Modifiers,
    },
    /// A row was activated by double-click or Enter.
    Activated { path: String, directory: bool },
    /// A sortable header was clicked. The model should apply this sort to its
    /// browser state and provide the resulting rows again.
    SortChanged { sort: Sort },
}

impl EventEmitter<FileTableEvent> for FileTable {}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HeaderSpec {
    label: &'static str,
    column: HeaderColumn,
    sort: Option<SortColumn>,
    width: Option<f32>,
}

const FULL_HEADER: [HeaderSpec; 4] = [
    HeaderSpec {
        label: "NAME",
        column: HeaderColumn::Name,
        sort: Some(SortColumn::Name),
        width: None,
    },
    HeaderSpec {
        label: "SIZE",
        column: HeaderColumn::Size,
        sort: Some(SortColumn::Size),
        width: Some(SIZE_WIDTH),
    },
    HeaderSpec {
        label: "MODIFIED",
        column: HeaderColumn::Modified,
        sort: Some(SortColumn::Modified),
        width: Some(MODIFIED_WIDTH),
    },
    HeaderSpec {
        label: "KIND",
        column: HeaderColumn::Kind,
        sort: Some(SortColumn::Kind),
        width: Some(KIND_WIDTH),
    },
];

const NAME_LED_HEADER: [HeaderSpec; 2] = [
    HeaderSpec {
        label: "NAME",
        column: HeaderColumn::Name,
        sort: Some(SortColumn::Name),
        width: None,
    },
    HeaderSpec {
        label: "DETAILS",
        column: HeaderColumn::Details,
        sort: None,
        width: Some(SIZE_WIDTH + MODIFIED_WIDTH + KIND_WIDTH),
    },
];

const ICONS_HEADER: [HeaderSpec; 1] = [HeaderSpec {
    label: "ICONS",
    column: HeaderColumn::Name,
    sort: None,
    width: None,
}];

const COLUMN_VIEW_HEADER: [HeaderSpec; 3] = [
    HeaderSpec {
        label: "PARENT",
        column: HeaderColumn::Parent,
        sort: None,
        width: Some(PARENT_WIDTH),
    },
    HeaderSpec {
        label: "NAME",
        column: HeaderColumn::Name,
        sort: Some(SortColumn::Name),
        width: None,
    },
    HeaderSpec {
        label: "KIND",
        column: HeaderColumn::Kind,
        sort: Some(SortColumn::Kind),
        width: Some(KIND_WIDTH),
    },
];

fn header_specs(layout: RowLayout) -> &'static [HeaderSpec] {
    match layout {
        RowLayout::Columns | RowLayout::Mono => &FULL_HEADER,
        RowLayout::NameLed => &NAME_LED_HEADER,
        RowLayout::Icons => &ICONS_HEADER,
        RowLayout::ColumnView => &COLUMN_VIEW_HEADER,
    }
}

fn column_label(column: SortColumn) -> &'static str {
    match column {
        SortColumn::Name => "NAME",
        SortColumn::Size => "SIZE",
        SortColumn::Modified => "MODIFIED",
        SortColumn::Kind => "KIND",
        SortColumn::Added => "ADDED",
    }
}

#[cfg(test)]
fn column_width(column: SortColumn) -> Option<f32> {
    match column {
        SortColumn::Size => Some(SIZE_WIDTH),
        SortColumn::Modified => Some(MODIFIED_WIDTH),
        SortColumn::Kind => Some(KIND_WIDTH),
        SortColumn::Name | SortColumn::Added => None,
    }
}

fn kind_glyph(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Dir => "▰",
        EntryKind::File => "▪",
        EntryKind::Symlink => "↗",
        EntryKind::Other => "·",
    }
}

fn display_size(row: &EntryRow) -> String {
    if row.kind == EntryKind::Dir {
        "—".to_owned()
    } else {
        browser::format_size(row.size)
    }
}

fn display_modified(row: &EntryRow) -> String {
    browser::format_modified(row.modified_ms)
}

fn display_kind(row: &EntryRow) -> &str {
    if row.kind == EntryKind::Dir {
        "Folder"
    } else {
        &row.kind_label
    }
}

fn row_height(layout: RowLayout) -> f32 {
    if layout == RowLayout::Icons {
        ICON_ROW_HEIGHT
    } else {
        ROW_HEIGHT
    }
}

/// Number of icon tiles that fit in the viewport, including the outer padding.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the positive, finite viewport count is floored before conversion"
)]
fn icon_columns(width: f32) -> usize {
    if !width.is_finite() || width <= 0. {
        return 1;
    }
    (((width - 2. * ICON_GRID_PADDING + ICON_GRID_GAP) / (ICON_CARD_WIDTH + ICON_GRID_GAP)).floor()
        as usize)
        .max(1)
}

fn icon_rows(items: usize, columns: usize) -> usize {
    items.div_ceil(columns.max(1))
}

fn resized_width(start: f32, delta: f32) -> f32 {
    (start + delta).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
}

fn page_delta(key: &str, page_rows: usize) -> Option<isize> {
    let page = page_rows.max(1).cast_signed();
    match key {
        "up" => Some(-1),
        "down" => Some(1),
        "pageup" => Some(-page),
        "pagedown" => Some(page),
        _ => None,
    }
}

fn modifiers(event: &MouseDownEvent) -> Modifiers {
    Modifiers {
        shift: event.modifiers.shift,
        toggle: event.modifiers.secondary(),
    }
}

/// A metadata-only, virtualised file table.
pub struct FileTable {
    rows: Arc<[EntryRow]>,
    selection: Selection,
    sort: Sort,
    layout: RowLayout,
    palette: Palette,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Per-column overrides, retained while switching views. `None` means the
    /// Finder-style flexible NAME column; fixed columns start at their native
    /// widths and become overrides after the first drag.
    column_widths: [Option<f32>; 6],
    header_gesture: Option<HeaderGesture>,
    /// The icon grid measures its viewport so it can change the number of
    /// cards per row without rebuilding the model or using horizontal lists.
    icon_view_width: Rc<Cell<f32>>,
}

impl FileTable {
    /// Construct an empty table. Supply the first listing with [`set_rows`].
    ///
    /// The `window` argument follows GPUI's entity-construction convention;
    /// no window state is retained by this component.
    #[must_use]
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            rows: Arc::from(Vec::new()),
            selection: Selection::default(),
            sort: Sort::DEFAULT,
            layout: RowLayout::Columns,
            palette: Palette::light(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            column_widths: [
                None,
                Some(SIZE_WIDTH),
                Some(MODIFIED_WIDTH),
                Some(KIND_WIDTH),
                Some(PARENT_WIDTH),
                Some(SIZE_WIDTH + MODIFIED_WIDTH + KIND_WIDTH),
            ],
            header_gesture: None,
            icon_view_width: Rc::new(Cell::new(0.)),
        }
    }

    /// Construct a table with its first metadata listing already installed.
    #[must_use]
    pub fn with_rows(window: &mut Window, cx: &mut Context<Self>, rows: Vec<EntryRow>) -> Self {
        let mut table = Self::new(window, cx);
        table.rows = Arc::from(rows);
        table
    }

    /// Replace the visible listing while retaining selected paths that still
    /// exist. This makes refreshes and resorting safe for path-keyed selection.
    pub fn set_rows(&mut self, rows: Vec<EntryRow>, cx: &mut Context<Self>) {
        let visible: HashSet<&str> = rows.iter().map(|row| row.path.as_str()).collect();
        self.selection
            .paths
            .retain(|path| visible.contains(path.as_str()));
        if self
            .selection
            .anchor
            .as_ref()
            .is_some_and(|path| !visible.contains(path.as_str()))
        {
            self.selection.anchor = None;
        }
        self.rows = Arc::from(rows);
        cx.notify();
    }

    /// Install the sort indicator supplied by the model.
    pub fn set_sort(&mut self, sort: Sort, cx: &mut Context<Self>) {
        if self.sort != sort {
            self.sort = sort;
            cx.notify();
        }
    }

    /// Change the row presentation while retaining path-keyed selection and
    /// the virtual list's scroll position.
    pub fn set_layout(&mut self, layout: RowLayout, cx: &mut Context<Self>) {
        if self.layout != layout {
            self.layout = layout;
            cx.notify();
        }
    }

    /// Update the theme tokens used by the table. Shells should call this when
    /// switching between light and dark appearance.
    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    #[must_use]
    pub const fn sort(&self) -> Sort {
        self.sort
    }

    #[must_use]
    pub const fn layout(&self) -> RowLayout {
        self.layout
    }

    #[must_use]
    pub fn rows(&self) -> &[EntryRow] {
        &self.rows
    }

    #[must_use]
    pub const fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Selected paths in the current visible order.
    #[must_use]
    pub fn selected_paths(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| self.selection.paths.contains(&row.path))
            .map(|row| row.path.clone())
            .collect()
    }

    /// The scroll handle is useful to shells that need to coordinate a second
    /// pane or expose an imperative scroll command.
    #[must_use]
    pub fn scroll_handle(&self) -> UniformListScrollHandle {
        self.scroll.clone()
    }

    /// Current width override for a visible header. This is intentionally
    /// public so a shell that persists view preferences can save the values;
    /// the table itself remains usable without a settings backend.
    #[must_use]
    pub fn column_width(&self, column: SortColumn) -> Option<f32> {
        let key = match column {
            SortColumn::Name => HeaderColumn::Name,
            SortColumn::Size => HeaderColumn::Size,
            SortColumn::Modified => HeaderColumn::Modified,
            SortColumn::Kind => HeaderColumn::Kind,
            SortColumn::Added => return None,
        };
        self.column_widths[key.index()]
    }

    fn order(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.path.clone()).collect()
    }

    fn emit_selection(&self, modifiers: Modifiers, cx: &mut Context<Self>) {
        cx.emit(FileTableEvent::SelectionChanged {
            paths: self.selected_paths(),
            anchor: self.selection.anchor.clone(),
            modifiers,
        });
    }

    fn select_at(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let order = self.order();
        self.selection = browser::click(&self.selection, &order, &row.path, modifiers);
        self.emit_selection(modifiers, cx);
        cx.notify();
    }

    fn activate(&self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        cx.emit(FileTableEvent::Activated {
            path: row.path.clone(),
            directory: row.kind == EntryKind::Dir,
        });
    }

    fn header_clicked(&mut self, column: SortColumn, cx: &mut Context<Self>) {
        let sort = self.sort.clicked(column);
        self.sort = sort;
        cx.emit(FileTableEvent::SortChanged { sort });
        cx.notify();
    }

    fn effective_width(&self, spec: HeaderSpec) -> Option<f32> {
        self.column_widths[spec.column.index()].or(spec.width)
    }

    fn begin_column_resize(
        &mut self,
        column: HeaderColumn,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let default = match column {
            HeaderColumn::Name => 320.,
            HeaderColumn::Size => SIZE_WIDTH,
            HeaderColumn::Modified => MODIFIED_WIDTH,
            HeaderColumn::Kind => KIND_WIDTH,
            HeaderColumn::Parent => PARENT_WIDTH,
            HeaderColumn::Details => SIZE_WIDTH + MODIFIED_WIDTH + KIND_WIDTH,
        };
        self.header_gesture = Some(HeaderGesture::Resize {
            column,
            start_x: f32::from(event.position.x),
            start_width: self.column_widths[column.index()].unwrap_or(default),
        });
        cx.stop_propagation();
    }

    fn on_header_drag_move(
        &mut self,
        event: &DragMoveEvent<HeaderDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(HeaderGesture::Resize {
            column,
            start_x,
            start_width,
        }) = self.header_gesture
        else {
            return;
        };
        let width = resized_width(start_width, f32::from(event.event.position.x) - start_x);
        self.column_widths[column.index()] = Some(width);
        cx.notify();
    }

    fn on_header_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.header_gesture.take().is_some() {
            cx.notify();
        }
    }

    fn page_rows(&self) -> usize {
        let state = self.scroll.0.borrow();
        let height = f32::from(state.base_handle.bounds().size.height);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "GPUI layout dimensions are f32 and are known non-negative here"
        )]
        let rows = (height / row_height(self.layout)).ceil() as usize;
        rows.max(1)
    }

    fn keyboard(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let order = self.order();
        if order.is_empty() {
            return;
        }
        if key == "enter" {
            if let Some(index) = browser::far_index(&order, &self.selection) {
                cx.stop_propagation();
                self.activate(index, cx);
            }
            return;
        }
        if key == "a" && event.keystroke.modifiers.secondary() {
            self.selection.paths = order.iter().cloned().collect();
            self.selection.anchor = order.first().cloned();
            self.emit_selection(Modifiers::default(), cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let moved = match key {
            "home" if event.keystroke.modifiers.shift => {
                browser::extend_to(&self.selection, &order, 0)
            }
            "end" if event.keystroke.modifiers.shift => {
                browser::extend_to(&self.selection, &order, order.len().cast_signed() - 1)
            }
            "home" => browser::move_to_index(&order, 0),
            "end" => browser::move_to_index(&order, order.len().cast_signed() - 1),
            _ => {
                let Some(delta) = page_delta(key, self.page_rows()) else {
                    return;
                };
                if event.keystroke.modifiers.shift {
                    browser::extend_by(&self.selection, &order, delta)
                } else {
                    browser::step(&self.selection, &order, delta)
                }
            }
        };
        self.selection = moved.selection;
        if let Some(index) = moved.reveal {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.emit_selection(
            Modifiers {
                shift: event.keystroke.modifiers.shift,
                toggle: false,
            },
            cx,
        );
        self.focus.focus(window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn row_mouse_down(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let modifiers = modifiers(event);
        self.select_at(index, modifiers, cx);
        if event.click_count >= 2 {
            self.activate(index, cx);
        }
        cx.stop_propagation();
    }

    fn render_header(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut header = div()
            .id("file-table-header")
            .relative()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(theme::color(palette.toolbar))
            .border_b_1()
            .border_color(theme::color(palette.chrome_line));
        for spec in header_specs(self.layout) {
            header = match spec.sort {
                Some(column) => header.child(self.sortable_header(*spec, column, palette, cx)),
                None => header.child(self.plain_header(*spec, palette, cx)),
            }
        }
        header
    }

    fn sortable_header(
        &self,
        spec: HeaderSpec,
        column: SortColumn,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let sorted = self.sort.column == column;
        let arrow = if self.sort.ascending { "▲" } else { "▼" };
        let mut cell = div()
            .id(SharedString::from(format!(
                "file-table-header-cell-{}",
                column_label(column)
            )))
            .relative()
            .h_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(9.))
            .cursor_pointer()
            .hover(|style| style.bg(theme::color(palette.accent_soft)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.header_clicked(column, cx);
            }));
        cell = match self.effective_width(spec) {
            Some(width) => cell.flex_none().w(px(width)),
            None => cell.flex_1().min_w_0(),
        }
        .child(theme::mono(
            column_label(column),
            9.5,
            theme::color(palette.text_secondary),
        ))
        .when(sorted, |cell| {
            cell.child(theme::mono(arrow, 8., theme::color(palette.accent)))
        })
        .child(Self::resize_handle(spec.column, palette, cx));
        cell
    }

    fn plain_header(
        &self,
        spec: HeaderSpec,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut cell = div()
            .id(SharedString::from(format!(
                "file-table-header-cell-{}",
                spec.label
            )))
            .relative()
            .h_full()
            .flex()
            .items_center()
            .px(px(9.))
            .child(theme::mono(
                spec.label,
                9.5,
                theme::color(palette.text_secondary),
            ));
        cell = match self.effective_width(spec) {
            Some(width) => cell.flex_none().w(px(width)),
            None => cell.flex_1().min_w_0(),
        };
        cell.child(Self::resize_handle(spec.column, palette, cx))
    }

    fn resize_handle(
        column: HeaderColumn,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = format!("file-table-resize-{}", column.index());
        div()
            .id(SharedString::from(id))
            .absolute()
            .top_0()
            .right_0()
            .h_full()
            .w(px(RESIZE_EDGE))
            .cursor_col_resize()
            .hover(|style| style.bg(theme::color(palette.accent_soft)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.begin_column_resize(column, event, cx);
                }),
            )
            .on_drag(HeaderDrag, |_, _, _, cx| cx.new(|_| NoopDrag))
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let palette = self.palette();
        let focused = self.focus.is_focused(window);
        let widths = self.column_widths;
        range
            .filter_map(|index| self.rows.get(index).cloned().map(|row| (index, row)))
            .map(|(index, row)| {
                let selected = self.selection.paths.contains(&row.path);
                let focus_ring =
                    focused && self.selection.anchor.as_deref() == Some(row.path.as_str());
                let path = row.path.clone();
                let glyph = kind_glyph(row.kind);
                let opacity = if row.hidden { 0.62 } else { 1. };
                let mut element = div()
                    .id(SharedString::from(format!("file-table-row-{path}")))
                    .h(px(row_height(self.layout)))
                    .w_full()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::color(palette.row_line))
                    .opacity(opacity)
                    .when(selected, |row| {
                        row.bg(theme::color(palette.accent_soft))
                            .border_l_2()
                            .border_color(theme::color(palette.accent))
                    })
                    .when(!selected, |row| {
                        row.hover(|style| style.bg(theme::color(palette.accent_soft)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.row_mouse_down(index, event, window, cx);
                        }),
                    );
                if focus_ring {
                    element = element.child(
                        div()
                            .absolute()
                            .inset_0()
                            .border_1()
                            .border_color(theme::color(palette.accent)),
                    );
                }
                let content = match self.layout {
                    RowLayout::Columns => columns_content(&row, glyph, palette, widths),
                    RowLayout::NameLed => name_led_content(&row, glyph, palette, widths),
                    RowLayout::Mono => mono_content(&row, glyph, palette, widths),
                    // Icon mode is rendered by render_icon_rows, but keeping
                    // this branch makes the row renderer total and safe if a
                    // future caller reuses it directly.
                    RowLayout::Icons => icon_tile(&row, glyph, palette, selected),
                    RowLayout::ColumnView => column_view_content(&row, glyph, palette, widths),
                };
                element.child(content)
            })
            .collect()
    }

    fn render_icon_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let palette = self.palette();
        let focused = self.focus.is_focused(window);
        let columns = icon_columns(self.icon_view_width.get());
        range
            .map(|grid_row| {
                let start = grid_row.saturating_mul(columns);
                let end = (start + columns).min(self.rows.len());
                let mut row = div()
                    .id(SharedString::from(format!(
                        "file-table-icon-row-{grid_row}"
                    )))
                    .h(px(ICON_ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .gap(px(ICON_GRID_GAP))
                    .px(px(ICON_GRID_PADDING));
                for index in start..end {
                    let Some(entry) = self.rows.get(index) else {
                        continue;
                    };
                    let selected = self.selection.paths.contains(&entry.path);
                    let focus_ring =
                        focused && self.selection.anchor.as_deref() == Some(entry.path.as_str());
                    let path = entry.path.clone();
                    let glyph = kind_glyph(entry.kind);
                    let opacity = if entry.hidden { 0.62 } else { 1. };
                    let mut tile = div()
                        .id(SharedString::from(format!("file-table-icon-{path}")))
                        .relative()
                        .w(px(ICON_CARD_WIDTH))
                        .h(px(ICON_CARD_HEIGHT))
                        .flex_none()
                        .opacity(opacity)
                        .when(selected, |tile| {
                            tile.bg(theme::color(palette.accent_soft))
                                .border_1()
                                .border_color(theme::color(palette.accent))
                        })
                        .when(!selected, |tile| {
                            tile.hover(|style| style.bg(theme::color(palette.accent_soft)))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.row_mouse_down(index, event, window, cx);
                            }),
                        )
                        .child(icon_tile(entry, glyph, palette, selected));
                    if focus_ring {
                        tile = tile.child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_1()
                                .border_color(theme::color(palette.accent)),
                        );
                    }
                    row = row.child(tile);
                }
                row
            })
            .collect()
    }

    fn render_icon_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.icon_view_width.clone();
        let entity = cx.entity_id();
        let rows = icon_rows(self.rows.len(), icon_columns(self.icon_view_width.get()));
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let width = f32::from(bounds.size.width);
                        if icon_columns(width) != icon_columns(measured.get()) {
                            cx.notify(entity);
                        }
                        measured.set(width);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                uniform_list(
                    "file-table-icon-grid",
                    rows,
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        this.render_icon_rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
    }

    fn palette(&self) -> Palette {
        self.palette
    }
}

fn width_or(widths: [Option<f32>; 6], column: HeaderColumn, fallback: f32) -> f32 {
    widths[column.index()].unwrap_or(fallback)
}

fn name_cell(row: &EntryRow, glyph: &str, palette: Palette, width: Option<f32>) -> Div {
    let mut cell = div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(9.))
        .child(
            theme::text(
                glyph,
                12.,
                theme::color(if row.kind == EntryKind::Dir {
                    palette.folder
                } else {
                    palette.file
                }),
            )
            .w(px(14.))
            .flex_none(),
        )
        .child(
            theme::text(row.name.clone(), 12., theme::color(palette.text_primary))
                .truncate()
                .min_w_0(),
        );
    cell = match width {
        Some(width) => cell.flex_none().w(px(width)),
        None => cell.flex_1(),
    };
    cell
}

fn columns_content(row: &EntryRow, glyph: &str, palette: Palette, widths: [Option<f32>; 6]) -> Div {
    div()
        .flex()
        .items_center()
        .size_full()
        .child(name_cell(
            row,
            glyph,
            palette,
            widths[HeaderColumn::Name.index()],
        ))
        .child(cell(
            display_size(row),
            width_or(widths, HeaderColumn::Size, SIZE_WIDTH),
            palette.text_secondary,
            true,
            palette,
        ))
        .child(cell(
            display_modified(row),
            width_or(widths, HeaderColumn::Modified, MODIFIED_WIDTH),
            palette.text_secondary,
            true,
            palette,
        ))
        .child(cell(
            display_kind(row).to_owned(),
            width_or(widths, HeaderColumn::Kind, KIND_WIDTH),
            palette.text_secondary,
            false,
            palette,
        ))
}

fn name_led_content(
    row: &EntryRow,
    glyph: &str,
    palette: Palette,
    widths: [Option<f32>; 6],
) -> Div {
    div()
        .flex()
        .items_center()
        .size_full()
        .child(name_cell(
            row,
            glyph,
            palette,
            widths[HeaderColumn::Name.index()],
        ))
        .child(
            theme::text(
                format!(
                    "{} · {} · {}",
                    display_kind(row),
                    display_size(row),
                    display_modified(row)
                ),
                10.,
                theme::color(palette.text_secondary),
            )
            .w(px(width_or(
                widths,
                HeaderColumn::Details,
                SIZE_WIDTH + MODIFIED_WIDTH + KIND_WIDTH,
            )))
            .px(px(9.))
            .truncate(),
        )
}

fn mono_content(row: &EntryRow, glyph: &str, palette: Palette, widths: [Option<f32>; 6]) -> Div {
    div()
        .flex()
        .items_center()
        .size_full()
        .child(mono_name_cell(
            row,
            glyph,
            palette,
            widths[HeaderColumn::Name.index()],
        ))
        .child(mono_cell(
            display_size(row),
            width_or(widths, HeaderColumn::Size, SIZE_WIDTH),
            palette.text_secondary,
        ))
        .child(mono_cell(
            display_modified(row),
            width_or(widths, HeaderColumn::Modified, MODIFIED_WIDTH),
            palette.text_secondary,
        ))
        .child(mono_cell(
            display_kind(row).to_owned(),
            width_or(widths, HeaderColumn::Kind, KIND_WIDTH),
            palette.text_secondary,
        ))
}

fn icon_tile(row: &EntryRow, glyph: &str, palette: Palette, selected: bool) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .child(
            theme::text(
                glyph,
                28.,
                theme::color(if row.kind == EntryKind::Dir {
                    palette.folder
                } else {
                    palette.file
                }),
            )
            .w(px(36.))
            .h(px(36.))
            .flex_none(),
        )
        .child(
            theme::text(row.name.clone(), 12., theme::color(palette.text_primary))
                .max_w(px(ICON_CARD_WIDTH - 12.))
                .truncate()
                .when(selected, |text| {
                    text.text_color(theme::color(palette.accent))
                }),
        )
}

fn column_view_content(
    row: &EntryRow,
    glyph: &str,
    palette: Palette,
    widths: [Option<f32>; 6],
) -> Div {
    let parent = row
        .path
        .rsplit_once('/')
        .map_or_else(|| "Share root".to_owned(), |(parent, _)| parent.to_owned());
    div()
        .flex()
        .items_center()
        .size_full()
        .child(
            theme::mono(parent, 9.5, theme::color(palette.text_secondary))
                .w(px(width_or(widths, HeaderColumn::Parent, PARENT_WIDTH)))
                .px(px(9.))
                .truncate(),
        )
        .child(name_cell(
            row,
            glyph,
            palette,
            widths[HeaderColumn::Name.index()],
        ))
        .child(cell(
            display_kind(row).to_owned(),
            width_or(widths, HeaderColumn::Kind, KIND_WIDTH),
            palette.text_secondary,
            false,
            palette,
        ))
}

fn mono_name_cell(row: &EntryRow, glyph: &str, palette: Palette, width: Option<f32>) -> Div {
    let mut cell = div()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(px(7.))
        .px(px(9.))
        .child(
            theme::mono(
                glyph,
                11.,
                theme::color(if row.kind == EntryKind::Dir {
                    palette.folder
                } else {
                    palette.file
                }),
            )
            .w(px(14.))
            .flex_none(),
        )
        .child(
            theme::mono(row.name.clone(), 10., theme::color(palette.text_primary))
                .truncate()
                .min_w_0(),
        );
    cell = match width {
        Some(width) => cell.flex_none().w(px(width)),
        None => cell.flex_1(),
    };
    cell
}

fn mono_cell(value: String, width: f32, colour: u32) -> Div {
    div()
        .flex_none()
        .w(px(width))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .px(px(9.))
        .child(theme::mono(value, 9.5, theme::color(colour)))
}

fn cell(value: String, width: f32, colour: u32, mono: bool, palette: Palette) -> Div {
    let text = if mono {
        theme::mono(value, 10., theme::color(colour))
    } else {
        theme::text(value, 11., theme::color(colour))
    };
    div()
        .flex_none()
        .w(px(width))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .px(px(9.))
        .child(text.when(!mono, |text| {
            text.text_color(theme::color(palette.text_secondary))
        }))
}

impl Render for FileTable {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette();
        let mut table = div()
            .id("file-table")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::color(palette.list))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::keyboard))
            .on_drag_move(cx.listener(Self::on_header_drag_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_header_mouse_up));
        if self.layout == RowLayout::Icons {
            table = table.child(self.render_icon_body(cx));
        } else {
            table = table.child(self.render_header(palette, cx)).child(
                uniform_list(
                    "file-table-rows",
                    self.rows.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        this.render_rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            );
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::{
        column_width, header_specs, icon_columns, icon_rows, kind_glyph, page_delta, resized_width,
        row_height, EntryKind, RowLayout, Sort, SortColumn, FULL_HEADER, ICON_ROW_HEIGHT,
        ROW_HEIGHT, SIZE_WIDTH,
    };

    #[test]
    fn visible_columns_have_expected_titles_and_widths() {
        assert_eq!(
            FULL_HEADER.map(|spec| spec.label),
            ["NAME", "SIZE", "MODIFIED", "KIND"]
        );
        assert_eq!(column_width(SortColumn::Name), None);
        assert_eq!(column_width(SortColumn::Size), Some(SIZE_WIDTH));
    }

    #[test]
    fn headers_match_each_layout_and_only_offer_meaningful_sorting() {
        let labels = |layout| {
            header_specs(layout)
                .iter()
                .map(|spec| spec.label)
                .collect::<Vec<_>>()
        };
        let sortable = |layout| {
            header_specs(layout)
                .iter()
                .filter_map(|spec| spec.sort)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(RowLayout::Columns),
            ["NAME", "SIZE", "MODIFIED", "KIND"]
        );
        assert_eq!(
            labels(RowLayout::Mono),
            ["NAME", "SIZE", "MODIFIED", "KIND"]
        );
        assert_eq!(labels(RowLayout::NameLed), ["NAME", "DETAILS"]);
        assert_eq!(labels(RowLayout::Icons), ["ICONS"]);
        assert_eq!(labels(RowLayout::ColumnView), ["PARENT", "NAME", "KIND"]);
        assert_eq!(sortable(RowLayout::NameLed), [SortColumn::Name]);
        assert!(sortable(RowLayout::Icons).is_empty());
        assert_eq!(
            sortable(RowLayout::ColumnView),
            [SortColumn::Name, SortColumn::Kind]
        );
        assert_eq!(
            row_height(RowLayout::Icons).to_bits(),
            ICON_ROW_HEIGHT.to_bits()
        );
        assert_eq!(
            row_height(RowLayout::Columns).to_bits(),
            ROW_HEIGHT.to_bits()
        );
    }

    #[test]
    fn numeric_header_starts_descending_and_same_header_toggles() {
        let first = Sort::DEFAULT.clicked(SortColumn::Size);
        assert_eq!(first.column, SortColumn::Size);
        assert!(!first.ascending);
        assert!(first.clicked(SortColumn::Size).ascending);
    }

    #[test]
    fn page_keys_use_at_least_one_row() {
        assert_eq!(page_delta("pageup", 0), Some(-1));
        assert_eq!(page_delta("pagedown", 8), Some(8));
        assert_eq!(page_delta("home", 8), None);
    }

    #[test]
    fn glyphs_distinguish_directories_links_and_files() {
        assert_ne!(kind_glyph(EntryKind::Dir), kind_glyph(EntryKind::File));
        assert_ne!(kind_glyph(EntryKind::Symlink), kind_glyph(EntryKind::File));
    }

    #[test]
    fn icon_view_is_a_wrapping_grid() {
        assert_eq!(icon_columns(0.), 1);
        assert_eq!(icon_columns(320.), 2);
        assert_eq!(icon_columns(500.), 3);
        assert_eq!(icon_rows(0, 3), 0);
        assert_eq!(icon_rows(7, 3), 3);
    }

    #[test]
    fn column_resize_stays_inside_finder_bounds() {
        assert_eq!(resized_width(100., -500.).to_bits(), 54_f32.to_bits());
        assert_eq!(resized_width(100., 25.).to_bits(), 125_f32.to_bits());
        assert_eq!(resized_width(800., 500.).to_bits(), 900_f32.to_bits());
    }
}
