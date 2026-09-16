//! New Connection modal for the native shell.
//!
//! The editable draft intentionally contains no password. The password field
//! lives in a masked GPUI input state and is converted to an engine `Secret`
//! only while emitting `Test` or `Connect`; the event is not `Debug` and the
//! secret is never persisted with the draft.

use std::sync::Arc;

use excalibur_engine::backend::{Access, ConnectionSpec, Protocol};
use excalibur_engine::{Secret, SecretStore};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
use gpui_kit::component::{Selectable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, rgba, App, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, KeyDownEvent,
    MouseButton, Stateful, Subscription, TextAlign, Window,
};

use crate::browser::ConnectionDraft;
use crate::theme::{self, Palette};

/// A typed result delivered by the dialog to its owner.
pub enum ConnectionDialogEvent {
    Cancel,
    Test {
        spec: ConnectionSpec,
        secret: Option<Secret>,
    },
    Connect {
        spec: ConnectionSpec,
        secret: Option<Secret>,
        save_in_keychain: bool,
    },
}

impl EventEmitter<ConnectionDialogEvent> for ConnectionDialog {}

/// Async connection-test state owned by the shell that handles `Test`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestStatus {
    Idle,
    Testing,
    Passed(String),
    Failed(String),
}

/// Form fields that can be reported to the user without including a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Host,
    Share,
    Port,
    Username,
    Domain,
}

/// One safe validation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub field: Field,
    pub message: String,
}

/// Validate a draft before emitting an engine operation.
#[must_use]
pub fn validate_draft(draft: &ConnectionDraft, _domain: &str) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let network = !matches!(draft.protocol, Protocol::Local);
    if network && draft.host.trim().is_empty() {
        errors.push(ValidationError {
            field: Field::Host,
            message: "Server is required".to_owned(),
        });
    }
    if draft.share.trim().is_empty() {
        errors.push(ValidationError {
            field: Field::Share,
            message: if draft.protocol == Protocol::Nfs {
                "Export path is required".to_owned()
            } else {
                "Share is required".to_owned()
            },
        });
    }
    if draft.port == Some(0) {
        errors.push(ValidationError {
            field: Field::Port,
            message: "Port must be between 1 and 65535".to_owned(),
        });
    }
    if draft.protocol == Protocol::Nfs {
        if let Some(identity) = draft
            .username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let valid = identity.split_once(':').map_or_else(
                || identity.parse::<u32>().is_ok(),
                |(uid, gid)| uid.trim().parse::<u32>().is_ok() && gid.trim().parse::<u32>().is_ok(),
            );
            if !valid {
                errors.push(ValidationError {
                    field: Field::Username,
                    message: "Identity must be uid or uid:gid".to_owned(),
                });
            }
        }
    }
    errors
}

/// Reusable connection form. `secret_store` is optional because saving is an
/// application policy; this view never reads from or writes to it directly.
pub struct ConnectionDialog {
    draft: ConnectionDraft,
    domain: String,
    password: Entity<InputState>,
    host: Entity<InputState>,
    share: Entity<InputState>,
    username: Entity<InputState>,
    domain_input: Entity<InputState>,
    port: Entity<InputState>,
    save_in_keychain: bool,
    access: Access,
    status: TestStatus,
    error: Option<String>,
    palette: Palette,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, palette: Palette) -> Self {
        Self::with_secret_store(window, cx, palette, None)
    }

    pub fn with_secret_store(
        window: &mut Window,
        cx: &mut Context<Self>,
        palette: Palette,
        secret_store: Option<Arc<dyn SecretStore>>,
    ) -> Self {
        let save_in_keychain = secret_store.is_some_and(|store| store.available());
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("nas.local"));
        let share = cx.new(|cx| InputState::new(window, cx).placeholder("media"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("username"));
        let domain_input = cx.new(|cx| InputState::new(window, cx).placeholder("WORKGROUP"));
        let port = cx.new(|cx| InputState::new(window, cx).placeholder("default"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let subscriptions = [
            host.clone(),
            share.clone(),
            username.clone(),
            domain_input.clone(),
            port.clone(),
            password.clone(),
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.status = TestStatus::Idle;
                    this.error = None;
                    cx.notify();
                }
            })
        })
        .collect();
        Self {
            draft: ConnectionDraft {
                protocol: Protocol::Smb,
                host: String::new(),
                port: None,
                username: None,
                share: String::new(),
            },
            domain: String::new(),
            password,
            host,
            share,
            username,
            domain_input,
            port,
            save_in_keychain,
            access: Access::Rw,
            status: TestStatus::Idle,
            error: None,
            palette,
            focus,
            _subscriptions: subscriptions,
        }
    }

    #[must_use]
    pub const fn draft(&self) -> &ConnectionDraft {
        &self.draft
    }

    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    #[must_use]
    pub const fn test_status(&self) -> &TestStatus {
        &self.status
    }

    /// Update the async-test result from the owner without putting runtime
    /// futures in this reusable view.
    pub fn set_test_status(&mut self, status: TestStatus, cx: &mut Context<Self>) {
        self.status = status;
        self.error = None;
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(ConnectionDialogEvent::Cancel);
    }

    pub fn set_protocol(&mut self, protocol: Protocol, cx: &mut Context<Self>) {
        self.draft.protocol = protocol;
        self.status = TestStatus::Idle;
        cx.notify();
    }

    /// Replace the non-secret portion of the form when reopening a bookmark.
    /// The password input is deliberately cleared and is never populated from
    /// a `ConnectionDraft` or persisted settings value.
    pub fn set_draft(
        &mut self,
        draft: ConnectionDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let domain = self.domain.clone();
        self.draft = draft.clone();
        self.host.update(cx, |input, input_cx| {
            input.set_value(draft.host, window, input_cx);
        });
        self.share.update(cx, |input, input_cx| {
            input.set_value(draft.share, window, input_cx);
        });
        self.username.update(cx, |input, input_cx| {
            input.set_value(draft.username.unwrap_or_default(), window, input_cx);
        });
        self.port.update(cx, |input, input_cx| {
            input.set_value(
                draft.port.map_or_else(String::new, |port| port.to_string()),
                window,
                input_cx,
            );
        });
        self.domain_input.update(cx, |input, input_cx| {
            input.set_value(domain, window, input_cx);
        });
        self.password
            .update(cx, |input, input_cx| input.set_value("", window, input_cx));
        self.status = TestStatus::Idle;
        self.error = None;
        cx.notify();
    }

    fn read_form(&mut self, cx: &App) {
        self.host
            .read(cx)
            .value()
            .to_string()
            .trim()
            .clone_into(&mut self.draft.host);
        self.share
            .read(cx)
            .value()
            .to_string()
            .trim()
            .clone_into(&mut self.draft.share);
        let username = self.username.read(cx).value().to_string().trim().to_owned();
        self.draft.username = (!username.is_empty()).then_some(username);
        self.domain_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .clone_into(&mut self.domain);
        let port = self.port.read(cx).value().to_string();
        self.draft.port = match port.trim() {
            "" => None,
            value => Some(value.parse::<u16>().unwrap_or(0)),
        };
    }

    fn spec(&mut self, cx: &App) -> std::result::Result<ConnectionSpec, Vec<ValidationError>> {
        self.read_form(cx);
        let errors = validate_draft(&self.draft, &self.domain);
        if !errors.is_empty() {
            return Err(errors);
        }
        let host = self.draft.host.clone();
        let share = self.draft.share.clone();
        let protocol = self.draft.protocol;
        let local = protocol == Protocol::Local;
        let id_share = if share.starts_with('/') {
            share.clone()
        } else {
            format!("/{share}")
        };
        let id_host = if local { "" } else { host.as_str() };
        Ok(ConnectionSpec {
            id: format!("{}://{}{}", protocol.scheme(), id_host, id_share),
            name: if share.is_empty() {
                host.clone()
            } else {
                share.clone()
            },
            protocol,
            host: if local { String::new() } else { host },
            port: if local { None } else { self.draft.port },
            share,
            username: if local {
                None
            } else {
                self.draft.username.clone()
            },
            domain: (protocol == Protocol::Smb && !self.domain.is_empty())
                .then(|| self.domain.clone()),
            mount_preference: self.access,
        })
    }

    fn emit_operation(&mut self, test: bool, cx: &mut Context<Self>) {
        let Ok(spec) = self.spec(cx) else {
            self.error = Some("Please correct the highlighted connection fields".to_owned());
            cx.notify();
            return;
        };
        let secret = if matches!(spec.protocol, Protocol::Smb | Protocol::Sftp)
            && !self.password.read(cx).unmask_value().is_empty()
        {
            Some(Secret::new(
                self.password.read(cx).unmask_value().to_string(),
            ))
        } else {
            None
        };
        self.status = if test {
            TestStatus::Testing
        } else {
            TestStatus::Idle
        };
        if test {
            cx.emit(ConnectionDialogEvent::Test { spec, secret });
        } else {
            cx.emit(ConnectionDialogEvent::Connect {
                spec,
                secret,
                save_in_keychain: self.save_in_keychain,
            });
        }
        cx.notify();
    }

    fn input_frame(
        &self,
        id: &'static str,
        input: &Entity<InputState>,
        mono: bool,
        password: bool,
    ) -> Stateful<Div> {
        let p = self.palette;
        let input = if password {
            Input::new(input)
                .appearance(false)
                .xsmall()
                .content_type(InputContentType::Password)
        } else {
            Input::new(input).appearance(false).xsmall()
        };
        div()
            .id(id)
            .h(px(24.))
            .flex()
            .items_center()
            .px(px(7.))
            .bg(theme::color(p.field))
            .border_1()
            .border_color(theme::color(p.control_border))
            .rounded(px(5.))
            .font_family(if mono { theme::MONO } else { theme::SANS })
            .text_size(px(if mono { 11. } else { 11.5 }))
            .text_color(theme::color(p.text_primary))
            .child(input)
    }
}

impl Render for ConnectionDialog {
    #[expect(
        clippy::too_many_lines,
        reason = "the modal layout is kept together for visual review"
    )]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.read_form(cx);
        let p = self.palette;
        let smb = self.draft.protocol == Protocol::Smb;
        let local = self.draft.protocol == Protocol::Local;
        let password_protocol = matches!(self.draft.protocol, Protocol::Smb | Protocol::Sftp);
        let share_label = match self.draft.protocol {
            Protocol::Local => "Folder",
            Protocol::Nfs => "Export",
            Protocol::Sftp | Protocol::Xsync | Protocol::Smb => "Share",
        };
        let username_label = if self.draft.protocol == Protocol::Nfs {
            "Identity"
        } else {
            "Username"
        };
        let status = match &self.status {
            TestStatus::Idle => "",
            TestStatus::Testing => "Testing…",
            TestStatus::Passed(value) | TestStatus::Failed(value) => value.as_str(),
        };
        let status_colour = if matches!(self.status, TestStatus::Passed(_)) {
            p.success
        } else if matches!(self.status, TestStatus::Failed(_)) || self.error.is_some() {
            p.danger
        } else {
            p.text_secondary
        };
        let domain_visible = smb;
        let keychain = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(toggle_box(
                self.save_in_keychain,
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.save_in_keychain = !this.save_in_keychain;
                    cx.notify();
                }),
            ))
            .child(theme::text(
                "Save in keychain",
                11.5,
                theme::color(p.text_secondary),
            ));
        div()
            .id("connection-dialog-backdrop")
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
                    this.cancel(cx);
                }
            }))
            .child(
                div()
                    .id("connection-dialog")
                    .w(px(460.))
                    .flex()
                    .flex_col()
                    .bg(theme::color(p.toolbar))
                    .border_1()
                    .border_color(theme::color(p.chrome_line))
                    .rounded(px(5.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_b_1()
                            .border_color(theme::color(p.chrome_line))
                            .child(theme::text(
                                "New Connection",
                                12.,
                                theme::color(p.text_primary),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(11.))
                            .p(px(18.))
                            .child(protocol_tabs(self.draft.protocol, cx))
                            .when(!local, |body| {
                                body.child(form_row(
                                    "Server",
                                    self.input_frame("host", &self.host, true, false),
                                    p,
                                ))
                            })
                            .child(form_row(
                                share_label,
                                self.input_frame("share", &self.share, true, false),
                                p,
                            ))
                            .when(!local, |body| {
                                body.child(form_row(
                                    username_label,
                                    self.input_frame("username", &self.username, false, false),
                                    p,
                                ))
                            })
                            .when(domain_visible, |body| {
                                body.child(form_row(
                                    "Domain",
                                    self.input_frame("domain", &self.domain_input, true, false),
                                    p,
                                ))
                            })
                            .when(!local, |body| {
                                body.child(form_row(
                                    "Port",
                                    self.input_frame("port", &self.port, true, false),
                                    p,
                                ))
                            })
                            .when(password_protocol, |body| {
                                body.child(form_row(
                                    "Password",
                                    self.input_frame("password", &self.password, true, true),
                                    p,
                                ))
                            })
                            .when(password_protocol, |body| body.child(keychain))
                            .child(form_row("Mount as", access_toggle(self.access, cx), p))
                            .child(
                                div()
                                    .px(px(10.))
                                    .py(px(8.))
                                    .bg(theme::color(p.field))
                                    .border_1()
                                    .border_color(theme::color(p.chrome_line))
                                    .rounded(px(5.))
                                    .child(theme::mono(
                                        uri_preview(&self.draft, &self.domain),
                                        10.5,
                                        theme::color(p.text_secondary),
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        Button::new("connection-test")
                                            .secondary()
                                            .label("Test")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    this.emit_operation(true, cx);
                                                },
                                            )),
                                    )
                                    .child(theme::text(status, 11., theme::color(status_colour)))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("connection-cancel")
                                            .secondary()
                                            .label("Cancel")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| this.cancel(cx),
                                            )),
                                    )
                                    .child(
                                        Button::new("connection-connect")
                                            .primary()
                                            .label("Connect")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    this.emit_operation(false, cx);
                                                },
                                            )),
                                    ),
                            ),
                    ),
            )
    }
}

fn form_row(label: &'static str, field: Stateful<Div>, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(82.))
                .text_align(TextAlign::Right)
                .child(theme::text(label, 11.5, theme::color(p.text_secondary))),
        )
        .child(field.flex_1())
}

fn protocol_tabs(protocol: Protocol, cx: &mut Context<ConnectionDialog>) -> Div {
    let mut tabs = div()
        .flex()
        .gap(px(4.))
        .p(px(2.))
        .bg(theme::color(0x00e2_e2e5))
        .rounded(px(6.));
    for (id, label, value) in [
        ("protocol-smb", "SMB", Protocol::Smb),
        ("protocol-nfs", "NFS", Protocol::Nfs),
        ("protocol-sftp", "SFTP", Protocol::Sftp),
        ("protocol-xsync", "xsync", Protocol::Xsync),
        ("protocol-local", "Local", Protocol::Local),
    ] {
        tabs = tabs.child(
            Button::new(id)
                .secondary()
                .selected(protocol == value)
                .label(label)
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.set_protocol(value, cx)),
                ),
        );
    }
    tabs
}

fn toggle_box(
    checked: bool,
    p: Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id("save-keychain")
        .size(px(13.))
        .flex_none()
        .border_1()
        .border_color(theme::color(if checked {
            p.accent
        } else {
            p.control_border
        }))
        .bg(theme::color(if checked { p.accent } else { p.field }))
        .rounded(px(2.))
        .on_click(listener)
        .child(if checked { "✓" } else { "" })
}

fn access_toggle(access: Access, cx: &mut Context<ConnectionDialog>) -> Stateful<Div> {
    div()
        .id("access-toggle")
        .flex()
        .gap(px(10.))
        .child(
            Button::new("access-rw")
                .ghost()
                .selected(access == Access::Rw)
                .label("Read & write")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.access = Access::Rw;
                    cx.notify();
                })),
        )
        .child(
            Button::new("access-ro")
                .ghost()
                .selected(access == Access::Ro)
                .label("Read-only")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.access = Access::Ro;
                    cx.notify();
                })),
        )
}

fn uri_preview(draft: &ConnectionDraft, domain: &str) -> String {
    let local = draft.protocol == Protocol::Local;
    let user = draft
        .username
        .as_deref()
        .filter(|name| !name.is_empty() && !local)
        .map_or(String::new(), |name| format!("{name}@"));
    let scheme = draft.protocol.scheme();
    let suffix = if draft.share.is_empty() {
        String::new()
    } else {
        format!("/{}", draft.share.trim_start_matches('/'))
    };
    let domain = if draft.protocol == Protocol::Smb && !domain.is_empty() {
        format!("{domain};")
    } else {
        String::new()
    };
    let host = if local { "" } else { draft.host.as_str() };
    format!("{scheme}://{domain}{user}{host}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::{validate_draft, ConnectionDraft, Field, Protocol};

    fn draft(protocol: Protocol) -> ConnectionDraft {
        ConnectionDraft {
            protocol,
            host: "nas.local".to_owned(),
            port: None,
            username: None,
            share: "media".to_owned(),
        }
    }

    #[test]
    fn validation_requires_server_for_network_protocols() {
        let mut value = draft(Protocol::Smb);
        value.host.clear();
        assert!(validate_draft(&value, "")
            .iter()
            .any(|error| error.field == Field::Host));
    }

    #[test]
    fn validation_keeps_local_folder_valid_without_a_host() {
        let mut value = draft(Protocol::Local);
        value.host.clear();
        assert!(validate_draft(&value, "").is_empty());
    }

    #[test]
    fn nfs_identity_is_checked_without_touching_password_state() {
        let mut value = draft(Protocol::Nfs);
        value.username = Some("alice".to_owned());
        assert!(validate_draft(&value, "")
            .iter()
            .any(|error| error.field == Field::Username));
        value.username = Some("1000:1000".to_owned());
        assert!(validate_draft(&value, "").is_empty());
    }
}
