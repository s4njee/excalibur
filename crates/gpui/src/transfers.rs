//! Virtualised transfer queue panel for the native shell.
//!
//! This view consumes only [`excalibur_engine::Transfer`] metadata. It never
//! opens a source or destination and therefore keeps file bytes behind the
//! engine service boundary. Queue mutation is reported as typed events; the
//! model remains the owner of the durable queue and workers.

use std::ops::Range;
use std::sync::Arc;

use excalibur_engine::{Direction, NamedQueue, Transfer, TransferStatus};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, relative, uniform_list, ClickEvent, Context, Div, EventEmitter, FocusHandle,
    SharedString, Stateful, UniformListScrollHandle, Window,
};

use crate::browser;
use crate::theme::{self, Palette};

/// Fixed row height used by the virtual list. Two lines keep destination and
/// recovery text legible without requiring variable-height virtualization.
pub const ROW_HEIGHT: f32 = 54.;
const HEADER_HEIGHT: f32 = 32.;
const STATUS_WIDTH: f32 = 170.;
const PROGRESS_WIDTH: f32 = 190.;
const ACTIONS_WIDTH: f32 = 108.;

/// Queue actions emitted by [`TransfersPanel`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransfersEvent {
    PauseAll,
    ResumeAll,
    ClearFinished,
    Retry { id: String },
    Cancel { id: String },
    Pause { id: String },
}

impl EventEmitter<TransfersEvent> for TransfersPanel {}

/// A metadata-only transfer queue panel.
pub struct TransfersPanel {
    transfers: Arc<[Transfer]>,
    queues: Arc<[NamedQueue]>,
    palette: Palette,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

/// Alias for callers that prefer the shorter component name.
pub type TransferPanel = TransfersPanel;

/// The row's progress fraction, or `None` when the total is unknown.
#[must_use]
pub fn progress_fraction(transfer: &Transfer) -> Option<f32> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "A progress fraction is intentionally reduced to GPUI's f32 layout precision"
    )]
    (transfer.bytes_total > 0)
        .then(|| (transfer.bytes_done as f32 / transfer.bytes_total as f32).clamp(0., 1.))
}

/// The direction chip shown beside a transfer.
#[must_use]
pub fn direction_label(transfer: &Transfer) -> &'static str {
    match transfer.direction {
        Some(Direction::Up) => "UPLOAD",
        Some(Direction::Down) => "DOWNLOAD",
        None => match transfer.from.as_ref() {
            Some(excalibur_engine::transfer::Endpoint::Http { .. }) => "DOWNLOAD",
            _ => "TRANSFER",
        },
    }
}

/// The status wording used by the row, including verified-vs-done semantics.
#[must_use]
pub fn format_status(transfer: &Transfer) -> String {
    let files_total = transfer.files_total;
    match transfer.status {
        TransferStatus::Queued => transfer
            .note
            .as_deref()
            .map_or_else(|| "queued".to_owned(), |note| format!("queued · {note}")),
        TransferStatus::Paused => "paused".to_owned(),
        TransferStatus::Cancelled => "cancelled".to_owned(),
        TransferStatus::Done => {
            let word = if transfer.verified {
                "verified"
            } else {
                "done"
            };
            let what = transfer
                .note
                .as_deref()
                .map_or_else(|| browser::format_size(transfer.bytes_done), str::to_owned);
            if files_total > 1 {
                format!("{word} · {files_total} files · {what}")
            } else {
                format!("{word} · {what}")
            }
        }
        TransferStatus::Failed => "failed".to_owned(),
        TransferStatus::Running => {
            let mut parts = Vec::new();
            if transfer.bytes_total > 0 {
                let total = browser::format_size(transfer.bytes_total);
                let unit = total
                    .split_once(' ')
                    .map_or_else(String::new, |(_, unit)| format!(" {unit}"));
                let done = browser::format_size(transfer.bytes_done);
                let done_number = done
                    .strip_suffix(&unit)
                    .map_or(done.as_str(), str::trim_end);
                parts.push(format!("{done_number} / {total}"));
            } else {
                parts.push(browser::format_size(transfer.bytes_done));
            }
            if files_total > 1 {
                parts.push(format!("{} / {files_total} files", transfer.files_done));
            }
            if let Some(rate) = transfer.rate.filter(|rate| *rate > 0) {
                parts.push(browser::format_rate(rate));
            }
            if let Some(eta) = transfer.eta_seconds {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "ETA is displayed as a human-readable floating-point duration"
                )]
                let eta = eta as f64;
                parts.push(browser::format_duration(eta));
            }
            parts.join(" · ")
        }
    }
}

/// Progress text beside the bar. Unknown totals intentionally have no fake %.
#[must_use]
pub fn format_progress(transfer: &Transfer) -> String {
    if transfer.status == TransferStatus::Queued {
        return "queued".to_owned();
    }
    let Some(fraction) = progress_fraction(transfer) else {
        return browser::format_size(transfer.bytes_done);
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "progress_fraction is clamped to 0..=1 before converting to a percentage"
    )]
    let percent = format!("{}%", (fraction * 100.).round() as u8);
    if transfer.status != TransferStatus::Running {
        percent
    } else if let Some(rate) = transfer.rate.filter(|rate| *rate > 0) {
        format!("{percent} · {}", browser::format_rate(rate))
    } else {
        percent
    }
}

/// Source/destination path shown below the transfer name.
#[must_use]
pub fn destination_label(transfer: &Transfer) -> String {
    if !transfer.dest_path.is_empty() {
        return transfer.dest_path.clone();
    }
    transfer
        .to
        .as_ref()
        .map_or_else(String::new, |endpoint| match endpoint {
            excalibur_engine::transfer::Endpoint::Local { path }
            | excalibur_engine::transfer::Endpoint::Share { path, .. } => path.clone(),
            excalibur_engine::transfer::Endpoint::Http { url } => url.clone(),
        })
}

/// Name of the queue responsible for a row, with a useful fallback for
/// restored rows whose named-queue definition is not present yet.
#[must_use]
pub fn queue_label(transfer: &Transfer, queues: &[NamedQueue]) -> String {
    queues
        .iter()
        .find(|queue| queue.id == transfer.queue_id)
        .map_or_else(
            || {
                if transfer.queue_id == "default" {
                    "Default".to_owned()
                } else {
                    transfer.queue_id.clone()
                }
            },
            |queue| queue.name.clone(),
        )
}

/// Footer aggregate for the rows currently displayed.
#[must_use]
pub fn footer_summary(transfers: &[Transfer]) -> String {
    let active = transfers
        .iter()
        .filter(|transfer| transfer.status == TransferStatus::Running)
        .count();
    let queued = transfers
        .iter()
        .filter(|transfer| transfer.status == TransferStatus::Queued)
        .count();
    let failed = transfers
        .iter()
        .filter(|transfer| transfer.status == TransferStatus::Failed)
        .count();
    let remaining = transfers
        .iter()
        .filter(|transfer| transfer.status == TransferStatus::Running)
        .map(|transfer| transfer.bytes_total.saturating_sub(transfer.bytes_done))
        .sum::<u64>();
    format!(
        "{active} active · {queued} queued · {failed} failed · {} remaining",
        browser::format_size(remaining)
    )
}

impl TransfersPanel {
    /// Construct an empty panel; install queue snapshots with [`set_transfers`].
    #[must_use]
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            transfers: Arc::from(Vec::new()),
            queues: Arc::from(Vec::new()),
            palette: Palette::light(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Construct a panel with a transfer snapshot and optional named queues.
    #[must_use]
    pub fn with_transfers(
        window: &mut Window,
        cx: &mut Context<Self>,
        transfers: Vec<Transfer>,
        queues: Vec<NamedQueue>,
    ) -> Self {
        let mut panel = Self::new(window, cx);
        panel.transfers = Arc::from(transfers);
        panel.queues = Arc::from(queues);
        panel
    }

    pub fn set_transfers(&mut self, transfers: Vec<Transfer>, cx: &mut Context<Self>) {
        self.transfers = Arc::from(transfers);
        cx.notify();
    }

    pub fn set_queues(&mut self, queues: Vec<NamedQueue>, cx: &mut Context<Self>) {
        self.queues = Arc::from(queues);
        cx.notify();
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    #[must_use]
    pub fn transfers(&self) -> &[Transfer] {
        &self.transfers
    }

    #[must_use]
    pub fn scroll_handle(&self) -> UniformListScrollHandle {
        self.scroll.clone()
    }

    fn emit(event: TransfersEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    fn render_action(
        label: &'static str,
        event: TransfersEvent,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(SharedString::from(format!(
                "transfer-action-{label}-{event:?}"
            )))
            .h(px(22.))
            .px(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(px(3.))
            .border_1()
            .border_color(theme::color(palette.control_border))
            .hover(|style| style.bg(theme::color(palette.accent_soft)))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| Self::emit(event.clone(), cx)))
            .child(theme::mono(label, 9., theme::color(palette.text_secondary)))
    }

    fn render_header(palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("transfers-header")
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .bg(theme::color(palette.toolbar))
            .border_b_1()
            .border_color(theme::color(palette.chrome_line))
            .child(theme::text(
                "TRANSFERS",
                10.,
                theme::color(palette.text_primary),
            ))
            .child(div().flex_1())
            .child(Self::render_action(
                "Pause all",
                TransfersEvent::PauseAll,
                palette,
                cx,
            ))
            .child(Self::render_action(
                "Resume all",
                TransfersEvent::ResumeAll,
                palette,
                cx,
            ))
            .child(Self::render_action(
                "Clear finished",
                TransfersEvent::ClearFinished,
                palette,
                cx,
            ))
    }

    fn render_transfer_actions(
        status: TransferStatus,
        id: String,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let actions = div()
            .flex_none()
            .w(px(ACTIONS_WIDTH))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(4.));
        match status {
            TransferStatus::Running => actions.child(Self::render_action(
                "Pause",
                TransfersEvent::Pause { id },
                palette,
                cx,
            )),
            TransferStatus::Failed => actions
                .child(Self::render_action(
                    "Retry",
                    TransfersEvent::Retry { id: id.clone() },
                    palette,
                    cx,
                ))
                .child(Self::render_action(
                    "Cancel",
                    TransfersEvent::Cancel { id },
                    palette,
                    cx,
                )),
            TransferStatus::Queued | TransferStatus::Paused => actions.child(Self::render_action(
                "Cancel",
                TransfersEvent::Cancel { id },
                palette,
                cx,
            )),
            TransferStatus::Done | TransferStatus::Cancelled => actions,
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let palette = self.palette;
        range
            .filter_map(|index| self.transfers.get(index).cloned().map(|row| (index, row)))
            .map(|(_index, transfer)| {
                let status = format_status(&transfer);
                let progress = format_progress(&transfer);
                let destination = destination_label(&transfer);
                let queue = queue_label(&transfer, &self.queues);
                let direction = direction_label(&transfer);
                let status_colour = match transfer.status {
                    TransferStatus::Failed => palette.danger,
                    TransferStatus::Done => palette.success,
                    TransferStatus::Running => palette.accent,
                    _ => palette.text_secondary,
                };
                let id = transfer.id.clone();
                let mut row = div()
                    .id(SharedString::from(format!("transfer-row-{id}")))
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .border_b_1()
                    .border_color(theme::color(palette.row_line));
                let name = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(theme::text(
                        transfer.name.clone(),
                        11.5,
                        theme::color(palette.text_primary),
                    ))
                    .child(theme::mono(
                        if destination.is_empty() {
                            format!("{direction} · {queue}")
                        } else {
                            format!("{destination} · {queue}")
                        },
                        9.,
                        theme::color(palette.text_tertiary),
                    ));
                row = row.child(name);
                row = row.child(
                    div()
                        .flex_none()
                        .w(px(PROGRESS_WIDTH))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(3.))
                        .child(progress_bar(&transfer, palette))
                        .child(theme::mono(
                            progress,
                            9.,
                            theme::color(palette.text_secondary),
                        )),
                );
                row = row.child(
                    theme::mono(status, 9.5, theme::color(status_colour))
                        .flex_none()
                        .w(px(STATUS_WIDTH))
                        .truncate(),
                );
                row.child(Self::render_transfer_actions(
                    transfer.status,
                    id,
                    palette,
                    cx,
                ))
            })
            .collect()
    }
}

fn progress_bar(transfer: &Transfer, palette: Palette) -> Div {
    let fraction = progress_fraction(transfer).unwrap_or(0.);
    div()
        .h(px(5.))
        .w_full()
        .rounded(px(3.))
        .bg(theme::color(palette.row_line))
        .child(
            div()
                .h_full()
                .w(relative(fraction))
                .rounded(px(3.))
                .bg(theme::color(match transfer.status {
                    TransferStatus::Failed => palette.danger,
                    TransferStatus::Done => palette.success,
                    _ => palette.accent,
                })),
        )
}

impl Render for TransfersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        div()
            .id("transfers-panel")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::color(palette.list))
            .track_focus(&self.focus)
            .child(Self::render_header(palette, cx))
            .child(
                uniform_list(
                    "transfers-rows",
                    self.transfers.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        this.render_rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(
                div()
                    .id("transfers-footer")
                    .h(px(theme::geometry::STATUS_BAR))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .border_t_1()
                    .border_color(theme::color(palette.chrome_line))
                    .child(theme::mono(
                        footer_summary(&self.transfers),
                        9.5,
                        theme::color(palette.text_secondary),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        destination_label, direction_label, footer_summary, format_status, progress_fraction,
        queue_label, Direction, NamedQueue, Transfer, TransferStatus,
    };

    fn transfer(status: TransferStatus) -> Transfer {
        let mut value = Transfer::new(
            "id",
            Direction::Down,
            "/source",
            "/dest/file.bin",
            1_000_000,
        );
        value.status = status;
        value
    }

    #[test]
    fn progress_is_unknown_without_a_total_and_clamped_with_one() {
        let mut value = transfer(TransferStatus::Running);
        value.bytes_total = 0;
        assert_eq!(progress_fraction(&value), None);
        value.bytes_total = 100;
        value.bytes_done = 150;
        assert_eq!(progress_fraction(&value), Some(1.));
    }

    #[test]
    fn status_says_verified_and_preserves_folder_counts() {
        let mut value = transfer(TransferStatus::Done);
        value.verified = true;
        value.bytes_done = value.bytes_total;
        value.files_done = 12;
        value.files_total = 12;
        assert_eq!(format_status(&value), "verified · 12 files · 1 MB");
    }

    #[test]
    fn running_status_contains_rate_and_eta() {
        let mut value = transfer(TransferStatus::Running);
        value.bytes_done = 500_000;
        value.rate = Some(10_000);
        value.eta_seconds = Some(61);
        assert_eq!(format_status(&value), "500 KB / 1 MB · 10 KB/s · 1m 01s");
    }

    #[test]
    fn footer_counts_running_queued_failed_and_remaining() {
        let mut failed = transfer(TransferStatus::Failed);
        failed.id = "failed".to_owned();
        let mut queued = transfer(TransferStatus::Queued);
        queued.id = "queued".to_owned();
        let mut running = transfer(TransferStatus::Running);
        running.id = "running".to_owned();
        running.bytes_done = 250_000;
        assert_eq!(
            footer_summary(&[failed, queued, running]),
            "1 active · 1 queued · 1 failed · 750 KB remaining"
        );
    }

    #[test]
    fn queue_name_falls_back_to_id_without_secrets() {
        let value = transfer(TransferStatus::Queued);
        assert_eq!(queue_label(&value, &[]), "Default");
        let queue = NamedQueue {
            id: "nightly".to_owned(),
            name: "Nightly copies".to_owned(),
            color: "blue".to_owned(),
            concurrency: 1,
            order: 0,
        };
        let mut value = value;
        value.queue_id = queue.id.clone();
        assert_eq!(queue_label(&value, &[queue]), "Nightly copies");
    }

    #[test]
    fn direction_and_destination_are_metadata_only() {
        let value = transfer(TransferStatus::Queued);
        assert_eq!(direction_label(&value), "DOWNLOAD");
        assert_eq!(destination_label(&value), "/dest/file.bin");
    }
}
