//! Native transfer-worker view of the GPUI session manager.
//!
//! The transfer worker intentionally depends on the small `BackendSource`
//! trait rather than on a window or on Tauri state.  This adapter keeps that
//! boundary just as narrow for the native shell: browse sessions remain owned
//! by `SessionManager`, while xsync gets an owned route that carries the same
//! known-hosts policy as the connector.

use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::{Protocol, ShareBackend};
use excalibur_engine::transfer::worker::BackendSource;
use excalibur_engine::transfer::xsync_engine::{SshEndpoint, XsyncRemote, XsyncRoute};
use excalibur_engine::{hostkeys, EngineError, KnownHosts, Result, SessionManager};

/// A Tauri-free backend source for a native transfer worker.
///
/// `SessionManager` is shared with `Services`; no second connector or session
/// registry is created here.  Consequently a worker sees reconnects and
/// server-derived read-only state from the same source as the browser.
pub struct NativeBackendSource {
    sessions: Arc<SessionManager>,
    known_hosts: KnownHosts,
}

impl NativeBackendSource {
    /// Construct a source using the service layer's session manager and the
    /// app-owned SSH trust store.
    #[must_use]
    pub fn new(sessions: Arc<SessionManager>, known_hosts: KnownHosts) -> Self {
        Self {
            sessions,
            known_hosts,
        }
    }
}

#[async_trait]
impl BackendSource for NativeBackendSource {
    async fn active(&self) -> Result<Arc<dyn ShareBackend>> {
        self.sessions.recent_backend().await
    }

    /// Prefer the mount named by a transfer row. Legacy rows with no mount
    /// retain the app-wide recent-mount behaviour.
    async fn backend_for(&self, mount: Option<&str>) -> Result<Arc<dyn ShareBackend>> {
        let Some(id) = mount else {
            return self.sessions.recent_backend().await;
        };
        self.sessions
            .backend(id)
            .await
            .ok_or_else(|| EngineError::Disconnected(format!("{id} is not connected")))
    }

    /// A disconnected row remains queued unless its xsync route can open its
    /// own strictly verified SSH session.
    async fn is_connected(&self, mount: Option<&str>) -> bool {
        self.backend_for(mount).await.is_ok() || self.xsync_route(mount).await.is_some()
    }

    /// Build the independent xsync route for an xsync mount.  The route uses
    /// the same known-hosts file as browse connections and never carries a
    /// password or keychain value.
    async fn xsync_route(&self, mount: Option<&str>) -> Option<XsyncRoute> {
        let id = match mount {
            Some(id) => id.to_owned(),
            None => self.sessions.recent().await?,
        };
        let spec = self.sessions.spec(&id).await?;
        if spec.protocol != Protocol::Xsync {
            return None;
        }
        let mount_info = self.sessions.mount_info(&id).await;
        Some(XsyncRoute {
            remote: XsyncRemote::Ssh(SshEndpoint {
                host: spec.host.clone(),
                port: spec.port.unwrap_or(hostkeys::DEFAULT_SSH_PORT),
                username: spec.username.clone(),
                known_hosts: self.known_hosts.path().to_path_buf(),
                rsh: None,
            }),
            export_root: spec.share.clone(),
            writable: mount_info.as_ref().is_some_and(|info| info.writable),
            reason: mount_info.map_or_else(
                || "the mount has not reported its access yet".to_owned(),
                |info| info.reason,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::NativeBackendSource;
    use async_trait::async_trait;
    use excalibur_engine::backend::{mock::MockBackend, Access, Protocol, ShareBackend};
    use excalibur_engine::transfer::queue::TransferQueue;
    use excalibur_engine::transfer::worker::{BackendSource, Worker};
    use excalibur_engine::{ConnectionSpec, Connector, KnownHosts, Result, SessionManager};
    use std::sync::Arc;

    struct FixedConnector {
        backend: Arc<dyn ShareBackend>,
    }

    #[async_trait]
    impl Connector for FixedConnector {
        async fn connect(&self, _spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
            Ok(Arc::clone(&self.backend))
        }
    }

    fn spec(id: &str, protocol: Protocol) -> ConnectionSpec {
        ConnectionSpec {
            id: id.to_owned(),
            name: id.to_owned(),
            protocol,
            host: "example.test".to_owned(),
            port: None,
            share: "/export".to_owned(),
            username: Some("alice".to_owned()),
            domain: None,
            mount_preference: Access::Ro,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn routes_named_and_recent_mock_mounts() {
        let sessions = Arc::new(SessionManager::new(Arc::new(FixedConnector {
            backend: Arc::new(MockBackend::media_share()),
        })));
        let mounted = sessions.mount("gpui", spec("mock", Protocol::Local)).await;
        assert!(mounted.is_ok());
        let source = NativeBackendSource::new(
            Arc::clone(&sessions),
            KnownHosts::new(std::env::temp_dir().join("excalibur-gpui-test-known-hosts")),
        );
        assert!(source.active().await.is_ok());
        assert!(source.backend_for(Some("mock")).await.is_ok());
        assert!(source.is_connected(Some("mock")).await);
        assert!(source.xsync_route(Some("mock")).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn worker_can_be_constructed_without_tauri_state() {
        let queue = Arc::new(TransferQueue::in_memory());
        let sessions = Arc::new(SessionManager::new(Arc::new(FixedConnector {
            backend: Arc::new(MockBackend::media_share()),
        })));
        let source = Arc::new(NativeBackendSource::new(
            sessions,
            KnownHosts::new(std::env::temp_dir().join("excalibur-gpui-test-known-hosts")),
        ));
        let worker = Worker::new(queue, source, 2);
        assert_eq!(worker.concurrency(), 2);
    }
}
