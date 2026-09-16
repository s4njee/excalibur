//! The native service bridge used by the GPUI shell.
//!
//! GPUI's executor is deliberately not assumed to be Tokio's executor.  The
//! engine, on the other hand, uses Tokio for network I/O and for the local
//! backend's blocking filesystem work.  [`Services`] owns one small Tokio
//! runtime and turns each operation into a sendable future.  A GPUI model can
//! hand that future to `cx.spawn` and remains entirely free of runtime handles
//! and file contents.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use excalibur_engine::backend::local::LocalBackend;
use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::backend::{
    Access, Attrs, ConnectionSpec, Listing, MountInfo, Protocol, ShareBackend,
};
use excalibur_engine::transfer::queue::TransferQueue;
use excalibur_engine::transfer::worker::Worker;
use excalibur_engine::{
    Connector, EngineError, KnownHosts, Result, Secret, SecretStore, SessionEvent, SessionManager,
};
use tokio::net::TcpListener;
use tokio::runtime::{Builder, Runtime};

use crate::backend_source::NativeBackendSource;
use crate::media_stream::{serve, token, MediaStream};

/// The default logical window name for the single native browser window.
pub const GPUI_WINDOW: &str = "gpui";

/// Maximum bytes one preview/range request may ask a backend for.
pub const MAX_PREVIEW_READ: u32 = 4 * 1024 * 1024;

/// A future that can be awaited by a GPUI background task.
pub type ServiceTask<T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'static>>;

/// A successful connection probe. The spec is returned only after the
/// connector has completed protocol setup, making this suitable for creating
/// a bookmark without ever carrying a credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionTestResult {
    pub spec: ConnectionSpec,
    pub mount: MountInfo,
}

/// Builds one engine backend from a connection specification.
#[async_trait]
pub trait BackendFactory: Send + Sync {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>>;
}

/// A factory that always returns one already-open backend.
///
/// This is useful for the deterministic mock screen and for embedding a
/// caller-owned backend in a native host. The backend is shared, but all
/// engine methods remain behind `ShareBackend`'s synchronization contract.
pub struct FixedBackend {
    backend: Arc<dyn ShareBackend>,
}

impl FixedBackend {
    #[must_use]
    pub fn new(backend: Arc<dyn ShareBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl BackendFactory for FixedBackend {
    async fn connect(&self, _spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        Ok(Arc::clone(&self.backend))
    }
}

/// A local-folder factory. `ConnectionSpec::share` is the folder path.
pub struct LocalFactory {
    read_only: bool,
}

impl LocalFactory {
    #[must_use]
    pub const fn new(read_only: bool) -> Self {
        Self { read_only }
    }
}

#[async_trait]
impl BackendFactory for LocalFactory {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        let read_only = self.read_only || spec.mount_preference == Access::Ro;
        let backend = LocalBackend::open(Path::new(&spec.share), read_only).await?;
        Ok(Arc::new(backend))
    }
}

/// An honest factory for protocols not yet wired into the native shell.
pub struct UnsupportedFactory;

#[async_trait]
impl BackendFactory for UnsupportedFactory {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        Err(EngineError::Unsupported(format!(
            "native GPUI connector does not support {} yet",
            spec.protocol.scheme()
        )))
    }
}

/// A small Tokio-backed facade over [`SessionManager`].
pub struct Services {
    runtime: Option<Runtime>,
    sessions: Arc<SessionManager>,
    secrets: Option<Arc<dyn SecretStore>>,
}

impl Services {
    /// Create services around a connector. Runtime construction is synchronous
    /// but does no network or filesystem work.
    ///
    /// # Errors
    ///
    /// Returns the runtime's I/O error when Tokio cannot create its worker
    /// threads or drivers.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the Arc is cloned into the runtime-owned connector"
    )]
    pub fn new(factory: Arc<dyn BackendFactory>) -> std::io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .thread_name("excalibur-gpui")
            .build()?;
        let connector: Arc<dyn Connector> = Arc::new(FactoryConnector {
            factory: Arc::clone(&factory),
        });
        let sessions = Arc::new(SessionManager::new(connector));
        Ok(Self {
            runtime: Some(runtime),
            sessions,
            secrets: None,
        })
    }

    /// Build from the engine connector supplied by a native host. This is the
    /// seam where a shell can reuse its credential/host-key policy without
    /// importing that shell's types into the model.
    ///
    /// # Errors
    ///
    /// Returns the runtime construction error.
    pub fn from_connector(connector: Arc<dyn Connector>) -> std::io::Result<Self> {
        Self::from_connector_with_secrets(connector, None)
    }

    /// Build from a connector and the same secure store used by that
    /// connector. Keeping this store in the service layer lets the dialog
    /// hand a password to the shell once, without putting it in model state.
    ///
    /// # Errors
    ///
    /// Returns the runtime construction error.
    pub fn from_connector_with_secrets(
        connector: Arc<dyn Connector>,
        secrets: Option<Arc<dyn SecretStore>>,
    ) -> std::io::Result<Self> {
        let mut services = Self::new(Arc::new(ConnectorFactory { connector }))?;
        services.secrets = secrets;
        Ok(services)
    }

    /// Deterministic design fixture, with the same media rows as the engine's
    /// headless mock backend.
    ///
    /// # Errors
    ///
    /// Returns the runtime construction error.
    pub fn mock() -> std::io::Result<Self> {
        Self::new(Arc::new(FixedBackend::new(Arc::new(
            MockBackend::media_share(),
        ))))
    }

    /// Browse one local folder through the real engine local backend.
    ///
    /// # Errors
    ///
    /// Returns the runtime construction error.
    pub fn local(read_only: bool) -> std::io::Result<Self> {
        Self::new(Arc::new(LocalFactory::new(read_only)))
    }

    /// Expose session events to a native shell. The receiver can be polled on
    /// the service runtime or forwarded to a GPUI model by the host.
    #[must_use]
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<SessionEvent> {
        self.sessions.events()
    }

    /// Mount a share and make it active for the native window.
    pub fn mount(&self, spec: ConnectionSpec) -> ServiceTask<MountInfo> {
        let sessions = Arc::clone(&self.sessions);
        self.submit(async move {
            sessions.mount(GPUI_WINDOW, spec).await?;
            let id = sessions.active(GPUI_WINDOW).await.ok_or_else(|| {
                EngineError::Disconnected("mount completed without an active share".to_owned())
            })?;
            sessions.mount_info(&id).await.ok_or_else(|| {
                EngineError::Disconnected("mount completed without mount facts".to_owned())
            })
        })
    }

    /// List one directory. Mount facts travel with the rows by engine design.
    pub fn list(&self, path: impl Into<String>) -> ServiceTask<Listing> {
        let sessions = Arc::clone(&self.sessions);
        let path = path.into();
        self.submit(async move {
            let backend = sessions.active_backend(GPUI_WINDOW).await?;
            backend.list_dir(&path).await
        })
    }

    /// Read metadata for one path from the currently active mount.
    pub fn stat(&self, path: impl Into<String>) -> ServiceTask<Attrs> {
        let sessions = Arc::clone(&self.sessions);
        let path = path.into();
        self.submit(async move {
            let backend = sessions.active_backend(GPUI_WINDOW).await?;
            backend.stat(&path).await
        })
    }

    /// Read a bounded range from the currently active mount. Callers that
    /// need a larger transfer must use the transfer service instead.
    pub fn read_range(
        &self,
        path: impl Into<String>,
        offset: u64,
        length: u32,
    ) -> ServiceTask<Vec<u8>> {
        if length > MAX_PREVIEW_READ {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "requested preview range exceeds the native cap".to_owned(),
                ))
            });
        }
        let sessions = Arc::clone(&self.sessions);
        let path = path.into();
        self.submit(async move {
            let backend = sessions.active_backend(GPUI_WINDOW).await?;
            backend.read_range(&path, offset, length).await
        })
    }

    /// Read a complete bounded file through the active backend in chunks.
    ///
    /// This is intended for image previews.  The caller supplies the size
    /// obtained from metadata so a mistaken or hostile value cannot make the
    /// service allocate an unbounded buffer.
    pub fn read_file_bounded(
        &self,
        path: impl Into<String>,
        size: u64,
        max: u64,
    ) -> ServiceTask<Vec<u8>> {
        let Ok(capacity) = usize::try_from(size) else {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "requested file exceeds the native preview cap".to_owned(),
                ))
            });
        };
        if size > max {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "requested file exceeds the native preview cap".to_owned(),
                ))
            });
        }
        let sessions = Arc::clone(&self.sessions);
        let path = path.into();
        self.submit(async move {
            let backend = sessions.active_backend(GPUI_WINDOW).await?;
            let mut result = Vec::with_capacity(capacity);
            let mut offset = 0_u64;
            while offset < size {
                let length = u32::try_from((size - offset).min(u64::from(MAX_PREVIEW_READ)))
                    .map_err(|_| EngineError::Io("preview chunk length overflowed".to_owned()))?;
                let bytes = backend.read_range(&path, offset, length).await?;
                if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > u64::from(length) {
                    return Err(EngineError::Io(
                        "backend returned more bytes than requested".to_owned(),
                    ));
                }
                let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                result.extend_from_slice(&bytes);
                offset += read;
                if read < u64::from(length) {
                    break;
                }
            }
            Ok(result)
        })
    }

    /// Create a temporary seekable localhost URL for one media file.
    ///
    /// The active backend must support random reads.  The returned server
    /// stays alive while any clone of [`MediaStream`] exists.
    pub fn media_stream(
        &self,
        path: impl Into<String>,
        size: u64,
        mime: impl Into<String>,
    ) -> ServiceTask<MediaStream> {
        let sessions = Arc::clone(&self.sessions);
        let path = path.into();
        let mime = mime.into();
        self.submit(async move {
            let backend = sessions.active_backend(GPUI_WINDOW).await?;
            if !backend.caps().random_access {
                return Err(EngineError::Unsupported(
                    "media preview requires random-access backend reads".to_owned(),
                ));
            }
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|error| EngineError::Io(format!("media stream bind failed: {error}")))?;
            let port = listener
                .local_addr()
                .map_err(|error| EngineError::Io(format!("media stream address failed: {error}")))?
                .port();
            let token_path = format!("/{}", token());
            let url = format!("http://127.0.0.1:{port}{token_path}");
            let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
            let safe_mime = if mime.is_empty()
                || mime
                    .chars()
                    .any(|character| character == '\r' || character == '\n')
            {
                "application/octet-stream".to_owned()
            } else {
                mime
            };
            tokio::spawn(serve(
                listener,
                backend,
                path,
                size,
                safe_mime,
                token_path,
                shutdown_receiver,
            ));
            Ok(MediaStream::new(url, shutdown_sender))
        })
    }

    /// Probe a connection without registering a session or changing the
    /// active share. The connector performs protocol handshakes; the engine
    /// reads mount facts before returning the bookmark-safe result.
    pub fn test_connection(&self, spec: ConnectionSpec) -> ServiceTask<ConnectionTestResult> {
        let sessions = Arc::clone(&self.sessions);
        self.submit(async move {
            let mount = sessions.test(&spec).await?;
            Ok(ConnectionTestResult { spec, mount })
        })
    }

    /// Save one credential in the configured OS secret store.
    pub fn save_secret(&self, account: impl Into<String>, secret: Secret) -> ServiceTask<()> {
        let account = account.into();
        let Some(store) = self.secrets.as_ref().map(Arc::clone) else {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "no native secret store is configured".to_owned(),
                ))
            });
        };
        self.submit(async move { store.set(&account, &secret) })
    }

    /// Remove one credential from the configured OS secret store.
    pub fn delete_secret(&self, account: impl Into<String>) -> ServiceTask<()> {
        let account = account.into();
        let Some(store) = self.secrets.as_ref().map(Arc::clone) else {
            return Box::pin(async {
                Err(EngineError::Unsupported(
                    "no native secret store is configured".to_owned(),
                ))
            });
        };
        self.submit(async move { store.delete(&account) })
    }

    /// Reconnect the active share without changing its requested access.
    pub fn reconnect(&self, id: impl Into<String>) -> ServiceTask<MountInfo> {
        let sessions = Arc::clone(&self.sessions);
        let id = id.into();
        self.submit(async move { sessions.reconnect(&id).await })
    }

    /// Drop a share. Existing listing requests may still complete, so the
    /// model's generation token—not cancellation timing—decides what is drawn.
    pub fn unmount(&self, id: impl Into<String>) -> ServiceTask<()> {
        let sessions = Arc::clone(&self.sessions);
        let id = id.into();
        self.submit(async move {
            sessions.unmount(&id).await;
            Ok(())
        })
    }

    /// The active mount, if one exists.
    pub fn mount_info(&self) -> ServiceTask<Option<MountInfo>> {
        let sessions = Arc::clone(&self.sessions);
        self.submit(async move {
            let id = sessions.active(GPUI_WINDOW).await;
            match id {
                Some(id) => Ok(sessions.mount_info(&id).await),
                None => Ok(None),
            }
        })
    }

    /// Build a transfer worker over this service's session manager.
    ///
    /// The worker is returned stopped; callers decide when to call
    /// [`Worker::drain`] after restoring the queue and attaching their GPUI
    /// event watcher.  Passing the app's [`KnownHosts`] store explicitly keeps
    /// xsync's second SSH process under the same host-key policy as browse
    /// connections.  HTTP uses the engine's safe, credential-free defaults;
    /// callers with process-only HTTP settings can apply them through
    /// [`Worker::set_http_config`].
    #[must_use]
    pub fn start_transfer_worker(
        &self,
        queue: Arc<TransferQueue>,
        known_hosts: KnownHosts,
        concurrency: usize,
    ) -> Arc<Worker> {
        let source = Arc::new(NativeBackendSource::new(
            Arc::clone(&self.sessions),
            known_hosts,
        ));
        Arc::new(Worker::new(queue, source, concurrency.max(1)))
    }

    /// Run one worker drain on the service runtime.
    ///
    /// The engine's drain claims only `Queued` rows. In particular, this does
    /// not turn restored `Paused` rows back into work; that transition belongs
    /// to the explicit Resume action in the transfer panel.
    pub fn start_worker(&self, worker: Arc<Worker>) -> ServiceTask<()> {
        self.submit(async move {
            worker.drain().await;
            Ok(())
        })
    }

    /// Construct a local spec suitable for [`Self::mount`].
    #[must_use]
    pub fn local_spec(path: impl Into<String>, read_only: bool) -> ConnectionSpec {
        let share = path.into();
        ConnectionSpec {
            id: format!("local:{share}"),
            name: Path::new(&share)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Local folder")
                .to_owned(),
            protocol: Protocol::Local,
            host: String::new(),
            port: None,
            share,
            username: None,
            domain: None,
            mount_preference: if read_only { Access::Ro } else { Access::Rw },
        }
    }

    /// Construct a stable fixture spec suitable for [`Self::mock`].
    #[must_use]
    pub fn mock_spec() -> ConnectionSpec {
        ConnectionSpec {
            id: "mock:media".to_owned(),
            name: "Media (mock)".to_owned(),
            protocol: Protocol::Local,
            host: String::new(),
            port: None,
            share: String::new(),
            username: None,
            domain: None,
            mount_preference: Access::Rw,
        }
    }

    pub(crate) fn submit<T, F>(&self, operation: F) -> ServiceTask<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.runtime
            .as_ref()
            .expect("service runtime is alive")
            .spawn(async move {
                let _ = sender.send(operation.await);
            });
        Box::pin(async move {
            receiver.await.map_err(|_| {
                EngineError::Disconnected("native service runtime stopped".to_owned())
            })?
        })
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        // `Services` is normally dropped by GPUI, but tests and embedders may
        // release it while another async runtime is on the current thread.
        // `Runtime::drop` would panic in that situation; background shutdown
        // still stops the worker threads without blocking the UI/runtime.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

struct FactoryConnector {
    factory: Arc<dyn BackendFactory>,
}

#[async_trait]
impl Connector for FactoryConnector {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        self.factory.connect(spec).await
    }
}

struct ConnectorFactory {
    connector: Arc<dyn Connector>,
}

#[async_trait]
impl BackendFactory for ConnectorFactory {
    async fn connect(&self, spec: &ConnectionSpec) -> Result<Arc<dyn ShareBackend>> {
        self.connector.connect(spec).await
    }
}

#[cfg(test)]
mod tests {
    use super::{Access, Protocol, Services};

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn mock_service_mounts_and_lists_without_returning_file_bytes() {
        let services = Services::mock().expect("runtime");
        let mount = services.mount(Services::mock_spec()).await.expect("mount");
        assert_eq!(mount.protocol_label, "SMB3");
        let listing = services.list("").await.expect("listing");
        assert!(listing.entries.iter().any(|entry| entry.name == "Series"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_connection_does_not_register_an_active_mount() {
        let services = Services::mock().expect("runtime");
        let result = services
            .test_connection(Services::mock_spec())
            .await
            .expect("connection test");
        assert_eq!(result.mount.protocol_label, "SMB3");
        assert!(services.mount_info().await.expect("mount query").is_none());
    }

    #[test]
    fn local_spec_keeps_path_in_share_and_never_in_host() {
        let spec = Services::local_spec("/tmp/media", true);
        assert_eq!(spec.protocol, Protocol::Local);
        assert_eq!(spec.share, "/tmp/media");
        assert!(spec.host.is_empty());
        assert_eq!(spec.mount_preference, Access::Ro);
    }
}
