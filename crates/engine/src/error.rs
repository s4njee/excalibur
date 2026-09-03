//! One error taxonomy, classified the way the UI has to react to it.

/// What went wrong, and — more usefully — where the answer belongs on screen.
///
/// The design routes failures three ways: a per-item failure becomes a row in
/// the transfer queue, a connection-level failure becomes a banner under the
/// toolbar, and a validation failure is inline on the field that caused it.
/// Nothing becomes a modal alert. That routing is a property of the error, so
/// it lives here rather than being re-derived at each call site.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The path does not exist.
    #[error("no such file or directory: {0}")]
    NotFound(String),
    /// The server refused on permission grounds.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// A write was attempted against a mount that is not writable.
    ///
    /// Reaching this is a bug in the UI's gating, not a normal outcome: the
    /// affordance should have been disabled. It is still refused here, because
    /// defence in depth is cheaper than trusting a signal from the frontend.
    #[error("read-only mount: {0}")]
    ReadOnly(String),
    /// The path already exists and the caller asked for exclusive creation.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// The connection dropped or was never established.
    #[error("connection lost: {0}")]
    Disconnected(String),
    /// Authentication was rejected.
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    /// The request was malformed, or asked for something this backend cannot do.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Anything else the transport reported.
    #[error("io: {0}")]
    Io(String),
}

impl EngineError {
    /// Whether retrying unchanged could plausibly succeed.
    ///
    /// The transfer worker retries transient failures with backoff and gives a
    /// fatal one a `Retry` button instead, so this decides which of the two a
    /// user sees.
    #[must_use]
    pub const fn transient(&self) -> bool {
        matches!(self, Self::Disconnected(_) | Self::Io(_))
    }

    /// Where this belongs on screen.
    #[must_use]
    pub const fn surface(&self) -> Surface {
        match self {
            Self::Disconnected(_) | Self::AuthenticationFailed(_) => Surface::Banner,
            Self::Unsupported(_) => Surface::Field,
            _ => Surface::TransferRow,
        }
    }
}

/// Where a failure is shown. See [`EngineError::surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// A row in the transfer queue.
    TransferRow,
    /// The connection banner under the toolbar.
    Banner,
    /// Inline on the field that caused it.
    Field,
}

/// Engine result.
pub type Result<T> = std::result::Result<T, EngineError>;
