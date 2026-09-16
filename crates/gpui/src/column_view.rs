//! Finder-style column view for the native GPUI browser.
//!
//! A column is an independent listing.  The view derives the columns from the
//! current share-relative path (`""`, `"Music"`, `"Music/Live"`, ...), so
//! changing directory naturally adds/removes panes and keeps ancestor context
//! visible while the user browses.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use excalibur_engine::backend::{Entry, EntryKind};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, uniform_list, Context, Div, EventEmitter, FocusHandle, MouseButton, MouseDownEvent,
    SharedString, Stateful, UniformListScrollHandle, Window,
};

use crate::browser::{self, Modifiers, Sort};
use crate::model::EntryRow;
use crate::services::Services;
use crate::theme::{self, Palette};

const COLUMN_WIDTH: f32 = 248.;
const ROW_HEIGHT: f32 = theme::geometry::ROW;

/// Events emitted by the column view.  The shell remains the owner of the
/// canonical `BrowserModel` selection and navigation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnViewEvent {
    SelectionChanged {
        paths: Vec<String>,
        modifiers: Modifiers,
    },
    Navigate {
        path: String,
    },
}

impl EventEmitter<ColumnViewEvent> for ColumnView {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PaneState {
    Loading,
    Ready(Vec<Entry>),
    Failed(String),
}

#[derive(Clone, Debug)]
struct Pane {
    path: String,
    state: PaneState,
    scroll: UniformListScrollHandle,
}

/// A horizontally scrollable sequence of independently loaded directory
/// panes.  Listing requests carry a generation so a slow ancestor response
/// cannot overwrite a newer navigation.
pub struct ColumnView {
    services: Arc<Services>,
    palette: Palette,
    path: String,
    sort: Sort,
    show_hidden: bool,
    folders_first: bool,
    panes: Vec<Pane>,
    generation: u64,
    selection: browser::Selection,
    focus: FocusHandle,
}

impl ColumnView {
    #[must_use]
    pub fn new(services: Arc<Services>, palette: Palette, cx: &mut Context<Self>) -> Self {
        Self {
            services,
            palette,
            path: String::new(),
            sort: Sort::DEFAULT,
            show_hidden: true,
            folders_first: true,
            panes: Vec::new(),
            generation: 0,
            selection: browser::Selection::default(),
            focus: cx.focus_handle(),
        }
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    pub fn set_preferences(
        &mut self,
        sort: Sort,
        show_hidden: bool,
        folders_first: bool,
        cx: &mut Context<Self>,
    ) {
        if self.sort != sort
            || self.show_hidden != show_hidden
            || self.folders_first != folders_first
        {
            self.sort = sort;
            self.show_hidden = show_hidden;
            self.folders_first = folders_first;
            cx.notify();
        }
    }

    pub fn set_selection(&mut self, paths: &HashSet<String>, cx: &mut Context<Self>) {
        if self.selection.paths != *paths {
            self.selection.paths.clone_from(paths);
            self.selection.anchor = paths.iter().next().cloned();
            cx.notify();
        }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Change the directory represented by the last column and load every
    /// ancestor in parallel.  The root column is always present.
    pub fn set_path(&mut self, path: impl Into<String>, cx: &mut Context<Self>) {
        let path = normalise_path(&path.into());
        if self.path == path && !self.panes.is_empty() {
            return;
        }
        self.path = path;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let paths = column_paths(&self.path);
        self.panes = paths
            .iter()
            .map(|path| Pane {
                path: path.clone(),
                state: PaneState::Loading,
                scroll: UniformListScrollHandle::new(),
            })
            .collect();

        for (index, pane_path) in paths.into_iter().enumerate() {
            let services = Arc::clone(&self.services);
            cx.spawn(async move |view, cx| {
                let result = services.list(pane_path).await;
                let _ = view.update(cx, |view, cx| {
                    if view.generation != generation {
                        return;
                    }
                    let Some(pane) = view.panes.get_mut(index) else {
                        return;
                    };
                    pane.state = match result {
                        Ok(listing) => PaneState::Ready(listing.entries),
                        Err(error) => PaneState::Failed(error.to_string()),
                    };
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn entries(&self, index: usize) -> Vec<Entry> {
        let Some(Pane {
            state: PaneState::Ready(entries),
            ..
        }) = self.panes.get(index)
        else {
            return Vec::new();
        };
        let filtered = entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.hidden)
            .cloned()
            .collect::<Vec<_>>();
        browser::sort_entries(&filtered, self.sort, self.folders_first)
    }

    fn rows(&self, index: usize) -> Vec<EntryRow> {
        self.entries(index)
            .into_iter()
            .map(EntryRow::from)
            .collect()
    }

    fn row_mouse_down(
        &mut self,
        pane_index: usize,
        row_index: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.rows(pane_index);
        let Some(row) = rows.get(row_index).cloned() else {
            return;
        };
        let order = rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>();
        let modifiers = Modifiers {
            shift: event.modifiers.shift,
            toggle: event.modifiers.secondary(),
        };
        self.selection = browser::click(&self.selection, &order, &row.path, modifiers);
        self.focus.focus(window, cx);
        if row.kind == EntryKind::Dir {
            let path = row.path.clone();
            self.set_path(path.clone(), cx);
            cx.emit(ColumnViewEvent::Navigate { path });
        } else {
            cx.emit(ColumnViewEvent::SelectionChanged {
                paths: self.selected_paths(),
                modifiers,
            });
        }
        cx.stop_propagation();
    }

    fn selected_paths(&self) -> Vec<String> {
        self.selection.paths.iter().cloned().collect::<Vec<_>>()
    }

    fn render_rows(
        &mut self,
        pane_index: usize,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let palette = self.palette;
        let rows = self.rows(pane_index);
        let next_path = self.panes.get(pane_index + 1).map(|pane| pane.path.clone());
        range
            .filter_map(|index| rows.get(index).cloned().map(|row| (index, row)))
            .map(|(index, row)| {
                let path = row.path.clone();
                let selected = self.selection.paths.contains(&path);
                let on_path = next_path.as_deref() == Some(path.as_str());
                let hidden = row.hidden;
                let directory = row.kind == EntryKind::Dir;
                let glyph = if directory { "▰" } else { "▪" };
                div()
                    .id(SharedString::from(format!(
                        "column-row-{pane_index}-{path}"
                    )))
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(9.))
                    .opacity(if hidden { 0.62 } else { 1. })
                    .when(selected || on_path, |row| {
                        row.bg(theme::color(palette.accent_soft))
                            .border_l_2()
                            .border_color(theme::color(palette.accent))
                    })
                    .when(!selected && !on_path, |row| {
                        row.hover(|style| style.bg(theme::color(palette.accent_soft)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.row_mouse_down(pane_index, index, event, window, cx);
                        }),
                    )
                    .child(theme::text(
                        glyph,
                        12.,
                        theme::color(if directory {
                            palette.folder
                        } else {
                            palette.file
                        }),
                    ))
                    .child(
                        theme::text(row.name, 12., theme::color(palette.text_primary)).truncate(),
                    )
                    .when(directory, |row| {
                        row.child(theme::text("›", 14., theme::color(palette.text_tertiary)))
                    })
            })
            .collect()
    }

    fn render_pane(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(pane) = self.panes.get(index) else {
            return div().id(SharedString::from(format!("column-pane-{index}")));
        };
        let path = pane.path.clone();
        let state = pane.state.clone();
        let scroll = pane.scroll.clone();
        let palette = self.palette;
        let mut column = div()
            .id(SharedString::from(format!("column-pane-{index}")))
            .w(px(COLUMN_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::color(palette.chrome_line))
            .bg(theme::color(palette.list))
            .child(
                div()
                    .h(px(theme::geometry::COLUMN_HEADER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .bg(theme::color(palette.toolbar))
                    .border_b_1()
                    .border_color(theme::color(palette.chrome_line))
                    .child(theme::mono(
                        if path.is_empty() { "Share root" } else { &path },
                        9.5,
                        theme::color(palette.text_secondary),
                    )),
            );
        match state {
            PaneState::Loading => {
                column = column.child(theme::text(
                    "Loading…",
                    11.,
                    theme::color(palette.text_disabled),
                ));
            }
            PaneState::Failed(error) => {
                column = column.child(div().p(px(10.)).child(theme::text(
                    error,
                    11.,
                    theme::color(palette.danger),
                )));
            }
            PaneState::Ready(entries) => {
                let count = self.rows(index).len();
                // Keep `entries` live through this branch: it documents that
                // the list is ready while the prepared rows are derived below.
                let _ = entries;
                column = column.child(
                    uniform_list(
                        SharedString::from(format!("column-rows-{index}")),
                        count,
                        cx.processor(move |this, range: Range<usize>, window, cx| {
                            this.render_rows(index, range, window, cx)
                        }),
                    )
                    .track_scroll(&scroll)
                    .flex_1()
                    .min_h_0(),
                );
            }
        }
        column
    }
}

impl Render for ColumnView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div()
            .id("column-view-strip")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .overflow_x_scroll()
            .track_focus(&self.focus)
            .bg(theme::color(self.palette.list));
        for index in 0..self.panes.len() {
            strip = strip.child(self.render_pane(index, window, cx));
        }
        strip
    }
}

/// Build all paths represented by a column view, always including share root.
#[must_use]
pub fn column_paths(path: &str) -> Vec<String> {
    let mut paths = vec![String::new()];
    let mut current = String::new();
    for segment in path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        if segment == ".." {
            current = current
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned());
            continue;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        paths.push(current.clone());
    }
    paths
}

fn normalise_path(path: &str) -> String {
    column_paths(path).last().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{column_paths, normalise_path};

    #[test]
    fn columns_start_at_root_and_include_each_ancestor() {
        assert_eq!(
            column_paths("Music/Live/2026"),
            ["", "Music", "Music/Live", "Music/Live/2026"]
        );
    }

    #[test]
    fn paths_are_share_relative_and_safe() {
        assert_eq!(normalise_path("/Music//Live/../Studio/"), "Music/Studio");
        assert_eq!(column_paths("../../"), [""]);
    }
}
