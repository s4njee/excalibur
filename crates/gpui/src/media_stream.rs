//! A bounded localhost HTTP bridge for native media playback.
//!
//! Media elements need a seekable URL, while the engine intentionally exposes
//! byte ranges rather than a long-lived file descriptor.  This module adapts
//! the two: every request is authenticated by a per-instance token and reads
//! at most one bounded range from the active backend.

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use excalibur_engine::backend::ShareBackend;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

pub(crate) const DEFAULT_WINDOW: u64 = 1024 * 1024;
pub(crate) const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A temporary, seekable URL for one media file.
///
/// The server is shut down when the last clone is dropped.  The token is not
/// a security boundary, but it prevents unrelated local requests from
/// accidentally reading a mounted file.
#[derive(Clone)]
pub struct MediaStream {
    url: String,
    _state: Arc<MediaStreamState>,
}

impl MediaStream {
    pub(crate) fn new(url: String, shutdown: oneshot::Sender<()>) -> Self {
        Self {
            url,
            _state: Arc::new(MediaStreamState {
                shutdown: Mutex::new(Some(shutdown)),
            }),
        }
    }

    /// The localhost URL that can be assigned to a media element.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl fmt::Debug for MediaStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaStream")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl PartialEq for MediaStream {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
    }
}

impl Eq for MediaStream {}

struct MediaStreamState {
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl Drop for MediaStreamState {
    fn drop(&mut self) {
        if let Ok(mut sender) = self.shutdown.lock() {
            if let Some(sender) = sender.take() {
                let _ = sender.send(());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestMethod {
    Get,
    Head,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Request {
    method: RequestMethod,
    range: Option<ByteRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeError {
    Malformed,
    MethodNotAllowed,
    Unsatisfiable,
}

pub(crate) fn token() -> String {
    let counter = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
        });
    format!("{pid:x}-{nanos:x}-{counter:x}")
}

pub(crate) async fn serve(
    listener: TcpListener,
    backend: Arc<dyn ShareBackend>,
    path: String,
    size: u64,
    mime: String,
    token_path: String,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            Ok(()) = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break };
                let backend = Arc::clone(&backend);
                let path = path.clone();
                let mime = mime.clone();
                let token_path = token_path.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, backend, &path, size, &mime, &token_path).await;
                });
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    backend: Arc<dyn ShareBackend>,
    path: &str,
    size: u64,
    mime: &str,
    token_path: &str,
) -> std::io::Result<()> {
    let mut request_bytes = vec![0_u8; 16 * 1024];
    let used = read_request(&mut stream, &mut request_bytes).await?;
    let request = match parse_request(&request_bytes[..used], token_path, size) {
        Ok(request) => request,
        Err(RangeError::Unsatisfiable) => {
            write_headers(
                &mut stream,
                "416 Range Not Satisfiable",
                0,
                Some(&format!("bytes */{size}")),
                mime,
            )
            .await?;
            return Ok(());
        }
        Err(RangeError::Malformed) => {
            write_headers(&mut stream, "400 Bad Request", 0, None, mime).await?;
            return Ok(());
        }
        Err(RangeError::MethodNotAllowed) => {
            write_headers(&mut stream, "405 Method Not Allowed", 0, None, mime).await?;
            return Ok(());
        }
    };

    let (status, start, requested_len, partial) =
        response_plan(request.method, request.range, size);
    if request.method == RequestMethod::Head {
        let end = start.saturating_add(requested_len.saturating_sub(1));
        let content_range = partial.then(|| format!("bytes {start}-{end}/{size}"));
        write_headers(
            &mut stream,
            status,
            requested_len,
            content_range.as_deref(),
            mime,
        )
        .await?;
        return Ok(());
    }
    let requested_len = requested_len.min(MAX_RESPONSE);

    let body = match backend
        .read_range(
            path,
            start,
            u32::try_from(requested_len).expect("media response length is capped"),
        )
        .await
    {
        Ok(body) => body,
        Err(error) => {
            write_headers(&mut stream, "500 Internal Server Error", 0, None, mime).await?;
            let _ = error;
            return Ok(());
        }
    };
    let actual_len = u64::try_from(body.len()).unwrap_or(u64::MAX);
    let content_range = partial.then(|| {
        let end = start.saturating_add(actual_len.saturating_sub(1));
        format!("bytes {start}-{end}/{size}")
    });
    write_headers(
        &mut stream,
        status,
        actual_len,
        content_range.as_deref(),
        mime,
    )
    .await?;
    stream.write_all(&body).await
}

fn response_plan(
    method: RequestMethod,
    range: Option<ByteRange>,
    size: u64,
) -> (&'static str, u64, u64, bool) {
    match (method, range) {
        (RequestMethod::Head, None) => ("200 OK", 0, size, false),
        (RequestMethod::Head | RequestMethod::Get, Some(range)) => (
            "206 Partial Content",
            range.start,
            range.end - range.start + 1,
            true,
        ),
        (RequestMethod::Get, None) if size == 0 => ("200 OK", 0, 0, false),
        (RequestMethod::Get, None) => ("206 Partial Content", 0, size.min(DEFAULT_WINDOW), true),
    }
}

async fn read_request(stream: &mut TcpStream, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut used = 0;
    while used < buffer.len() {
        let read = stream.read(&mut buffer[used..]).await?;
        if read == 0 {
            break;
        }
        used += read;
        if buffer[..used]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            return Ok(used);
        }
    }
    Ok(used)
}

async fn write_headers(
    stream: &mut TcpStream,
    status: &str,
    length: u64,
    content_range: Option<&str>,
    mime: &str,
) -> std::io::Result<()> {
    let mut headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nContent-Type: {mime}\r\nConnection: close\r\n"
    );
    if let Some(content_range) = content_range {
        write!(headers, "Content-Range: {content_range}\r\n")
            .expect("writing to a String cannot fail");
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).await
}

fn parse_request(
    bytes: &[u8],
    token_path: &str,
    size: u64,
) -> std::result::Result<Request, RangeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| RangeError::Malformed)?;
    let header_end = text.find("\r\n\r\n").ok_or(RangeError::Malformed)?;
    let mut lines = text[..header_end].split("\r\n");
    let request_line = lines.next().ok_or(RangeError::Malformed)?;
    let mut parts = request_line.split_ascii_whitespace();
    let method = match parts.next() {
        Some("GET") => RequestMethod::Get,
        Some("HEAD") => RequestMethod::Head,
        Some(_) => return Err(RangeError::MethodNotAllowed),
        _ => return Err(RangeError::Malformed),
    };
    let target = parts.next().ok_or(RangeError::Malformed)?;
    if target != token_path || parts.next() != Some("HTTP/1.1") {
        return Err(RangeError::Malformed);
    }
    let mut range_header = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Range") {
                range_header = Some(value.trim());
            }
        }
    }
    let range = range_header
        .map(|header| parse_range(header, size))
        .transpose()?;
    Ok(Request { method, range })
}

fn parse_range(header: &str, size: u64) -> std::result::Result<ByteRange, RangeError> {
    let value = header.strip_prefix("bytes=").ok_or(RangeError::Malformed)?;
    if value.contains(',') {
        return Err(RangeError::Malformed);
    }
    let (start, end) = value.split_once('-').ok_or(RangeError::Malformed)?;
    if size == 0 {
        return Err(RangeError::Unsatisfiable);
    }
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| RangeError::Malformed)?;
        if suffix == 0 {
            return Err(RangeError::Unsatisfiable);
        }
        let length = suffix.min(size);
        return Ok(ByteRange {
            start: size - length,
            end: size - 1,
        });
    }
    let start = start.parse::<u64>().map_err(|_| RangeError::Malformed)?;
    if start >= size {
        return Err(RangeError::Unsatisfiable);
    }
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>()
            .map_err(|_| RangeError::Malformed)?
            .min(size - 1)
    };
    if start > end {
        return Err(RangeError::Unsatisfiable);
    }
    Ok(ByteRange { start, end })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_range, response_plan, ByteRange, RangeError, RequestMethod, DEFAULT_WINDOW,
        MAX_RESPONSE,
    };

    #[test]
    fn parses_standard_ranges() {
        assert_eq!(
            parse_range("bytes=10-20", 100),
            Ok(ByteRange { start: 10, end: 20 })
        );
        assert_eq!(
            parse_range("bytes=10-", 100),
            Ok(ByteRange { start: 10, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=-10", 100),
            Ok(ByteRange { start: 90, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=-1000", 100),
            Ok(ByteRange { start: 0, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=100-", 100),
            Err(RangeError::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=2-1", 100),
            Err(RangeError::Unsatisfiable)
        );
        assert_eq!(
            parse_range("bytes=1-2,4-5", 100),
            Err(RangeError::Malformed)
        );
    }

    #[test]
    fn bounded_window_constants_are_safe() {
        assert_eq!(DEFAULT_WINDOW, 1024 * 1024);
        assert_eq!(MAX_RESPONSE, 8 * 1024 * 1024);
    }

    #[test]
    fn response_plan_keeps_media_seekable_without_a_range_header() {
        assert_eq!(
            response_plan(RequestMethod::Get, None, 2 * DEFAULT_WINDOW),
            ("206 Partial Content", 0, DEFAULT_WINDOW, true)
        );
        assert_eq!(
            response_plan(RequestMethod::Head, None, 2 * DEFAULT_WINDOW),
            ("200 OK", 0, 2 * DEFAULT_WINDOW, false)
        );
    }

    #[test]
    fn request_parser_requires_the_exact_token_and_accepts_case_insensitive_range_name() {
        let request = b"GET /secret HTTP/1.1\r\nrange: bytes=-2\r\n\r\n";
        let parsed = super::parse_request(request, "/secret", 10).expect("request");
        assert_eq!(parsed.range, Some(ByteRange { start: 8, end: 9 }));
        assert!(super::parse_request(request, "/other", 10).is_err());
    }
}
