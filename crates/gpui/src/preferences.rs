//! Native Preferences modal for the browser view.
//!
//! Preferences are deliberately limited to presentation state. This dialog
//! never opens a connection, handles credentials, or performs persistence;
//! [`PreferencesEvent::Apply`] hands a validated [`Settings`] value to the
//! shell, which can save it through `SettingsStore`.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::Disableable;
use gpui_kit::component::Selectable;
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, rgba, ClickEvent, Context, Div, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    SharedString, Stateful, TextAlign, Window,
};

use crate::settings::{PaneMode, RowLayoutPreference, Settings, ThemeChoice};
use crate::theme::{self, Palette};

const DIALOG_WIDTH: f32 = 500.;
const SIDEBAR_MIN: u32 = 160;
const SIDEBAR_MAX: u32 = 420;
const SIDEBAR_STEP: u32 = 8;
const SPLIT_MIN: f32 = 0.2;
const SPLIT_MAX: f32 = 0.8;
const SPLIT_STEP: f32 = 0.05;

/// The row choices shown in Preferences.  Keep the copy and the miniature
/// preview together with the persisted value so adding a new presentation
/// cannot accidentally leave the chooser describing another one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RowLayoutOption {
    layout: RowLayoutPreference,
    slug: &'static str,
    label: &'static str,
    description: &'static str,
    preview: &'static str,
}

const ROW_LAYOUT_OPTIONS: [RowLayoutOption; 5] = [
    RowLayoutOption {
        layout: RowLayoutPreference::Columns,
        slug: "columns",
        label: "Columns",
        description: "macOS-style details",
        preview: "▤  NAME    SIZE",
    },
    RowLayoutOption {
        layout: RowLayoutPreference::NameLed,
        slug: "name-led",
        label: "Name-led",
        description: "Name with inline details",
        preview: "▣  NAME — details",
    },
    RowLayoutOption {
        layout: RowLayoutPreference::Mono,
        slug: "mono",
        label: "Compact",
        description: "Dense text rows",
        preview: "01  NAME    SIZE",
    },
    RowLayoutOption {
        layout: RowLayoutPreference::Icons,
        slug: "icons",
        label: "Icons",
        description: "Wrapping icon grid",
        preview: "▦ ▦\n▦ ▦",
    },
    RowLayoutOption {
        layout: RowLayoutPreference::ColumnView,
        slug: "column-view",
        label: "Column view",
        description: "Side-by-side folder hierarchy",
        preview: "‹ FOLDER │ FILES ›",
    },
];

/// Clamp values that come from persisted state or a future slider/input.
#[must_use]
pub fn clamp_settings(mut settings: Settings) -> Settings {
    settings.sidebar_width = settings.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
    settings.split_ratio = if settings.split_ratio.is_finite() {
        settings.split_ratio.clamp(SPLIT_MIN, SPLIT_MAX)
    } else {
        Settings::default().split_ratio
    };
    settings
}

#[must_use]
pub fn clamp_sidebar_width(value: i32) -> u32 {
    value
        .clamp(SIDEBAR_MIN.cast_signed(), SIDEBAR_MAX.cast_signed())
        .cast_unsigned()
}

#[must_use]
pub fn clamp_split_ratio(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(SPLIT_MIN, SPLIT_MAX)
    } else {
        Settings::default().split_ratio
    }
}

#[must_use]
pub fn settings_for_apply(settings: Settings) -> Settings {
    clamp_settings(settings)
}

#[must_use]
pub fn reset_settings() -> Settings {
    Settings::default()
}

#[must_use]
pub fn can_decrease_sidebar_width(value: u32) -> bool {
    value > SIDEBAR_MIN
}

#[must_use]
pub fn can_increase_sidebar_width(value: u32) -> bool {
    value < SIDEBAR_MAX
}

#[must_use]
pub fn can_decrease_split_ratio(value: f32) -> bool {
    value > SPLIT_MIN
}

#[must_use]
pub fn can_increase_split_ratio(value: f32) -> bool {
    value < SPLIT_MAX
}

fn update_theme(settings: &mut Settings, value: ThemeChoice) -> bool {
    if settings.theme == value {
        false
    } else {
        settings.theme = value;
        true
    }
}

fn update_pane_mode(settings: &mut Settings, value: PaneMode) -> bool {
    if settings.pane_mode == value {
        false
    } else {
        settings.pane_mode = value;
        true
    }
}

fn update_row_layout(settings: &mut Settings, value: RowLayoutPreference) -> bool {
    if settings.row_layout == value {
        false
    } else {
        settings.row_layout = value;
        true
    }
}

fn update_show_hidden(settings: &mut Settings, value: bool) -> bool {
    if settings.show_hidden == value {
        false
    } else {
        settings.show_hidden = value;
        true
    }
}

fn update_folders_first(settings: &mut Settings, value: bool) -> bool {
    if settings.folders_first == value {
        false
    } else {
        settings.folders_first = value;
        true
    }
}

fn sidebar_width_after_delta(value: u32, delta: i32) -> u32 {
    clamp_sidebar_width(value.cast_signed().saturating_add(delta))
}

fn split_ratio_after_delta(value: f32, delta: f32) -> f32 {
    clamp_split_ratio(value + delta)
}

/// Events emitted by the modal. Apply carries only non-secret presentation
/// settings; Reset restores the in-memory form to defaults before Apply.
#[derive(Clone, Debug, PartialEq)]
pub enum PreferencesEvent {
    Apply(Settings),
    Cancel,
    Reset,
}

impl EventEmitter<PreferencesEvent> for Preferences {}

/// Preferences modal state. It is an ordinary GPUI entity, so a shell can
/// create it with `cx.new(|cx| Preferences::new(window, cx, palette, settings))`.
pub struct Preferences {
    settings: Settings,
    palette: Palette,
    focus: FocusHandle,
}

impl Preferences {
    #[must_use]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        palette: Palette,
        settings: Settings,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            settings: clamp_settings(settings),
            palette,
            focus,
        }
    }

    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: ThemeChoice, cx: &mut Context<Self>) {
        if update_theme(&mut self.settings, theme) {
            cx.notify();
        }
    }

    pub fn set_pane_mode(&mut self, mode: PaneMode, cx: &mut Context<Self>) {
        if update_pane_mode(&mut self.settings, mode) {
            cx.notify();
        }
    }

    pub fn set_row_layout(&mut self, layout: RowLayoutPreference, cx: &mut Context<Self>) {
        if update_row_layout(&mut self.settings, layout) {
            cx.notify();
        }
    }

    pub fn toggle_show_hidden(&mut self, cx: &mut Context<Self>) {
        self.set_show_hidden(!self.settings.show_hidden, cx);
    }

    pub fn toggle_folders_first(&mut self, cx: &mut Context<Self>) {
        self.set_folders_first(!self.settings.folders_first, cx);
    }

    pub fn set_show_hidden(&mut self, value: bool, cx: &mut Context<Self>) {
        if update_show_hidden(&mut self.settings, value) {
            cx.notify();
        }
    }

    pub fn set_folders_first(&mut self, value: bool, cx: &mut Context<Self>) {
        if update_folders_first(&mut self.settings, value) {
            cx.notify();
        }
    }

    pub fn adjust_sidebar_width(&mut self, delta: i32, cx: &mut Context<Self>) {
        let next = sidebar_width_after_delta(self.settings.sidebar_width, delta);
        if self.settings.sidebar_width != next {
            self.settings.sidebar_width = next;
            cx.notify();
        }
    }

    pub fn adjust_split_ratio(&mut self, delta: f32, cx: &mut Context<Self>) {
        let next = split_ratio_after_delta(self.settings.split_ratio, delta);
        if self.settings.split_ratio.to_bits() != next.to_bits() {
            self.settings.split_ratio = next;
            cx.notify();
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        cx.emit(PreferencesEvent::Apply(settings_for_apply(
            self.settings.clone(),
        )));
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.settings = reset_settings();
        cx.emit(PreferencesEvent::Reset);
        cx.notify();
    }

    fn cancel(cx: &mut Context<Self>) {
        cx.emit(PreferencesEvent::Cancel);
    }

    fn option_group<T: Copy + PartialEq + 'static>(
        id: &'static str,
        choices: &[(T, &'static str)],
        selected: T,
        cx: &mut Context<Self>,
        choose: impl Fn(&mut Preferences, T, &mut Context<Preferences>) + 'static + Copy,
    ) -> Stateful<Div> {
        let mut group = div().id(id).flex().gap(px(5.));
        for &(value, label) in choices {
            group = group.child(
                Button::new(SharedString::from(format!("{id}-{label}")))
                    .ghost()
                    .selected(selected == value)
                    .toggled(selected == value)
                    .label(label)
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| choose(this, value, cx)),
                    ),
            );
        }
        group
    }

    fn render_row_layout_option(
        &self,
        option: RowLayoutOption,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let selected = self.settings.row_layout == option.layout;
        let layout = option.layout;
        div()
            .id(SharedString::from(format!("row-layout-{}", option.slug)))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(38.))
            .px(px(7.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::color(if selected {
                p.accent
            } else {
                p.control_border
            }))
            .bg(theme::color(if selected {
                p.accent_soft
            } else {
                p.control
            }))
            .hover(|style| style.bg(theme::color(p.accent_soft)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set_row_layout(layout, cx);
            }))
            .child(
                theme::mono(
                    option.preview,
                    9.,
                    theme::color(if selected { p.accent } else { p.text_tertiary }),
                )
                .w(px(100.))
                .flex_none(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(theme::text(option.label, 11., theme::color(p.text_primary)))
                    .child(theme::text(
                        option.description,
                        9.5,
                        theme::color(p.text_secondary),
                    )),
            )
    }

    fn render_row_layouts(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut options = div().id("row-layout-options").flex().flex_col().gap(px(5.));
        for &option in &ROW_LAYOUT_OPTIONS {
            options = options.child(self.render_row_layout_option(option, cx));
        }
        options
    }

    fn render_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        cx: &mut Context<Self>,
        set: fn(&mut Preferences, bool, &mut Context<Preferences>),
    ) -> Checkbox {
        Checkbox::new(id)
            .label(label)
            .checked(checked)
            .text_color(theme::color(self.palette.text_secondary))
            .on_change(cx.listener(move |this, value, _, cx| set(this, *value, cx)))
    }

    fn render_row(&self, label: &'static str, control: impl IntoElement) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .w(px(125.))
                    .text_align(TextAlign::Right)
                    .child(theme::text(
                        label,
                        11.5,
                        theme::color(self.palette.text_secondary),
                    )),
            )
            .child(div().flex_1().child(control))
    }

    fn adjust_button(
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Preferences, &mut Context<Preferences>) + 'static,
    ) -> Button {
        Button::new(id)
            .ghost()
            .compact()
            .disabled(!enabled)
            .w(px(24.))
            .h(px(24.))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| action(this, cx)))
            .label(label)
    }

    fn render_sidebar_width(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Self::adjust_button(
                "sidebar-minus",
                "−",
                can_decrease_sidebar_width(self.settings.sidebar_width),
                cx,
                |this, cx| {
                    this.adjust_sidebar_width(-SIDEBAR_STEP.cast_signed(), cx);
                },
            ))
            .child(theme::mono(
                format!("{} px", self.settings.sidebar_width),
                10.,
                theme::color(self.palette.text_secondary),
            ))
            .child(Self::adjust_button(
                "sidebar-plus",
                "+",
                can_increase_sidebar_width(self.settings.sidebar_width),
                cx,
                |this, cx| {
                    this.adjust_sidebar_width(SIDEBAR_STEP.cast_signed(), cx);
                },
            ))
    }

    fn render_split_ratio(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Self::adjust_button(
                "split-minus",
                "−",
                can_decrease_split_ratio(self.settings.split_ratio),
                cx,
                |this, cx| this.adjust_split_ratio(-SPLIT_STEP, cx),
            ))
            .child(theme::mono(
                format!("{:.0}% left", self.settings.split_ratio * 100.),
                10.,
                theme::color(self.palette.text_secondary),
            ))
            .child(Self::adjust_button(
                "split-plus",
                "+",
                can_increase_split_ratio(self.settings.split_ratio),
                cx,
                |this, cx| this.adjust_split_ratio(SPLIT_STEP, cx),
            ))
    }

    fn render_file_options(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.render_toggle(
                "show-hidden",
                "Show hidden files",
                self.settings.show_hidden,
                cx,
                Self::set_show_hidden,
            ))
            .child(self.render_toggle(
                "folders-first",
                "Keep folders first when sorting",
                self.settings.folders_first,
                cx,
                Self::set_folders_first,
            ))
    }
}

impl Preferences {
    fn render_options(&self, cx: &mut Context<Self>) -> Div {
        let appearance = Self::option_group(
            "appearance",
            &[
                (ThemeChoice::System, "System"),
                (ThemeChoice::Light, "Light"),
                (ThemeChoice::Dark, "Dark"),
            ],
            self.settings.theme,
            cx,
            Self::set_theme,
        );
        let pane = Self::option_group(
            "pane-mode",
            &[(PaneMode::Single, "Single"), (PaneMode::Dual, "Two panes")],
            self.settings.pane_mode,
            cx,
            Self::set_pane_mode,
        );
        let rows = self.render_row_layouts(cx);
        let width = self.render_sidebar_width(cx);
        let split = self.render_split_ratio(cx);
        let files = self.render_file_options(cx);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(18.))
            .child(self.render_row("Appearance", appearance))
            .child(self.render_row("Default panes", pane))
            .child(self.render_row("Rows", rows))
            .child(self.render_row("Files", files))
            .child(self.render_row("Sidebar width", width));
        if self.settings.pane_mode == PaneMode::Dual {
            body = body.child(self.render_row("Pane split", split));
        }
        body.child(self.render_actions(cx))
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(7.))
            .pt(px(5.))
            .border_t_1()
            .border_color(theme::color(p.chrome_line))
            .child(
                Button::new("preferences-reset")
                    .ghost()
                    .label("Reset")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reset(cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("preferences-cancel")
                    .secondary()
                    .label("Cancel")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| Self::cancel(cx))),
            )
            .child(
                Button::new("preferences-apply")
                    .primary()
                    .label("Apply")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.apply(cx))),
            )
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        div()
            .id("preferences-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0000_008c))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    Self::cancel(cx);
                }
            }))
            .child(
                div()
                    .id("preferences-dialog")
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
                            .child(theme::text(
                                "Preferences",
                                12.,
                                theme::color(p.text_primary),
                            )),
                    )
                    .child(self.render_options(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        can_decrease_sidebar_width, can_decrease_split_ratio, can_increase_sidebar_width,
        can_increase_split_ratio, clamp_settings, clamp_sidebar_width, clamp_split_ratio,
        reset_settings, settings_for_apply, sidebar_width_after_delta, split_ratio_after_delta,
        update_folders_first, update_pane_mode, update_row_layout, update_show_hidden,
        update_theme, PaneMode, RowLayoutPreference, Settings, ThemeChoice, ROW_LAYOUT_OPTIONS,
        SIDEBAR_MAX, SIDEBAR_MIN, SPLIT_MAX, SPLIT_MIN,
    };

    #[test]
    fn sidebar_width_clamps_to_reasonable_native_bounds() {
        assert_eq!(clamp_sidebar_width(-1), SIDEBAR_MIN);
        assert_eq!(clamp_sidebar_width(999), SIDEBAR_MAX);
        assert_eq!(clamp_sidebar_width(240), 240);
    }

    #[test]
    fn split_ratio_clamps_and_rejects_non_finite_values() {
        assert_eq!(clamp_split_ratio(0.01).to_bits(), SPLIT_MIN.to_bits());
        assert_eq!(clamp_split_ratio(0.99).to_bits(), SPLIT_MAX.to_bits());
        assert_eq!(
            clamp_split_ratio(f32::NAN).to_bits(),
            Settings::default().split_ratio.to_bits()
        );
    }

    #[test]
    fn persisted_settings_are_normalised_before_apply() {
        let settings = Settings {
            sidebar_width: 1,
            split_ratio: f32::INFINITY,
            ..Settings::default()
        };
        let normal = clamp_settings(settings);
        assert_eq!(normal.sidebar_width, SIDEBAR_MIN);
        assert_eq!(
            normal.split_ratio.to_bits(),
            Settings::default().split_ratio.to_bits()
        );
    }

    #[test]
    fn every_preference_value_survives_apply_normalisation() {
        let settings = Settings {
            theme: ThemeChoice::Dark,
            pane_mode: PaneMode::Dual,
            row_layout: RowLayoutPreference::ColumnView,
            show_hidden: false,
            folders_first: false,
            sidebar_width: 280,
            split_ratio: 0.65,
            ..Settings::default()
        };
        let applied = settings_for_apply(settings.clone());
        assert_eq!(applied.theme, ThemeChoice::Dark);
        assert_eq!(applied.pane_mode, PaneMode::Dual);
        assert_eq!(applied.row_layout, RowLayoutPreference::ColumnView);
        assert!(!applied.show_hidden);
        assert!(!applied.folders_first);
        assert_eq!(applied.sidebar_width, 280);
        assert_eq!(applied.split_ratio.to_bits(), 0.65_f32.to_bits());
    }

    #[test]
    fn reset_is_default_and_numeric_buttons_disable_at_bounds() {
        assert_eq!(reset_settings(), Settings::default());
        assert!(!can_decrease_sidebar_width(SIDEBAR_MIN));
        assert!(can_increase_sidebar_width(SIDEBAR_MIN));
        assert!(can_decrease_sidebar_width(SIDEBAR_MAX));
        assert!(!can_increase_sidebar_width(SIDEBAR_MAX));
        assert!(!can_decrease_split_ratio(SPLIT_MIN));
        assert!(can_increase_split_ratio(SPLIT_MIN));
        assert!(can_decrease_split_ratio(SPLIT_MAX));
        assert!(!can_increase_split_ratio(SPLIT_MAX));
    }

    #[test]
    fn control_mutations_update_only_their_draft_field() {
        let mut settings = Settings::default();
        assert!(update_theme(&mut settings, ThemeChoice::Dark));
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert!(!update_theme(&mut settings, ThemeChoice::Dark));
        assert!(update_pane_mode(&mut settings, PaneMode::Dual));
        assert_eq!(settings.pane_mode, PaneMode::Dual);
        assert!(update_row_layout(
            &mut settings,
            RowLayoutPreference::ColumnView
        ));
        assert_eq!(settings.row_layout, RowLayoutPreference::ColumnView);
        assert!(update_show_hidden(&mut settings, false));
        assert!(!settings.show_hidden);
        assert!(update_folders_first(&mut settings, false));
        assert!(!settings.folders_first);
        assert_eq!(sidebar_width_after_delta(SIDEBAR_MIN, -1), SIDEBAR_MIN);
        assert_eq!(sidebar_width_after_delta(200, 8), 208);
        assert_eq!(sidebar_width_after_delta(SIDEBAR_MAX, 1), SIDEBAR_MAX);
        assert_eq!(
            split_ratio_after_delta(SPLIT_MIN, -0.1).to_bits(),
            SPLIT_MIN.to_bits()
        );
        assert_eq!(
            split_ratio_after_delta(0.5, 0.05).to_bits(),
            0.55_f32.to_bits()
        );
        assert_eq!(
            split_ratio_after_delta(SPLIT_MAX, 0.1).to_bits(),
            SPLIT_MAX.to_bits()
        );
    }

    #[test]
    fn row_layout_choices_have_distinct_meaningful_previews() {
        assert_eq!(ROW_LAYOUT_OPTIONS.len(), 5);
        for (index, option) in ROW_LAYOUT_OPTIONS.iter().enumerate() {
            assert!(!option.label.is_empty());
            assert!(!option.description.is_empty());
            assert!(!option.preview.is_empty());
            assert!(
                ROW_LAYOUT_OPTIONS[..index]
                    .iter()
                    .all(|other| other.layout != option.layout
                        && other.slug != option.slug
                        && other.preview != option.preview),
                "row layout option {:?} duplicates an earlier choice",
                option.layout
            );
        }
    }

    #[test]
    fn every_row_layout_choice_updates_the_intended_setting() {
        for option in ROW_LAYOUT_OPTIONS {
            let mut settings = Settings::default();
            let changed = update_row_layout(&mut settings, option.layout);
            assert_eq!(settings.row_layout, option.layout);
            assert_eq!(changed, option.layout != RowLayoutPreference::Columns);
        }
    }
}
