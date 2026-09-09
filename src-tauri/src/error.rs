//! What a failed command looks like on the other side of the bridge.

use excalibur_engine::EngineError;

/// Errors as the frontend sees them: a message and where it belongs on screen.
///
/// The engine's error type does not cross IPC, so a new variant there cannot
/// silently become an untyped string here.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    pub(crate) message: String,
    pub(crate) surface: excalibur_engine::error::Surface,
    pub(crate) transient: bool,
    /// Set for the two host-key failures.
    ///
    /// The message already says all of this in a sentence, but a fingerprint is
    /// something the user compares character by character against what the
    /// server operator told them. That needs its own element, in a monospace
    /// font, not a substring of a paragraph — so it travels as data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) host_key: Option<HostKeyProblem>,
}

/// The host-key detail behind a [`CommandError`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostKeyProblem {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) algorithm: String,
    pub(crate) fingerprint: String,
    /// The previously accepted fingerprint, when the key changed. `None` means
    /// the host is simply unknown, which is a question rather than an alarm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expected: Option<String>,
}

impl CommandError {
    /// A message with nowhere better to go than the window's banner.
    ///
    /// Not transient, and carrying no host key: the shape every command that
    /// fails on something local — a folder that is not there, a window that
    /// would not open, a drag that could not start — was writing out by hand.
    pub(crate) fn banner(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            surface: excalibur_engine::error::Surface::Banner,
            transient: false,
            host_key: None,
        }
    }
}

impl From<excalibur_engine::EngineError> for CommandError {
    fn from(error: excalibur_engine::EngineError) -> Self {
        let host_key = match &error {
            EngineError::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint,
            } => Some(HostKeyProblem {
                host: host.clone(),
                port: *port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                expected: None,
            }),
            EngineError::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint,
                expected,
            } => Some(HostKeyProblem {
                host: host.clone(),
                port: *port,
                algorithm: algorithm.clone(),
                fingerprint: fingerprint.clone(),
                expected: Some(expected.clone()),
            }),
            _ => None,
        };
        Self {
            message: error.to_string(),
            surface: error.surface(),
            transient: error.transient(),
            host_key,
        }
    }
}
