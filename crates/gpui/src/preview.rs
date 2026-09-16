//! Bounded native Quick Look support.
//!
//! Preview requests deliberately have a much smaller budget than transfers.
//! Text and native image bytes are bounded explicitly. Video/audio and image
//! formats not understood by GPUI are served through the media bridge and
//! opened by mpv, so codecs such as H.264, H.265 and AV1 remain supported.

use std::sync::Arc;

use excalibur_engine::backend::EntryKind;
use excalibur_engine::kind::Category;
use excalibur_engine::{EngineError, Result};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, img, px, ClickEvent, Context, Div, EventEmitter, Image, ImageFormat, IntoElement,
    ObjectFit, Render, ScrollHandle, Stateful, TextAlign, Window,
};

use crate::media_player::{MediaKind, MediaPlayer};
use crate::media_stream::MediaStream;
use crate::model::EntryRow;
use crate::services::{ServiceTask, Services};
use crate::theme::{self, Palette};

/// Maximum bytes materialized for a text preview.
pub const MAX_PREVIEW_BYTES: u32 = 256 * 1024;
/// Maximum bytes materialized for an inline native image preview.
pub const MAX_PREVIEW_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum displayed lines in one preview.
pub const MAX_PREVIEW_LINES: usize = 2_000;

/// Safe metadata supplied by the browser row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewTarget {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub category: Category,
    pub kind_label: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

impl From<&EntryRow> for PreviewTarget {
    fn from(row: &EntryRow) -> Self {
        Self {
            name: row.name.clone(),
            path: row.path.clone(),
            kind: row.kind,
            category: row.kind_category,
            kind_label: row.kind_label.clone(),
            size: row.size,
            modified_ms: row.modified_ms,
        }
    }
}

/// Metadata shown in the Quick Look header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewMetadata {
    pub name: String,
    pub path: String,
    pub kind_label: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

/// Decoded preview body. Text is converted to bounded lines; native image
/// bytes are retained only inside GPUI's immutable image value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewBody {
    Text {
        lines: Vec<String>,
        truncated: bool,
    },
    Image {
        image: Arc<Image>,
    },
    Media {
        kind: MediaKind,
        stream: MediaStream,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewContent {
    pub metadata: PreviewMetadata,
    pub body: PreviewBody,
}

/// Current preview request state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewState {
    Idle,
    Loading {
        token: u64,
        metadata: PreviewMetadata,
    },
    Ready(PreviewContent),
    Unsupported {
        metadata: PreviewMetadata,
        reason: String,
    },
    Failed {
        metadata: PreviewMetadata,
        error: String,
    },
}

/// Actions emitted by the floating panel.
pub enum PreviewEvent {
    Close,
    OpenInEditor { path: String },
    Download { path: String },
}

/// A started bounded preview request.
pub struct PreviewRequest {
    token: u64,
    task: ServiceTask<PreviewContent>,
}

impl PreviewRequest {
    #[must_use]
    pub const fn token(&self) -> u64 {
        self.token
    }

    #[must_use]
    pub fn into_task(self) -> ServiceTask<PreviewContent> {
        self.task
    }
}

/// GPUI model for a dark Quick Look panel.
pub struct PreviewModel {
    services: Arc<Services>,
    target: Option<PreviewTarget>,
    next_token: u64,
    state: PreviewState,
    scroll: ScrollHandle,
    media_stream: Option<MediaStream>,
    media_player: Option<MediaPlayer>,
}

impl EventEmitter<PreviewEvent> for PreviewModel {}

impl PreviewModel {
    #[must_use]
    pub fn new(services: Arc<Services>, _palette: Palette) -> Self {
        Self {
            services,
            target: None,
            next_token: 0,
            state: PreviewState::Idle,
            scroll: ScrollHandle::new(),
            media_stream: None,
            media_player: None,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &PreviewState {
        &self.state
    }

    #[must_use]
    pub const fn target(&self) -> Option<&PreviewTarget> {
        self.target.as_ref()
    }

    /// Start a bounded request for one file. Unsupported media never reaches
    /// the backend and therefore cannot accidentally buffer a whole file.
    pub fn open(&mut self, target: PreviewTarget) -> PreviewRequest {
        self.stop_media();
        self.next_token = self.next_token.wrapping_add(1).max(1);
        let token = self.next_token;
        let target_metadata = metadata(&target);
        self.target = Some(target.clone());

        if let Some(reason) = unsupported_reason(target.kind, target.category) {
            self.state = PreviewState::Unsupported {
                metadata: target_metadata,
                reason,
            };
            return immediate_request(
                token,
                EngineError::Unsupported("preview is not supported for this file".to_owned()),
            );
        }
        self.state = PreviewState::Loading {
            token,
            metadata: target_metadata.clone(),
        };
        let task = match target.category {
            Category::Text | Category::Code => {
                let length = u32::try_from(target.size.min(u64::from(MAX_PREVIEW_BYTES)))
                    .unwrap_or(MAX_PREVIEW_BYTES);
                let request = self.services.read_range(target.path.clone(), 0, length);
                Box::pin(async move {
                    let bytes = request.await?;
                    decode_text(&target, &bytes)
                }) as ServiceTask<PreviewContent>
            }
            Category::Image => {
                if let Some(format) = native_image_format(&target) {
                    let request = self.services.read_file_bounded(
                        target.path.clone(),
                        target.size,
                        MAX_PREVIEW_IMAGE_BYTES,
                    );
                    Box::pin(async move {
                        let bytes = request.await?;
                        Ok(PreviewContent {
                            metadata: metadata(&target),
                            body: PreviewBody::Image {
                                image: Arc::new(Image::from_bytes(format, bytes)),
                            },
                        })
                    }) as ServiceTask<PreviewContent>
                } else {
                    media_request(&self.services, target, MediaKind::Image)
                }
            }
            Category::Video => media_request(&self.services, target, MediaKind::Video),
            Category::Audio => media_request(&self.services, target, MediaKind::Audio),
            _ => return immediate_unsupported(token, target_metadata),
        };
        PreviewRequest { token, task }
    }

    /// Apply only the currently visible request's result.
    pub fn apply(&mut self, token: u64, result: Result<PreviewContent>) -> bool {
        let PreviewState::Loading { token: current, .. } = self.state else {
            return false;
        };
        if current != token {
            return false;
        }
        self.state = match result {
            Ok(content) => {
                if let PreviewBody::Media { kind, stream } = &content.body {
                    match MediaPlayer::open(stream.url(), &content.metadata.name, *kind) {
                        Ok(player) => {
                            self.media_stream = Some(stream.clone());
                            self.media_player = Some(player);
                            PreviewState::Ready(content)
                        }
                        Err(error) => PreviewState::Failed {
                            metadata: content.metadata,
                            error,
                        },
                    }
                } else {
                    PreviewState::Ready(content)
                }
            }
            Err(error) => {
                let metadata = match &self.state {
                    PreviewState::Loading { metadata, .. } => metadata.clone(),
                    _ => unreachable!(),
                };
                PreviewState::Failed {
                    metadata,
                    error: error.to_string(),
                }
            }
        };
        true
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.stop_media();
        self.next_token = self.next_token.wrapping_add(1).max(1);
        self.target = None;
        self.state = PreviewState::Idle;
        cx.emit(PreviewEvent::Close);
        cx.notify();
    }

    fn stop_media(&mut self) {
        if let Some(mut player) = self.media_player.take() {
            let _ = player.stop();
        }
        self.media_stream = None;
    }

    fn emit_path(&mut self, event: fn(String) -> PreviewEvent, cx: &mut Context<Self>) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        cx.emit(event(target.path.clone()));
    }
}

impl Render for PreviewModel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::dark();
        let title = self
            .target
            .as_ref()
            .map_or_else(|| "Quick Look".to_owned(), |target| target.name.clone());
        div()
            .id("quick-look")
            .absolute()
            .top(px(50.))
            .right(px(16.))
            .w(px(440.))
            .max_h(px(560.))
            .flex()
            .flex_col()
            .bg(theme::color(p.list))
            .border_1()
            .border_color(theme::color(p.chrome_line))
            .rounded(px(5.))
            .shadow_lg()
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .flex_none()
                    .bg(theme::color(p.toolbar))
                    .child(theme::text(title, 11.5, theme::color(p.text_primary)))
                    .child(div().flex_1())
                    .child(
                        Button::new("quick-look-close")
                            .ghost()
                            .label("×")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close(cx))),
                    ),
            )
            .child(preview_body(&self.state, p, &self.scroll))
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .flex_none()
                    .bg(theme::color(p.toolbar))
                    .child(theme::mono(
                        footer_label(&self.state),
                        9.5,
                        theme::color(p.text_secondary),
                    ))
                    .child(div().flex_1())
                    .when(self.target.is_some(), |footer| {
                        footer
                            .child(
                                Button::new("quick-look-editor")
                                    .ghost()
                                    .label("Open in editor")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.emit_path(
                                            |path| PreviewEvent::OpenInEditor { path },
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("quick-look-download")
                                    .ghost()
                                    .label("Download")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.emit_path(|path| PreviewEvent::Download { path }, cx);
                                    })),
                            )
                    }),
            )
    }
}

fn metadata(target: &PreviewTarget) -> PreviewMetadata {
    PreviewMetadata {
        name: target.name.clone(),
        path: target.path.clone(),
        kind_label: target.kind_label.clone(),
        size: target.size,
        modified_ms: target.modified_ms,
    }
}

fn unsupported_reason(kind: EntryKind, category: Category) -> Option<String> {
    if kind != EntryKind::File {
        return Some("Only regular files can be previewed".to_owned());
    }
    match category {
        Category::Text | Category::Code | Category::Image | Category::Video | Category::Audio => {
            None
        }
        Category::Folder => Some("Folders cannot be previewed".to_owned()),
        _ => Some("Quick Look is unavailable for this file type".to_owned()),
    }
}

fn immediate_unsupported(token: u64, metadata: PreviewMetadata) -> PreviewRequest {
    PreviewRequest {
        token,
        task: Box::pin(async move {
            Err(EngineError::Unsupported(format!(
                "Quick Look is unavailable for {}",
                metadata.name
            )))
        }),
    }
}

fn extension(target: &PreviewTarget) -> &str {
    target
        .name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
}

fn native_image_format(target: &PreviewTarget) -> Option<ImageFormat> {
    match extension(target).to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "ico" => Some(ImageFormat::Ico),
        "pnm" | "pbm" | "pgm" | "ppm" => Some(ImageFormat::Pnm),
        "svg" => Some(ImageFormat::Svg),
        _ => None,
    }
}

fn media_mime(target: &PreviewTarget, kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => match extension(target).to_ascii_lowercase().as_str() {
            "heic" | "heif" => "image/heic",
            "avif" => "image/avif",
            _ => "image/*",
        },
        MediaKind::Video => match extension(target).to_ascii_lowercase().as_str() {
            "mkv" => "video/x-matroska",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            _ => "video/*",
        },
        MediaKind::Audio => match extension(target).to_ascii_lowercase().as_str() {
            "flac" => "audio/flac",
            "wav" => "audio/wav",
            "ogg" | "opus" => "audio/ogg",
            _ => "audio/*",
        },
    }
}

fn media_request(
    services: &Services,
    target: PreviewTarget,
    kind: MediaKind,
) -> ServiceTask<PreviewContent> {
    let request = services.media_stream(
        target.path.clone(),
        target.size,
        media_mime(&target, kind).to_owned(),
    );
    Box::pin(async move {
        let stream = request.await?;
        Ok(PreviewContent {
            metadata: metadata(&target),
            body: PreviewBody::Media { kind, stream },
        })
    })
}

fn decode_text(target: &PreviewTarget, bytes: &[u8]) -> Result<PreviewContent> {
    if bytes.contains(&0) {
        return Err(EngineError::Unsupported(
            "binary file preview is unavailable".to_owned(),
        ));
    }
    let truncated_bytes = target.size > u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text
        .lines()
        .take(MAX_PREVIEW_LINES)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let truncated_lines = text.lines().nth(MAX_PREVIEW_LINES).is_some();
    if lines.is_empty() {
        lines.push(String::new());
    }
    Ok(PreviewContent {
        metadata: metadata(target),
        body: PreviewBody::Text {
            lines,
            truncated: truncated_bytes || truncated_lines,
        },
    })
}

fn immediate_request(token: u64, error: EngineError) -> PreviewRequest {
    PreviewRequest {
        token,
        task: Box::pin(async move { Err(error) }),
    }
}

fn footer_label(state: &PreviewState) -> String {
    match state {
        PreviewState::Idle => String::new(),
        PreviewState::Loading { .. } => "Loading…".to_owned(),
        PreviewState::Ready(content) => match &content.body {
            PreviewBody::Text { truncated, .. } if *truncated => "Text · truncated".to_owned(),
            PreviewBody::Text { .. } => "Text".to_owned(),
            PreviewBody::Image { .. } => "Image".to_owned(),
            PreviewBody::Media { kind, .. } => match kind {
                MediaKind::Image => "Image · mpv".to_owned(),
                MediaKind::Video => "Video · mpv".to_owned(),
                MediaKind::Audio => "Audio · mpv".to_owned(),
            },
        },
        PreviewState::Unsupported { reason, .. } => reason.clone(),
        PreviewState::Failed { error, .. } => error.clone(),
    }
}

fn preview_body(state: &PreviewState, p: Palette, scroll: &ScrollHandle) -> Stateful<Div> {
    match state {
        PreviewState::Idle => div().id("preview-idle").flex_1(),
        PreviewState::Loading { .. } => {
            div()
                .id("preview-loading")
                .flex_1()
                .p(px(14.))
                .child(theme::text(
                    "Loading preview…",
                    11.,
                    theme::color(p.text_secondary),
                ))
        }
        PreviewState::Unsupported { reason, .. } => div()
            .id("preview-unsupported")
            .flex_1()
            .p(px(14.))
            .child(theme::text(
                reason.clone(),
                11.,
                theme::color(p.text_secondary),
            )),
        PreviewState::Failed { error, .. } => div()
            .id("preview-failed")
            .flex_1()
            .p(px(14.))
            .child(theme::text(error.clone(), 11., theme::color(p.danger))),
        PreviewState::Ready(content) => match &content.body {
            PreviewBody::Text { lines, truncated } => text_preview(lines, *truncated, p, scroll),
            PreviewBody::Image { image } => div()
                .id("preview-image")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(12.))
                .bg(theme::color(p.list))
                .child(
                    img(Arc::clone(image))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Contain),
                ),
            PreviewBody::Media { kind, .. } => div()
                .id("preview-media")
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(18.))
                .bg(theme::color(p.list))
                .child(theme::text(
                    media_instructions(*kind),
                    11.,
                    theme::color(p.text_secondary),
                )),
        },
    }
}

fn text_preview(
    lines: &[String],
    truncated: bool,
    p: Palette,
    scroll: &ScrollHandle,
) -> Stateful<Div> {
    let mut body = div()
        .id("preview-text")
        .flex_1()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_y_scroll()
        .p(px(10.))
        .bg(theme::color(p.list))
        .flex()
        .flex_col();
    for (index, line) in lines.iter().enumerate() {
        body = body.child(
            div()
                .flex()
                .items_start()
                .min_h(px(18.))
                .child(
                    theme::mono(
                        format!("{:>4} ", index + 1),
                        10.,
                        theme::color(p.text_disabled),
                    )
                    .w(px(38.))
                    .text_align(TextAlign::Right),
                )
                .child(theme::mono(
                    line.clone(),
                    10.5,
                    theme::color(p.text_primary),
                )),
        );
    }
    if truncated {
        body = body.child(theme::text(
            "Preview truncated at the native safety limit",
            10.,
            theme::color(p.text_tertiary),
        ));
    }
    body
}

fn media_instructions(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "Image opened in mpv · close the mpv window when finished",
        MediaKind::Video => "Video opened in mpv · use the mpv window for playback controls",
        MediaKind::Audio => "Audio opened in mpv · use the mpv window for playback controls",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_text, media_mime, native_image_format, unsupported_reason, Category, EngineError,
        EntryKind, ImageFormat, MediaKind, Palette, PreviewBody, PreviewModel, PreviewState,
        PreviewTarget, MAX_PREVIEW_BYTES,
    };
    use crate::services::Services;
    use crate::services::MAX_PREVIEW_READ;
    use std::sync::Arc;

    fn text_target(size: u64) -> PreviewTarget {
        PreviewTarget {
            name: "notes.txt".to_owned(),
            path: "notes.txt".to_owned(),
            kind: EntryKind::File,
            category: Category::Text,
            kind_label: "Plain Text".to_owned(),
            size,
            modified_ms: None,
        }
    }

    #[test]
    fn binary_bytes_are_refused_without_retaining_them() {
        let target = text_target(2);
        let error = decode_text(&target, &[b'a', 0]).expect_err("binary refusal");
        assert!(error.to_string().contains("binary"));
    }

    #[test]
    fn text_decode_is_lossy_and_caps_lines() {
        let target = text_target(4);
        let content = decode_text(&target, &[b'a', 0xff, b'\n', b'b']).expect("text");
        let PreviewBody::Text { lines, truncated } = content.body else {
            panic!("expected text preview");
        };
        assert_eq!(lines.len(), 2);
        assert!(!truncated);
    }

    #[test]
    fn preview_cap_stays_below_service_range_cap() {
        const { assert!(MAX_PREVIEW_BYTES <= MAX_PREVIEW_READ) };
    }

    #[test]
    fn stale_preview_result_is_ignored() {
        let services = Arc::new(Services::mock().expect("runtime"));
        let mut model = PreviewModel::new(services, Palette::dark());
        let old = model.open(text_target(5));
        let new = model.open(text_target(5));
        assert!(!model.apply(
            old.token(),
            Err(EngineError::Disconnected("old preview".to_owned())),
        ));
        assert!(
            matches!(model.state(), PreviewState::Loading { token, .. } if *token == new.token())
        );
    }

    #[test]
    fn category_controls_preview_routing() {
        assert!(unsupported_reason(EntryKind::File, Category::Video).is_none());
        assert!(unsupported_reason(EntryKind::File, Category::Audio).is_none());
        assert!(unsupported_reason(EntryKind::File, Category::Image).is_none());
        assert!(unsupported_reason(EntryKind::File, Category::Archive).is_some());
    }

    #[test]
    fn native_image_formats_are_explicit_and_case_insensitive() {
        let mut target = text_target(4);
        target.name = "photo.JPEG".to_owned();
        assert_eq!(native_image_format(&target), Some(ImageFormat::Jpeg));
        target.name = "photo.avif".to_owned();
        assert_eq!(native_image_format(&target), None);
        target.name = "photo.pgm".to_owned();
        assert_eq!(native_image_format(&target), Some(ImageFormat::Pnm));
    }

    #[test]
    fn media_mime_follows_the_selected_category() {
        let mut target = text_target(4);
        target.name = "clip.mkv".to_owned();
        assert_eq!(media_mime(&target, MediaKind::Video), "video/x-matroska");
        target.name = "track.flac".to_owned();
        assert_eq!(media_mime(&target, MediaKind::Audio), "audio/flac");
        target.name = "camera.avif".to_owned();
        assert_eq!(media_mime(&target, MediaKind::Image), "image/avif");
    }
}
