//! Excalibur's native-shell design tokens.
//!
//! The values mirror `design_handoff_share_browser 2/README.md`.  Keeping the
//! palette in one small module makes the light/dark switch a shell concern and
//! gives future browser/model views a stable vocabulary instead of scattered
//! raw colours.

use gpui_kit::{rgb, ParentElement, Rgba, Styled};

/// Theme-aware colours used by the shell and its browser surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub title_bar: u32,
    pub toolbar: u32,
    pub sidebar: u32,
    pub list: u32,
    pub field: u32,
    pub control: u32,
    pub control_border: u32,
    pub chrome_line: u32,
    pub row_line: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_tertiary: u32,
    pub text_disabled: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_soft: u32,
    pub success: u32,
    pub danger: u32,
    pub folder: u32,
    pub file: u32,
}

impl Palette {
    /// Light tokens from the handoff's primary browser screen.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            title_bar: 0x00f7_f7f8,
            toolbar: 0x00f4_f4f5,
            sidebar: 0x00ee_eef0,
            list: 0x00ff_ffff,
            field: 0x00ff_ffff,
            control: 0x00ff_ffff,
            control_border: 0x00c9_c9cd,
            chrome_line: 0x00dc_dcde,
            row_line: 0x00f2_f2f3,
            text_primary: 0x001a_1a1c,
            text_secondary: 0x006a_6a70,
            text_tertiary: 0x008a_8a90,
            text_disabled: 0x00a0_a0a6,
            accent: 0x002f_6df6,
            accent_hover: 0x001d_4fc4,
            accent_soft: 0x00e8_effd,
            success: 0x002f_9e5f,
            danger: 0x00c0_392b,
            folder: 0x008a_8f98,
            file: 0x00a8_adb5,
        }
    }

    /// Dark tokens from the read-only browser screen.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            title_bar: 0x0030_3135,
            toolbar: 0x002a_2b2e,
            sidebar: 0x0026_272a,
            list: 0x001e_1f21,
            field: 0x001c_1d1f,
            control: 0x0035_363a,
            control_border: 0x0045_464b,
            chrome_line: 0x001a_1b1d,
            row_line: 0x0026_272a,
            text_primary: 0x00e8_e8ea,
            text_secondary: 0x009a_9ba0,
            text_tertiary: 0x008a_8b90,
            text_disabled: 0x0063_646a,
            accent: 0x004f_86ff,
            accent_hover: 0x003e_73ea,
            accent_soft: 0x001f_3358,
            success: 0x002f_9e5f,
            danger: 0x00c0_392b,
            folder: 0x007d_838c,
            file: 0x00a8_adb5,
        }
    }
}

#[inline]
#[must_use]
pub fn color(value: u32) -> Rgba {
    rgb(value)
}

pub const SANS: &str = "IBM Plex Sans";
pub const MONO: &str = "IBM Plex Mono";

pub mod geometry {
    pub const TITLE_BAR: f32 = 34.;
    pub const TOOLBAR: f32 = 44.;
    pub const COLUMN_HEADER: f32 = 24.;
    pub const ROW: f32 = 24.;
    pub const STATUS_BAR: f32 = 24.;
    pub const SIDEBAR: f32 = 206.;
}

/// Build a text element using the shell's bundled sans or mono face.
pub fn text(content: impl Into<gpui_kit::SharedString>, size: f32, colour: Rgba) -> gpui_kit::Div {
    gpui_kit::div()
        .font_family(SANS)
        .text_size(gpui_kit::px(size))
        .line_height(gpui_kit::px(size + 3.))
        .text_color(colour)
        .child(content.into())
}

/// Build compact metadata text in the bundled mono face.
pub fn mono(content: impl Into<gpui_kit::SharedString>, size: f32, colour: Rgba) -> gpui_kit::Div {
    gpui_kit::div()
        .font_family(MONO)
        .text_size(gpui_kit::px(size))
        .line_height(gpui_kit::px(size + 3.))
        .text_color(colour)
        .child(content.into())
}
