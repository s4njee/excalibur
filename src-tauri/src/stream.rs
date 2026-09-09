//! The `excalibur-stream://` URI scheme: previews and media, by byte range.

use std::sync::Arc;

use excalibur_engine::backend::ShareBackend;
use tauri::http;
use tauri::Manager;

use crate::state::AppState;

/// The scheme previews and media are served over.
///
/// A custom protocol rather than a command, because **file bytes must never
/// cross IPC**: a command's return value is serialised through the webview
/// bridge, which would mean holding a 40 GB film in memory to show a frame of
/// it. This streams a byte range straight from the backend to the `<video>`
/// element that asked for it.
pub(crate) const STREAM_SCHEME: &str = "excalibur-stream";

/// Bytes served per request when the webview does not ask for a range.
///
/// A first request with no `Range` is what a `<video>` sends to discover the
/// file; answering the whole thing would defeat the point. One megabyte is
/// enough for a container header and small enough to be instant.
const STREAM_DEFAULT_WINDOW: u64 = 1024 * 1024;

/// Largest range answered in one response, whatever was asked for.
const STREAM_MAX_WINDOW: u64 = 8 * 1024 * 1024;

/// Which window a stream URL names, if it names one.
///
/// `excalibur-stream://<label>/<path>`: the label is the authority, because the
/// protocol handler is given a request and not a window — there is no caller to
/// inject one. A URL with no label is answered from the app's mount rather than
/// refused, which covers the platforms that rewrite a custom scheme to
/// `http://excalibur-stream.localhost/…` and lose the authority on the way.
fn window_from_host(host: Option<&str>) -> Option<&str> {
    let host = host?;
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return None;
    }
    Some(host)
}

/// Percent-decode a URI path segment.
///
/// Hand-rolled rather than a dependency: this decodes one path, the rules are
/// three lines, and the alternative is a crate in the audit for it.
pub(crate) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a `Range: bytes=…` header asked for, resolved against a known size.
///
/// Returns `None` for a syntactically valid header naming a range past the end,
/// which is the case that must answer `416` rather than be clamped into
/// something the caller did not ask for.
fn parse_range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    let Some(header) = header else {
        // No header: serve from the start, a window at a time.
        let end = size.min(STREAM_DEFAULT_WINDOW).saturating_sub(1);
        return Some((0, end));
    };
    let spec = header.strip_prefix("bytes=")?.trim();
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    if start >= size {
        return None;
    }
    let end = match end.trim() {
        "" => size - 1,
        value => value.parse::<u64>().ok()?.min(size - 1),
    };
    if end < start {
        return None;
    }
    // Capped so one request cannot ask for the whole file: a player that sends
    // `bytes=0-` is asking for everything, and answering that literally is how
    // a preview becomes a download. The player asks again for the rest.
    Some((start, end.min(start + STREAM_MAX_WINDOW - 1)))
}

/// Answer one range request from the active mount.
async fn stream_range(
    backend: Arc<dyn ShareBackend>,
    path: &str,
    range: Option<String>,
) -> http::Response<Vec<u8>> {
    let refuse = |status: http::StatusCode| {
        http::Response::builder()
            .status(status)
            .body(Vec::new())
            .unwrap_or_default()
    };

    let Ok(attrs) = backend.stat(path).await else {
        return refuse(http::StatusCode::NOT_FOUND);
    };
    let size = attrs.size;
    let Some((start, end)) = parse_range(range.as_deref(), size) else {
        return http::Response::builder()
            .status(http::StatusCode::RANGE_NOT_SATISFIABLE)
            .header(http::header::CONTENT_RANGE, format!("bytes */{size}"))
            .body(Vec::new())
            .unwrap_or_default();
    };
    let length = u32::try_from(end - start + 1).unwrap_or(u32::MAX);
    // Deliberately still `read_range` and not the streaming handle E13-S7 gave
    // transfers: a seek is a range, not a stream. Each request here is
    // unrelated to the last — a player jumps, an image loads once — so a handle
    // kept open between them would hold a file the user has already navigated
    // away from, to save an open nobody is waiting on.
    let Ok(data) = backend.read_range(path, start, length).await else {
        return refuse(http::StatusCode::INTERNAL_SERVER_ERROR);
    };
    // A short read is the end of the file, so the header describes what is
    // actually in the body rather than what was asked for.
    let end = start + data.len() as u64 - 1;

    http::Response::builder()
        .status(http::StatusCode::PARTIAL_CONTENT)
        .header(http::header::CONTENT_TYPE, mime_for(path))
        .header(http::header::ACCEPT_RANGES, "bytes")
        .header(
            http::header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}"),
        )
        .header(http::header::CONTENT_LENGTH, data.len().to_string())
        // The webview treats a custom scheme as a foreign origin.
        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(data)
        .unwrap_or_default()
}

/// A media type for the `<video>`/`<img>` element, from the name alone.
///
/// The engine's kind table already decided what this file is; this maps its
/// category onto the handful of types a browser needs told. Sniffing would mean
/// reading the file to answer a question the name answers well enough.
fn mime_for(path: &str) -> &'static str {
    use excalibur_engine::kind::Category;
    let name = path.rsplit('/').next().unwrap_or(path);
    let kind = excalibur_engine::kind::classify(name);
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "aac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        _ => match kind.category {
            Category::Text | Category::Code => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        },
    }
}

/// Answer one `excalibur-stream://` request.
///
/// A free function rather than a closure in the builder, so the handler's
/// rules sit beside [`parse_range`] and [`stream_range`], which are what
/// enforce them.
// Both arguments are owned because Tauri's handler bound says so; taking
// either by reference would no longer fit `register_asynchronous_uri_scheme_protocol`.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn serve(
    context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let sessions = Arc::clone(&context.app_handle().state::<AppState>().sessions);
    // Which window asked. A protocol handler gets a request, not a
    // caller, so the label cannot be injected the way a command's is —
    // it travels in the URL instead, and the webview puts it there
    // because it knows its own label (E23-S1). Without this, two
    // windows on two shares would both preview the app's most recent
    // mount, and the second window would show the first one's files.
    let label = window_from_host(request.uri().host()).map(str::to_owned);
    // The path is percent-encoded by the webview; a name with a space
    // or a `#` in it is common enough that decoding is not optional.
    let path = percent_decode(request.uri().path().trim_start_matches('/'));
    let range = request
        .headers()
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    tauri::async_runtime::spawn(async move {
        // Nothing mounted is not something to answer with a stand-in:
        // a preview of a file that does not exist is worse than a
        // broken image (E1-S6).
        let mount = match label.as_deref() {
            Some(window) => sessions.active_backend(window).await,
            None => sessions.recent_backend().await,
        };
        let response = match mount {
            Ok(backend) => stream_range(backend, &path, range).await,
            Err(_) => http::Response::builder()
                .status(http::StatusCode::SERVICE_UNAVAILABLE)
                .body(Vec::new())
                .unwrap_or_default(),
        };
        responder.respond(response);
    });
}

#[cfg(test)]
mod tests {
    use super::{
        parse_range, percent_decode, window_from_host, STREAM_DEFAULT_WINDOW, STREAM_MAX_WINDOW,
    };

    #[test]
    fn no_range_header_serves_a_window_from_the_start() {
        // A `<video>`'s first request has no Range: it is discovering the file.
        // Answering the whole thing would defeat the point of streaming.
        assert_eq!(
            parse_range(None, 100 * 1024 * 1024),
            Some((0, STREAM_DEFAULT_WINDOW - 1))
        );
    }

    #[test]
    fn a_small_file_is_served_whole() {
        assert_eq!(parse_range(None, 10), Some((0, 9)));
    }

    #[test]
    fn an_open_ended_range_is_capped_rather_than_answered_literally() {
        // `bytes=0-` asks for everything. Answering it is how a preview
        // becomes a download; the player asks again for the rest.
        assert_eq!(
            parse_range(Some("bytes=0-"), 40 * 1024 * 1024 * 1024),
            Some((0, STREAM_MAX_WINDOW - 1))
        );
    }

    #[test]
    fn a_closed_range_is_honoured_within_the_cap() {
        assert_eq!(parse_range(Some("bytes=100-199"), 1000), Some((100, 199)));
    }

    #[test]
    fn a_range_is_clamped_to_the_end_of_the_file() {
        assert_eq!(parse_range(Some("bytes=900-5000"), 1000), Some((900, 999)));
    }

    #[test]
    fn a_range_past_the_end_is_refused_rather_than_clamped() {
        // This must answer 416. Clamping it would serve bytes the caller did
        // not ask for, and a player seeking past the end would play them.
        assert_eq!(parse_range(Some("bytes=2000-"), 1000), None);
    }

    #[test]
    fn a_backwards_or_malformed_range_is_refused() {
        for header in ["bytes=200-100", "bytes=", "bytes=abc-", "items=0-1", "0-1"] {
            assert_eq!(parse_range(Some(header), 1000), None, "{header}");
        }
    }

    #[test]
    fn a_stream_url_names_the_window_that_asked() {
        // The authority is the window's label (E23-S1), because a protocol
        // handler is handed a request and not a caller.
        assert_eq!(window_from_host(Some("browser-2")), Some("browser-2"));
        assert_eq!(window_from_host(Some("main")), Some("main"));
    }

    #[test]
    fn a_stream_url_with_no_window_falls_back_rather_than_failing() {
        // The three shapes that carry no label: a URL written before this
        // story, one with no authority at all, and the `http://<scheme>
        // .localhost/…` rewrite that Windows and Android apply to a custom
        // scheme — which loses the authority on the way through.
        assert_eq!(window_from_host(Some("localhost")), None);
        assert_eq!(window_from_host(Some("")), None);
        assert_eq!(window_from_host(None), None);
        assert_eq!(window_from_host(Some("excalibur-stream.localhost")), None);
    }

    #[test]
    fn a_percent_encoded_path_decodes() {
        // A space or a `#` in a filename is common, and the webview encodes it.
        assert_eq!(
            percent_decode("Blade%20Runner%232.mkv"),
            "Blade Runner#2.mkv"
        );
        assert_eq!(percent_decode("plain.txt"), "plain.txt");
        // A stray `%` is left alone rather than swallowing what follows it.
        assert_eq!(percent_decode("100%.txt"), "100%.txt");
    }
}
