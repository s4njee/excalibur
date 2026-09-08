//! HTTP-to-local transfers (XDL-E1-S1).
//!
//! An HTTP download is deliberately a thin route into xsync's planner. The
//! source is `xsync_download::HttpSource`; the destination is xsync's
//! `LocalSink`, whose staged writes and local resume journal are the same ones
//! used by the existing local transfer route. A `ShareSink` would lose the
//! journal's ability to prove that its stage survived a restart.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use xsync_core::local::{sync_endpoints, LocalEvent, LocalSyncOptions, LocalSyncReport};
use xsync_core::sink::LocalSink;
use xsync_download::{ClientConfig, HttpSource, MirrorSource, RetryPolicy, UreqClient};
use xsync_endpoint::Source;
use xsync_endpoint::{CancelToken, EndpointError, EndpointErrorKind};

use super::xsync_engine::{Moved, Settled, Tally};
use super::{Endpoint, Transfer};
pub use super::{HttpDownloadLimits, HttpDownloadOptions, HttpProbe};
use crate::error::{EngineError, Result};

const MAX_CONNECTIONS: usize = 16;
const MAX_PER_HOST: usize = 4;
static ORIGIN_LIMITS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> =
    OnceLock::new();

fn origin_semaphore(origin: &str) -> Arc<tokio::sync::Semaphore> {
    let mut limits = ORIGIN_LIMITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    limits
        .entry(origin.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(MAX_PER_HOST)))
        .clone()
}

async fn origin_permits(
    origin: &str,
    count: usize,
    stop: &AtomicBool,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>> {
    let semaphore = origin_semaphore(origin);
    let count = u32::try_from(count.clamp(1, MAX_PER_HOST)).unwrap_or(1);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        match Arc::clone(&semaphore).try_acquire_many_owned(count) {
            Ok(permit) => return Ok(Some(permit)),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                tokio::time::sleep(std::time::Duration::from_millis(CANCEL_POLL_MS)).await;
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(EngineError::Io("HTTP origin limiter closed".to_owned()));
            }
        }
    }
}

async fn probe_permits(
    urls: &[String],
    stop: &AtomicBool,
) -> Result<Option<Vec<tokio::sync::OwnedSemaphorePermit>>> {
    let origins = urls
        .iter()
        .map(|url| xsync_download::origin_of(url))
        .collect::<BTreeSet<_>>();
    let mut permits = Vec::with_capacity(origins.len());
    for origin in origins {
        let Some(permit) = origin_permits(&origin, 1, stop).await? else {
            return Ok(None);
        };
        permits.push(permit);
    }
    Ok(Some(permits))
}

#[must_use]
pub const fn download_limits() -> HttpDownloadLimits {
    HttpDownloadLimits {
        max_connections: MAX_CONNECTIONS,
        max_per_host: MAX_PER_HOST,
    }
}

fn effective_connections(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(MAX_PER_HOST)
        .clamp(1, MAX_CONNECTIONS.min(MAX_PER_HOST))
}

/// Probe an HTTP URL synchronously. Network failures are represented by a warning;
/// only malformed/unsupported URLs are returned as errors.
///
/// # Errors
///
/// Returns an error for a malformed URL or when the HTTP adapter cannot issue
/// the probe.
pub fn probe_download(url: &str) -> Result<HttpProbe> {
    super::validate_http_url(url)?;
    let config = ClientConfig::new().timeout(std::time::Duration::from_secs(30));
    let client =
        UreqClient::with_config(config).map_err(|error| EngineError::Io(error.to_string()))?;
    let probe = xsync_download::probe(url.to_owned(), Arc::new(client));
    probe
        .map(|p| HttpProbe {
            length: p.length,
            ranges: p.ranges,
            filename: Some(p.filename.to_string()),
            final_url: Some(p.final_url),
            warning: p.warning,
        })
        .map_err(|error| endpoint_to_engine(&error))
}

const CANCEL_POLL_MS: u64 = 100;

/// Map one HTTP failure onto Excalibur's taxonomy without losing whether it is
/// permanent.
///
/// `HttpSource` spends its own retry budget on transient transport failures
/// (xsync-downloader.md XD-E7-S2), so a failure that escapes it is either
/// permanent — a 404, a refused URL, a range the object cannot satisfy — or a
/// transport that outlasted five attempts. The worker's generic share retry
/// (a second 30s of backoff) must not be spent on the permanent half: a 404 is
/// a statement the server will not change, and a refused URL is a row the user
/// should be told to fix, not one that sits "running" retrying itself.
fn endpoint_to_engine(error: &EndpointError) -> EngineError {
    match error.kind() {
        EndpointErrorKind::NotFound => EngineError::NotFound(error.to_string()),
        EndpointErrorKind::PermissionDenied => EngineError::PermissionDenied(error.to_string()),
        // Transport, genuinely worth one more worker attempt after the source's
        // own budget is spent.
        EndpointErrorKind::Io | EndpointErrorKind::Other => EngineError::Io(error.to_string()),
        // The request itself is wrong and will be wrong again: a path that
        // cannot be named, a range outside the object, a digest that failed, a
        // capability this origin does not have.
        _ => EngineError::Unsupported(error.to_string()),
    }
}

fn journal_root() -> Option<PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("excalibur").join("resume"))
}

fn options(cancel: CancelToken) -> LocalSyncOptions {
    LocalSyncOptions {
        checksum: false,
        delete: false,
        dry_run: false,
        max_delete: Some(0),
        paranoid: false,
        journal_root: journal_root(),
        cancel,
        ..LocalSyncOptions::default()
    }
}

fn parent(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Validate and resolve the local destination after probing the origin.
///
/// The caller's destination is a folder. `HttpSource` owns all filename
/// sanitisation, including `Content-Disposition`; this function only joins
/// that safe single component to the requested folder.
///
/// # Errors
///
/// Returns [`EngineError::Unsupported`] when the URL is not `http`/`https`,
/// and an I/O error when the HTTP client cannot be built.
pub fn resolved_destination(
    url: &str,
    destination: &Path,
    cancel: CancelToken,
) -> Result<(HttpSource, PathBuf)> {
    let config = ClientConfig::new().timeout(std::time::Duration::from_secs(30));
    let client =
        UreqClient::with_config(config).map_err(|error| EngineError::Io(error.to_string()))?;
    let source =
        HttpSource::new(url, None, Arc::new(client)).map_err(|error| endpoint_to_engine(&error))?;
    let source = source.with_cancel(cancel);
    let name = source.output_name().to_string();
    let path = destination.join(name);
    Ok((source, path))
}

fn run_blocking(
    source: &dyn xsync_endpoint::Source,
    destination: &Path,
    cancel: CancelToken,
    events: tokio::sync::mpsc::UnboundedSender<LocalEvent>,
) -> std::result::Result<LocalSyncReport, xsync_core::local::LocalSyncError> {
    let sink = LocalSink::new_without_creation(parent(destination));
    let options = options(cancel);
    sync_endpoints(source, &sink, &options, move |event| {
        let _ = events.send(event);
    })
}

struct PreparedDownload {
    source: HttpSource,
    resolved: PathBuf,
    origin: String,
    connections: usize,
}

fn prepare_download(
    urls: &[String],
    destination: &Path,
    requested_connections: usize,
    retry: Option<super::HttpRetryPolicy>,
    cancel: CancelToken,
) -> Result<PreparedDownload> {
    let config = ClientConfig::new().timeout(std::time::Duration::from_secs(30));
    let client: Arc<dyn xsync_download::HttpClient> = Arc::new(
        UreqClient::with_config(config).map_err(|error| EngineError::Io(error.to_string()))?,
    );
    let retry = retry.map(|policy| RetryPolicy {
        max_tries: policy.max_attempts,
        initial_wait: std::time::Duration::from_secs(policy.initial_wait_seconds),
        max_wait: std::time::Duration::from_secs(policy.max_wait_seconds),
    });
    let (source, selected_url) = if urls.len() == 1 {
        let mut source = HttpSource::new(&urls[0], None, client)
            .map_err(|error| endpoint_to_engine(&error))?
            .with_segments(requested_connections)
            .with_cancel(cancel);
        if let Some(retry) = retry {
            source = source.with_retry(retry);
        }
        (source, &urls[0])
    } else {
        let mut mirrors = MirrorSource::new(urls.iter().cloned(), None, Arc::clone(&client))
            .map_err(|error| endpoint_to_engine(&error))?;
        if let Some(retry) = retry {
            mirrors = mirrors.with_retry(retry);
        }
        mirrors
            .enumerate()
            .map_err(|error| endpoint_to_engine(&error))?;
        let selected_url = &urls[mirrors.selected_mirror().index];
        let name = mirrors
            .sole_destination_name()
            .map_or_else(|| "download".to_owned(), |candidate| candidate.to_string());
        let mut source = HttpSource::new(selected_url, Some(&name), client)
            .map_err(|error| endpoint_to_engine(&error))?
            .with_segments(requested_connections)
            .with_cancel(cancel);
        if let Some(retry) = retry {
            source = source.with_retry(retry);
        }
        (source, selected_url)
    };
    let name = source.output_name().to_string();
    let connections = source.read_concurrency().min(requested_connections).max(1);
    Ok(PreparedDownload {
        source,
        resolved: destination.join(name),
        origin: xsync_download::origin_of(selected_url),
        connections,
    })
}

fn stopped() -> Settled {
    Settled {
        bytes: 0,
        files: 0,
        verified: false,
        note: None,
        stopped: true,
    }
}

/// Map a whole-session failure the same way [`endpoint_to_engine`] maps a
/// single one: an `Endpoint` failure keeps its kind's permanence, and anything
/// else is ordinary transport.
fn session_to_engine(error: xsync_core::local::LocalSyncError) -> EngineError {
    match error {
        xsync_core::local::LocalSyncError::Endpoint(endpoint) => endpoint_to_engine(&endpoint),
        other => EngineError::Io(other.to_string()),
    }
}

/// Run one HTTP download through xsync's local endpoint planner.
///
/// # Errors
///
/// Returns a permanent error for a URL that cannot be downloaded or an origin
/// refusal, and a transient one for a transport failure the transfer could be
/// retried past.
pub async fn run(
    url: &str,
    destination: &Path,
    stop: Arc<AtomicBool>,
    on_resolved: impl FnOnce(PathBuf) + Send + 'static,
    progress: impl FnMut(Moved) + Send + 'static,
) -> Result<Settled> {
    run_with_options(
        url,
        destination,
        HttpDownloadOptions::default(),
        stop,
        on_resolved,
        progress,
    )
    .await
}

/// Run an HTTP transfer with persisted E3 controls.
///
/// # Errors
///
/// Returns an error for invalid controls, an unavailable source, or a transfer
/// that could not be verified and published.
pub async fn run_with_options(
    url: &str,
    destination: &Path,
    options: HttpDownloadOptions,
    stop: Arc<AtomicBool>,
    on_resolved: impl FnOnce(PathBuf) + Send + 'static,
    mut progress: impl FnMut(Moved) + Send + 'static,
) -> Result<Settled> {
    super::validate_http_url(url)?;
    let primary = super::Endpoint::Http {
        url: url.to_owned(),
    };
    super::validate_http_options(Some(&primary), &options)?;
    let mut urls = vec![url.to_owned()];
    urls.extend(options.mirrors.clone());
    let preferred = options.preferred_mirror.unwrap_or(0);
    let preferred_url = urls.remove(preferred);
    urls.insert(0, preferred_url);
    let Some(probe_permits) = probe_permits(&urls, &stop).await? else {
        return Ok(stopped());
    };

    let cancel = CancelToken::new();
    let destination = destination.to_path_buf();
    let requested_connections = effective_connections(options.connections);
    let source_cancel = cancel.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_download(
            &urls,
            &destination,
            requested_connections,
            options.retry,
            source_cancel,
        )
    })
    .await
    .map_err(|error| EngineError::Io(format!("the HTTP probe thread failed: {error}")))??;
    drop(probe_permits);
    let PreparedDownload {
        source,
        resolved,
        origin,
        connections,
    } = prepared;
    on_resolved(resolved.clone());
    let Some(permit) = origin_permits(&origin, connections, &stop).await? else {
        return Ok(stopped());
    };
    let watcher_cancel = cancel.clone();
    let watcher_stop = Arc::clone(&stop);
    let watcher = tokio::spawn(async move {
        while !watcher_stop.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(CANCEL_POLL_MS)).await;
        }
        watcher_cancel.cancel();
    });
    let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
    let session =
        tokio::task::spawn_blocking(move || run_blocking(&source, &resolved, cancel, events));
    let mut tally = Tally::new();
    while let Some(event) = incoming.recv().await {
        if tally.absorb(&event) {
            progress(tally.moved());
        }
    }
    watcher.abort();
    let report = session
        .await
        .map_err(|error| EngineError::Io(format!("the HTTP transfer thread failed: {error}")))?
        .map_err(session_to_engine)?;
    drop(permit);
    if stop.load(Ordering::SeqCst) || report.cancelled {
        return Ok(Settled {
            bytes: tally.moved().bytes,
            files: tally.moved().files,
            verified: false,
            note: None,
            stopped: true,
        });
    }
    if report.partial_failure() || tally.failed() > 0 {
        return Err(EngineError::Io(format!(
            "the HTTP download could not be published: {}",
            tally.first_failure()
        )));
    }
    Ok(tally.settle(&report))
}

/// URL source and local folder extraction used by `Worker::attempt`.
///
/// # Errors
///
/// Returns [`EngineError::Unsupported`] when the row does not pair an HTTP
/// source with a local destination.
pub fn endpoints(transfer: &Transfer) -> Result<(&str, &Path)> {
    match (&transfer.from, &transfer.to) {
        (Some(Endpoint::Http { url }), Some(Endpoint::Local { path })) => {
            Ok((url, Path::new(path)))
        }
        _ => Err(EngineError::Unsupported(
            "HTTP downloads require an HTTP source and local destination".to_owned(),
        )),
    }
}
