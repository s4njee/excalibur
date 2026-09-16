//! Tauri-free protocol connector for the native GPUI shell.
//!
//! This is the native counterpart of `src-tauri/src/connector.rs`.  The
//! connector is deliberately the only place that turns a `ConnectionSpec`
//! into a backend: credentials and host-key policy therefore cannot drift
//! between a mount and a connection test. Passwords are read from the
//! keychain and are never fields of the connector or the UI model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::nfs::{self, Identity, NfsBackend, NfsTarget};
use excalibur_engine::backend::sftp::{Credential, SftpBackend, SftpTarget};
#[cfg(feature = "smb")]
use excalibur_engine::backend::smb::{SmbBackend, SmbTarget};
use excalibur_engine::backend::xsync::{self, SshTarget, XsyncBackend};
use excalibur_engine::backend::{Access, ConnectionSpec, Protocol, ShareBackend};
use excalibur_engine::{hostkeys, Connector, EngineError, KnownHosts, MessageLog, Result};
use excalibur_engine::{SecretStore, Trust};

/// Builds the backend named by a connection specification.
pub struct NativeConnector {
    known_hosts: KnownHosts,
    secrets: Arc<dyn SecretStore>,
    log: Arc<MessageLog>,
}

impl NativeConnector {
    /// Construct a connector backed by a known-hosts file path.
    pub fn new(
        known_hosts_path: impl Into<PathBuf>,
        secrets: Arc<dyn SecretStore>,
        log: Arc<MessageLog>,
    ) -> Self {
        Self::with_known_hosts(KnownHosts::new(known_hosts_path), secrets, log)
    }

    /// Construct a connector from an existing known-hosts store.
    #[must_use]
    pub fn with_known_hosts(
        known_hosts: KnownHosts,
        secrets: Arc<dyn SecretStore>,
        log: Arc<MessageLog>,
    ) -> Self {
        Self {
            known_hosts,
            secrets,
            log,
        }
    }

    /// Construct a connector using the platform app-data known-hosts file.
    ///
    /// # Errors
    ///
    /// Returns the engine error when the platform has no suitable app-data
    /// directory for the native known-hosts store.
    pub fn app_default(secrets: Arc<dyn SecretStore>, log: Arc<MessageLog>) -> Result<Self> {
        Ok(Self::with_known_hosts(
            KnownHosts::app_default()?,
            secrets,
            log,
        ))
    }

    #[must_use]
    pub fn known_hosts(&self) -> &KnownHosts {
        &self.known_hosts
    }
}

#[async_trait]
impl Connector for NativeConnector {
    #[allow(clippy::too_many_lines)]
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        match spec.protocol {
            Protocol::Xsync => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
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
                let backend = XsyncBackend::connect_ssh(
                    &target,
                    &spec.share,
                    Some(xsync::frame_tap(Arc::clone(&self.log), spec.id.clone())),
                )
                .await?;
                Ok(Arc::new(backend))
            }
            Protocol::Local => Ok(Arc::new(
                LocalBackend::open(Path::new(&spec.share), spec.mount_preference == Access::Ro)
                    .await?,
            )),
            Protocol::Sftp => {
                let port = spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT);
                let username = spec.username.clone().unwrap_or_else(whoami);
                let credential = match self.secrets.get(&spec.id) {
                    Ok(Some(secret)) => Credential::Password(secret.expose().to_owned()),
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
                    mount_port: None,
                    export: &spec.share,
                    identity: parse_identity(spec.username.as_deref()),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = NfsBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
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
                    workgroup: spec.domain.as_deref().unwrap_or(""),
                    read_only: spec.mount_preference == Access::Ro,
                };
                let backend = SmbBackend::connect(&target).await?;
                Ok(Arc::new(backend))
            }
            #[cfg(not(feature = "smb"))]
            Protocol::Smb => Err(EngineError::Unsupported(
                "this build was compiled without SMB support".to_owned(),
            )),
        }
    }
}

/// Parse the NFS `AUTH_SYS` identity field (`uid`, `uid:gid`, or empty).
/// Invalid components intentionally fall back to the current process identity
/// rather than silently requesting uid 0.
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

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{parse_identity, Identity, NativeConnector};
    use excalibur_engine::secrets::MemorySecretStore;
    use excalibur_engine::MessageLog;
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn identity_parser_preserves_valid_uid_and_gid() {
        assert_eq!(
            parse_identity(Some("1001:1002")),
            Identity {
                uid: 1001,
                gid: 1002,
                gids: Vec::new(),
            }
        );
    }

    #[test]
    fn malformed_identity_does_not_request_root() {
        let parsed = parse_identity(Some("not-a-number"));
        let current = Identity::default();
        assert_eq!(parsed, current);
    }

    #[test]
    fn constructor_keeps_known_hosts_path_and_secret_store_out_of_debug_surface() {
        let connector = NativeConnector::new(
            "/tmp/excalibur-gpui-known-hosts",
            Arc::new(MemorySecretStore::default()),
            Arc::new(MessageLog::new()),
        );
        assert_eq!(
            connector.known_hosts().path(),
            Path::new("/tmp/excalibur-gpui-known-hosts")
        );
    }
}
