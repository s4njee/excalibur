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
    /// The host's SSH key has never been accepted on this machine.
    ///
    /// Carries the fingerprint rather than a formatted sentence because the
    /// window has to show it as something the user can compare, character by
    /// character, against what the server operator told them.
    #[error("unknown host key for {host}: {algorithm} {fingerprint}")]
    HostKeyUnknown {
        /// Host as the user typed it.
        host: String,
        /// Port the key belongs to.
        port: u16,
        /// Key type, e.g. `ssh-ed25519`.
        algorithm: String,
        /// `SHA256:` fingerprint, as `ssh-keygen -lf` prints it.
        fingerprint: String,
    },
    /// The host offered a different key from the one already accepted.
    ///
    /// Deliberately not recoverable from where it is reported. This is
    /// indistinguishable from an attack, and a user cannot tell it apart from a
    /// rebuilt server by looking at a dialog, so the only way forward is to
    /// forget the old key in the bookmark editor — a separate, deliberate act.
    #[error("host key for {host} has changed: expected {expected}, got {fingerprint}")]
    HostKeyChanged {
        /// Host as the user typed it.
        host: String,
        /// Port the key belongs to.
        port: u16,
        /// Key type whose key changed.
        algorithm: String,
        /// The fingerprint the host offered now.
        fingerprint: String,
        /// The fingerprint that was accepted previously.
        expected: String,
    },
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

    /// Whether this is a host-key decision the user has to make or act on.
    ///
    /// Both host-key errors stop a connection that is otherwise fine, so a
    /// caller retrying "until it works" needs to know to stop and ask instead.
    #[must_use]
    pub const fn host_key(&self) -> bool {
        matches!(
            self,
            Self::HostKeyUnknown { .. } | Self::HostKeyChanged { .. }
        )
    }

    /// Where this belongs on screen.
    #[must_use]
    pub const fn surface(&self) -> Surface {
        match self {
            Self::HostKeyUnknown { .. } => Surface::TrustPrompt,
            Self::Disconnected(_) | Self::AuthenticationFailed(_) | Self::HostKeyChanged { .. } => {
                Surface::Banner
            }
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
    /// A prompt asking the user to accept or refuse a host's identity.
    ///
    /// Separate from `Banner` because it is a question, not a report: it has to
    /// show a fingerprint and take an answer, and the connection is waiting on
    /// it. A changed key is a `Banner` precisely because it is *not* a question.
    TrustPrompt,
}

/// Engine result.
pub type Result<T> = std::result::Result<T, EngineError>;

#[cfg(test)]
mod tests {
    use super::{EngineError, Surface};

    fn unknown() -> EngineError {
        EngineError::HostKeyUnknown {
            host: "nas.local".to_owned(),
            port: 22,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:abc".to_owned(),
        }
    }

    fn changed() -> EngineError {
        EngineError::HostKeyChanged {
            host: "nas.local".to_owned(),
            port: 22,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: "SHA256:new".to_owned(),
            expected: "SHA256:old".to_owned(),
        }
    }

    #[test]
    fn an_unknown_host_key_is_a_question_and_a_changed_one_is_not() {
        // The whole difference between the two on screen comes from here: one
        // opens a prompt that can be answered, the other raises a banner that
        // cannot. Routing them the same way would put an accept button on a
        // key change, which is the one thing E3-S4 must not do.
        assert_eq!(unknown().surface(), Surface::TrustPrompt);
        assert_eq!(changed().surface(), Surface::Banner);
    }

    #[test]
    fn neither_host_key_failure_is_worth_retrying() {
        // A retry loop would re-run ssh-keyscan against a host whose answer
        // will not change, and would never surface the decision to anyone.
        assert!(!unknown().transient());
        assert!(!changed().transient());
        assert!(unknown().host_key());
        assert!(changed().host_key());
        assert!(!EngineError::NotFound("x".to_owned()).host_key());
    }

    #[test]
    fn the_message_carries_both_fingerprints_when_a_key_changed() {
        // The window reads the structured fields, but the CLI and any log line
        // only get this string, so it has to stand on its own.
        let message = changed().to_string();
        assert!(message.contains("SHA256:old"), "{message}");
        assert!(message.contains("SHA256:new"), "{message}");
    }
}
