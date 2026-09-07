//! HTTP-to-local transfers (XDL-E1-S1).
//!
//! An HTTP download is deliberately a thin route into xsync's planner. The
//! source is `xsync_download::HttpSource`; the destination is xsync's
//! `LocalSink`, whose staged writes and local resume journal are the same ones
//! used by the existing local transfer route. A `ShareSink` would lose the
//! journal's ability to prove that its stage survived a restart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use xsync_core::local::{sync_endpoints, LocalEvent, LocalSyncOptions, LocalSyncReport};
use xsync_core::sink::LocalSink;
use xsync_download::{ClientConfig, HttpSource, UreqClient};
use xsync_endpoint::{CancelToken, EndpointError, EndpointErrorKind};

use super::xsync_engine::{Moved, Settled, Tally};
use super::{Endpoint, Transfer};
use crate::error::{EngineError, Result};

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
    source: &HttpSource,
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
    mut progress: impl FnMut(Moved) + Send + 'static,
) -> Result<Settled> {
    let cancel = CancelToken::new();
    let probe_url = url.to_owned();
    let probe_destination = destination.to_path_buf();
    let probe_cancel = cancel.clone();
    let (source, resolved) = tokio::task::spawn_blocking(move || {
        resolved_destination(&probe_url, &probe_destination, probe_cancel)
    })
    .await
    .map_err(|error| EngineError::Io(format!("the HTTP probe thread failed: {error}")))??;
    on_resolved(resolved.clone());
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
