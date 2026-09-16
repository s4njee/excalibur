//! Metadata-only Properties modal for the browser.
//!
//! The modal deliberately works on a projection of [`Entry`].  An engine
//! entry also has an opaque change cookie; keeping only display metadata here
//! makes it clear that opening Properties never reads (or retains) file
//! bytes.  Mutating and filesystem-facing operations remain events for the
//! shell/model to authorize.

use excalibur_engine::backend::{Entry, EntryKind, MountInfo, MountStatus};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, rgba, ClickEvent, Context, Div, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    TextAlign, Window,
};

use crate::browser;
use crate::theme::{self, Palette};

const DIALOG_WIDTH: f32 = 470.;

/// The safe actions a shell may handle for a Properties modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertiesEvent {
    /// Dismiss the modal without performing an operation.
    Close,
    /// Ask the owner to reveal the selected share-relative paths.
    ///
    /// The event does not touch the filesystem. The owner can map paths to a
    /// local reveal action only when the active backend supports it.
    Reveal { paths: Vec<String> },
    /// Ask the owner to begin a rename workflow for one selected entry.
    ///
    /// This is an intent, not a write. The model must re-check writability and
    /// perform the rename through the engine.
    RenameRequested { path: String, current_name: String },
}

impl EventEmitter<PropertiesEvent> for Properties {}

/// Metadata retained by the modal for one selected entry.
///
/// This intentionally excludes [`excalibur_engine::backend::Attrs::change_cookie`]
/// and all file contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub kind_label: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub added_ms: Option<i64>,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
}

impl From<&Entry> for PropertyEntry {
    fn from(entry: &Entry) -> Self {
        Self {
            name: entry.name.clone(),
            path: entry.path.clone(),
            kind: entry.attrs.kind,
            kind_label: entry.kind_label.clone(),
            size: entry.attrs.size,
            modified_ms: entry.attrs.modified_ms,
            added_ms: entry.attrs.added_ms,
            mode: entry.attrs.mode,
            link_target: entry.attrs.link_target.clone(),
        }
    }
}

/// Aggregate values shown for a multiple-entry selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionSummary {
    /// Number of non-directory entries (symlinks are counted as files).
    pub files: usize,
    pub folders: usize,
    /// Sum of file-like entry sizes; directories are intentionally excluded.
    pub bytes: u64,
}

/// Summarise metadata without walking directories or reading file contents.
#[must_use]
pub fn summarise(entries: &[PropertyEntry]) -> SelectionSummary {
    let mut summary = SelectionSummary::default();
    for entry in entries {
        if entry.kind == EntryKind::Dir {
            summary.folders += 1;
        } else {
            summary.files += 1;
            summary.bytes = summary.bytes.saturating_add(entry.size);
        }
    }
    summary
}

/// Return the newest known modification time in a selection.
#[must_use]
pub fn newest_modified(entries: &[PropertyEntry]) -> Option<i64> {
    entries.iter().filter_map(|entry| entry.modified_ms).max()
}

/// Format an exact byte count with stable comma grouping.
#[must_use]
pub fn format_exact_bytes(bytes: u64) -> String {
    let digits = bytes.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {}", if bytes == 1 { "byte" } else { "bytes" })
}

/// Format the share-relative location shown for a selection.
#[must_use]
pub fn location_label(location: &str, share_name: &str) -> String {
    let trimmed = location.trim_matches('/');
    if trimmed.is_empty() {
        share_name.to_owned()
    } else {
        format!("{share_name}/{trimmed}")
    }
}

/// Mark an entry path as relative to the share root.
#[must_use]
pub fn share_path(path: &str) -> String {
    format!("/{}", path.trim_start_matches('/'))
}

/// Format the count line used for a multi-selection.
#[must_use]
pub fn format_count_line(summary: SelectionSummary) -> String {
    let mut parts = Vec::with_capacity(2);
    if summary.files > 0 {
        parts.push(format!(
            "{} file{}",
            summary.files,
            if summary.files == 1 { "" } else { "s" }
        ));
    }
    if summary.folders > 0 {
        parts.push(format!(
            "{} folder{}",
            summary.folders,
            if summary.folders == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        "Nothing selected".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Convert mode bits to the familiar octal representation.
#[must_use]
pub fn format_mode(mode: Option<u32>) -> String {
    mode.map_or_else(|| "—".to_owned(), |mode| format!("{mode:o}"))
}

/// Human-readable mount access state for the modal.
#[must_use]
pub fn access_label(mount: Option<&MountInfo>) -> String {
    let Some(mount) = mount else {
        return "Unknown".to_owned();
    };
    if mount.writable {
        "Read & write".to_owned()
    } else if mount.reason.trim().is_empty() {
        "Read-only".to_owned()
    } else {
        format!("Read-only — {}", mount.reason)
    }
}

/// Metadata-only Properties dialog.
pub struct Properties {
    entries: Vec<PropertyEntry>,
    location: String,
    share_name: String,
    mount: Option<MountInfo>,
    palette: Palette,
    focus: FocusHandle,
}

impl Properties {
    /// Create a modal from engine entries and the current mount facts.
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        palette: Palette,
        entries: impl IntoIterator<Item = Entry>,
        location: impl Into<String>,
        share_name: impl Into<String>,
        mount: Option<MountInfo>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            entries: entries
                .into_iter()
                .map(|entry| PropertyEntry::from(&entry))
                .collect(),
            location: location.into(),
            share_name: share_name.into(),
            mount,
            palette,
            focus,
        }
    }

    /// Construct directly from already-projected metadata, useful for a model
    /// that has discarded engine-only fields before creating the view.
    pub fn from_metadata(
        window: &mut Window,
        cx: &mut Context<Self>,
        palette: Palette,
        entries: Vec<PropertyEntry>,
        location: impl Into<String>,
        share_name: impl Into<String>,
        mount: Option<MountInfo>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            entries,
            location: location.into(),
            share_name: share_name.into(),
            mount,
            palette,
            focus,
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[PropertyEntry] {
        &self.entries
    }

    #[must_use]
    pub fn mount(&self) -> Option<&MountInfo> {
        self.mount.as_ref()
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    pub fn close(&self, cx: &mut Context<Self>) {
        cx.emit(PropertiesEvent::Close);
    }

    /// Emit a reveal intent for all selected paths. No filesystem operation is
    /// attempted by this view.
    pub fn reveal(&self, cx: &mut Context<Self>) {
        cx.emit(PropertiesEvent::Reveal {
            paths: self
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
        });
    }

    /// Emit a rename intent only when exactly one item is selected. Writability
    /// is still checked by the model when it handles this event.
    pub fn request_rename(&self, cx: &mut Context<Self>) {
        if let [entry] = self.entries.as_slice() {
            cx.emit(PropertiesEvent::RenameRequested {
                path: entry.path.clone(),
                current_name: entry.name.clone(),
            });
        }
    }

    fn detail_row(&self, label: &str, value: String, mono: bool) -> Div {
        let p = self.palette;
        div()
            .flex()
            .items_start()
            .gap(px(12.))
            .child(
                div()
                    .w(px(92.))
                    .flex_none()
                    .text_align(TextAlign::Right)
                    .child(theme::text(label, 11., theme::color(p.text_secondary))),
            )
            .child(if mono {
                theme::mono(value, 10.5, theme::color(p.text_primary))
            } else {
                theme::text(value, 11., theme::color(p.text_primary))
            })
    }

    fn render_single(&self, entry: &PropertyEntry) -> Div {
        let mut body = div().flex().flex_col().gap(px(8.));
        body = body.child(self.detail_row("Kind", entry.kind_label.clone(), false));
        if entry.kind == EntryKind::Dir {
            body = body.child(self.detail_row("Size", "—".to_owned(), true));
            body = body.child(theme::text(
                "A folder's contents are not measured — that would mean reading the whole tree.",
                10.,
                theme::color(self.palette.text_tertiary),
            ));
        } else {
            body = body.child(self.detail_row(
                "Size",
                format!(
                    "{} ({})",
                    browser::format_size(entry.size),
                    format_exact_bytes(entry.size)
                ),
                true,
            ));
        }
        body = body
            .child(self.detail_row(
                "Modified",
                browser::format_modified(entry.modified_ms),
                false,
            ))
            .child(self.detail_row("Added", browser::format_modified(entry.added_ms), false))
            .child(self.detail_row("Mode", format_mode(entry.mode), true))
            .child(self.detail_row("Path", share_path(&entry.path), true));
        if let Some(target) = &entry.link_target {
            body = body.child(self.detail_row("Links to", target.clone(), true));
        }
        body
    }

    fn render_mount(&self) -> Div {
        let Some(mount) = &self.mount else {
            return div();
        };
        let status = match mount.status {
            MountStatus::Mounting => "Connecting",
            MountStatus::Ready => "Ready",
            MountStatus::Error => "Error",
            MountStatus::Reconnecting => "Reconnecting",
        };
        div()
            .mt(px(4.))
            .pt(px(10.))
            .border_t_1()
            .border_color(theme::color(self.palette.chrome_line))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.detail_row("Protocol", mount.protocol_label.clone(), false))
            .child(self.detail_row("Access", access_label(Some(mount)), false))
            .child(self.detail_row("Status", status.to_owned(), false))
    }

    fn render_selection_body(&self, single: bool, summary: SelectionSummary, heading: &str) -> Div {
        let mut body = div().flex().flex_col().gap(px(8.));
        if let Some(entry) = self.entries.first().filter(|_| single) {
            body = body.child(self.render_single(entry));
        } else {
            body = body
                .child(self.detail_row("Items", heading.to_owned(), false))
                .child(self.detail_row(
                    "Size",
                    format!(
                        "{} ({})",
                        browser::format_size(summary.bytes),
                        format_exact_bytes(summary.bytes)
                    ),
                    true,
                ));
            if summary.folders > 0 {
                body = body.child(theme::text(
                    "Folders are not included — their contents are not measured.",
                    10.,
                    theme::color(self.palette.text_tertiary),
                ));
            }
            body = body
                .child(self.detail_row(
                    "Modified",
                    browser::format_modified(newest_modified(&self.entries)),
                    false,
                ))
                .child(self.detail_row(
                    "Where",
                    location_label(&self.location, &self.share_name),
                    true,
                ));
        }
        body.child(self.render_mount())
    }

    fn render_actions(&self, single: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let can_reveal = self
            .mount
            .as_ref()
            .is_some_and(|mount| mount.protocol_label.to_ascii_lowercase().contains("local"));
        let can_rename = single && self.mount.as_ref().is_some_and(|mount| mount.writable);
        let mut actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(7.))
            .pt(px(5.))
            .border_t_1()
            .border_color(theme::color(p.chrome_line));
        if can_reveal {
            actions = actions.child(
                Button::new("properties-reveal")
                    .ghost()
                    .label("Reveal")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reveal(cx))),
            );
        }
        if can_rename {
            actions = actions.child(
                Button::new("properties-rename")
                    .ghost()
                    .label("Rename…")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.request_rename(cx))),
            );
        }
        actions.child(
            Button::new("properties-close")
                .secondary()
                .label("Done")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close(cx))),
        )
    }
}

impl Render for Properties {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let single = self.entries.len() == 1;
        let summary = summarise(&self.entries);
        let heading = if single {
            self.entries[0].name.clone()
        } else {
            format_count_line(summary)
        };
        let body = self.render_selection_body(single, summary, &heading);
        let actions = self.render_actions(single, cx);

        div()
            .id("properties-backdrop")
            .track_focus(&self.focus)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_008c))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            .child(
                div()
                    .id("properties-dialog")
                    .w(px(DIALOG_WIDTH))
                    .flex()
                    .flex_col()
                    .bg(theme::color(p.toolbar))
                    .border_1()
                    .border_color(theme::color(p.chrome_line))
                    .rounded(px(6.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_b_1()
                            .border_color(theme::color(p.chrome_line))
                            .child(theme::text("Properties", 12., theme::color(p.text_primary))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .p(px(18.))
                            .child(theme::text(heading, 13., theme::color(p.text_primary)))
                            .child(body)
                            .child(actions),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        access_label, format_count_line, format_exact_bytes, format_mode, location_label,
        newest_modified, share_path, summarise, EntryKind, PropertyEntry, SelectionSummary,
    };

    fn entry(name: &str, kind: EntryKind, size: u64, modified_ms: Option<i64>) -> PropertyEntry {
        PropertyEntry {
            name: name.to_owned(),
            path: format!("dir/{name}"),
            kind,
            kind_label: if kind == EntryKind::Dir {
                "Folder"
            } else {
                "Document"
            }
            .to_owned(),
            size,
            modified_ms,
            added_ms: None,
            mode: None,
            link_target: None,
        }
    }

    #[test]
    fn summary_counts_directories_without_measuring_them() {
        let entries = [
            entry("a", EntryKind::File, 5, Some(10)),
            entry("b", EntryKind::Dir, 999, Some(20)),
            entry("c", EntryKind::Symlink, 7, None),
        ];
        assert_eq!(
            summarise(&entries),
            SelectionSummary {
                files: 2,
                folders: 1,
                bytes: 12,
            }
        );
        assert_eq!(newest_modified(&entries), Some(20));
    }

    #[test]
    fn count_line_omits_empty_categories() {
        assert_eq!(
            format_count_line(SelectionSummary {
                files: 1,
                folders: 0,
                bytes: 1,
            }),
            "1 file"
        );
        assert_eq!(
            format_count_line(SelectionSummary {
                files: 0,
                folders: 2,
                bytes: 0,
            }),
            "2 folders"
        );
        assert_eq!(
            format_count_line(SelectionSummary::default()),
            "Nothing selected"
        );
    }

    #[test]
    fn exact_bytes_are_grouped_and_pluralized() {
        assert_eq!(format_exact_bytes(0), "0 bytes");
        assert_eq!(format_exact_bytes(1), "1 byte");
        assert_eq!(format_exact_bytes(48_234_496_000), "48,234,496,000 bytes");
    }

    #[test]
    fn paths_and_locations_are_share_relative() {
        assert_eq!(share_path("/media/a"), "/media/a");
        assert_eq!(share_path("a"), "/a");
        assert_eq!(location_label("///", "media"), "media");
        assert_eq!(
            location_label("/Documentaries/", "media"),
            "media/Documentaries"
        );
    }

    #[test]
    fn mode_and_access_formats_are_honest() {
        assert_eq!(format_mode(None), "—");
        assert_eq!(format_mode(Some(0o755)), "755");
        assert_eq!(access_label(None), "Unknown");
    }
}
