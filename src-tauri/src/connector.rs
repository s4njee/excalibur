//! Turning a `ConnectionSpec` into a live backend.
//!
//! One place, so that the credential and host-key handling a protocol needs
//! cannot drift between the command that mounts and the one that tests.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::nfs::{self, Identity, NfsBackend, NfsTarget};
use excalibur_engine::backend::sftp::{Credential, SftpBackend, SftpTarget};
#[cfg(feature = "smb")]
use excalibur_engine::backend::smb::{SmbBackend, SmbTarget};
use excalibur_engine::backend::xsync::{self, SshTarget, XsyncBackend};
use excalibur_engine::backend::{Access, ConnectionSpec, Protocol, ShareBackend};
use excalibur_engine::{
    hostkeys, Connector, EngineError, KnownHosts, MessageLog, Result, SecretStore, Trust,
};

use crate::util::whoami;

/// Builds the backend a `ConnectionSpec` names.
///
/// This is where the protocol enum turns into a client. `connect()` is
/// deliberately not on `ShareBackend`: each protocol needs different things to
/// get started — a keychain entry, a host key, a port — and the shape of that
/// should come from the second real backend rather than be guessed before the
/// first.
pub(crate) struct ProtocolConnector {
    pub(crate) known_hosts: KnownHosts,
    /// Where a stored password comes from. Read here, never in the window.
    pub(crate) secrets: Arc<dyn SecretStore>,
    /// The app's message log, for the one protocol that can show its frames.
    ///
    /// Shared with the session manager rather than owned by it, because a tap
    /// has to be installed *as the client is built* — before the opening
    /// handshake — and that happens here (`twopane.md` XTP-E5-S1).
    pub(crate) log: Arc<MessageLog>,
}

#[async_trait]
impl Connector for ProtocolConnector {
    // One arm per protocol, each of them the credential handling and the
    // target struct that protocol needs, read top to bottom. It went over the
    // lint's hundred lines when SMB landed and E21's local arm is not what
    // would pay for splitting it: the arms share nothing, so factoring them
    // out buys five one-line functions and a longer path to the code.
    #[allow(clippy::too_many_lines)]
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        match spec.protocol {
            Protocol::Xsync => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
                // Decided before `ssh` runs, so an unknown or changed key
                // reaches the window as something it can render rather than as
                // OpenSSH's own prompt, which has no terminal to appear on.
                match hostkeys::verify(&hostkeys::Keyscan, &self.known_hosts, &spec.host, port)
                    .await?
                {
                    Trust::Trusted => {}
                    Trust::Unknown { offered } => {
                        return Err(EngineError::HostKeyUnknown {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                        })
                    }
                    Trust::Changed { offered, stored } => {
                        return Err(EngineError::HostKeyChanged {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                            expected: stored.fingerprint(),
                        })
                    }
                    Trust::Revoked { offered } => {
                        return Err(EngineError::HostKeyChanged {
                            host: spec.host.clone(),
                            port,
                            algorithm: offered.algorithm.clone(),
                            fingerprint: offered.fingerprint(),
                            expected: "a key you revoked".to_owned(),
                        })
                    }
                }
                let target = SshTarget {
                    host: &spec.host,
                    port,
                    username: spec.username.as_deref(),
                    known_hosts: self.known_hosts.path(),
                    rsh: None,
                };
                // Frame resolution for xsync alone: the tap is its client's,
                // and the other protocols have no frames to show. Their mounts
                // keep the verb lines every backend records.
                let backend = XsyncBackend::connect_ssh(
                    &target,
                    &spec.share,
                    Some(xsync::frame_tap(Arc::clone(&self.log), spec.id.clone())),
                )
                .await?;
                Ok(Arc::new(backend))
            }
            // `share` is the folder and `host` is empty: a folder on this
            // machine has no host to name, and inventing one would put a
            // reachability question on a bookmark that cannot fail (E21-S2).
            Protocol::Local => Ok(Arc::new(
                LocalBackend::open(Path::new(&spec.share), spec.mount_preference == Access::Ro)
                    .await?,
            )),
            Protocol::Sftp => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
                let username = spec.username.clone().unwrap_or_else(whoami);
                // The credential is read from the keychain here rather than
                // being passed in from the window: that is what keeps E3-S3's
                // rule true — the window collects a password once and never
                // sees it again.
                let credential = match self.secrets.get(&spec.id) {
                    Ok(Some(secret)) => Credential::Password(secret.expose().to_owned()),
                    // No stored password means the agent, which is what most
                    // SSH users actually have, and needs nothing typed.
                    _ => Credential::Agent,
                };
                let target = SftpTarget {
                    host: &spec.host,
                    port,
                    username: &username,
                    root: &spec.share,
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend =
                    SftpBackend::connect(&target, &credential, Arc::new(self.known_hosts.clone()))
                        .await?;
                Ok(Arc::new(backend))
            }
            Protocol::Nfs => {
                let target = NfsTarget {
                    host: &spec.host,
                    port: spec.port.unwrap_or(nfs::DEFAULT_NFS_PORT),
                    // The portmapper names `mountd`'s port, which is right
                    // except behind a port mapping, where it reports the port
                    // inside. Nothing in the dialog offers that yet.
                    mount_port: None,
                    export: &spec.share,
                    // NFS has no password. The dialog calls this field
                    // "Identity" and it carries `uid:gid`, because AUTH_SYS is
                    // an assertion rather than a login — there is no secret to
                    // read from the keychain here, and nothing to read it with.
                    identity: parse_identity(spec.username.as_deref()),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = NfsBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
            // Named rather than caught by a wildcard, so that adding a
            // protocol to the enum breaks this match instead of silently
            // routing it here. Two hangs in the sibling xsync repo came from
            // exactly that shape of arm.
            #[cfg(feature = "smb")]
            Protocol::Smb => {
                let username = spec.username.clone().unwrap_or_else(whoami);
                let password = match self.secrets.get(&spec.id) {
                    Ok(Some(secret)) => secret.expose().to_owned(),
                    _ => String::new(),
                };
                let target = SmbTarget {
                    host: &spec.host,
                    port: spec.port,
                    share: &spec.share,
                    username: &username,
                    password: &password,
                    // The dialog's Domain field, which only SMB has.
                    workgroup: spec.domain.as_deref().unwrap_or(""),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = SmbBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
            // Built only with the `smb` feature, because linking libsmbclient
            // makes the binary GPL-3.0-or-later. Without it, a spec naming SMB
            // is answered honestly rather than silently mocked.
            #[cfg(not(feature = "smb"))]
            Protocol::Smb => Err(excalibur_engine::EngineError::Unsupported(
                "this build was compiled without SMB support".to_owned(),
            )),
        }
    }
}

/// `uid`, `uid:gid`, or nothing at all.
///
/// Anything unparseable falls back to this user rather than to root: claiming
/// uid 0 is how `root_squash` maps you to `nobody`, and the resulting refusal
/// would have nothing to do with what the user typed.
fn parse_identity(field: Option<&str>) -> Identity {
    let Some(text) = field.map(str::trim).filter(|text| !text.is_empty()) else {
        return Identity::default();
    };
    let (uid, gid) = match text.split_once(':') {
        Some((uid, gid)) => (uid.trim().parse().ok(), gid.trim().parse().ok()),
        None => (text.parse().ok(), None),
    };
    let default = Identity::default();
    Identity {
        uid: uid.unwrap_or(default.uid),
        gid: gid.unwrap_or(default.gid),
        gids: Vec::new(),
    }
}
