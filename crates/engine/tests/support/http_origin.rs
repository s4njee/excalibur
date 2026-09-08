//! A loopback origin shared by the HTTP route tests and throughput harness.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct Origin {
    pub url: String,
    pub sent: Arc<AtomicUsize>,
    // This support module is compiled separately by the throughput harness,
    // which does not inspect concurrency.
    #[allow(dead_code)]
    pub max_active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}

impl Origin {
    pub fn start(bytes: Vec<u8>, disposition: Option<&str>, ranges: bool, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/object.bin", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let done = Arc::clone(&stop);
        let written = Arc::clone(&sent);
        let active_requests = Arc::clone(&active);
        let peak_requests = Arc::clone(&max_active);
        let bytes = Arc::new(bytes);
        let disposition = disposition.map(str::to_owned);
        let task = thread::spawn(move || {
            let mut clients = Vec::new();
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // The listener is non-blocking, so accepted sockets
                        // inherit that; `respond` expects a blocking socket
                        // with timeouts, otherwise its first read fails with
                        // `WouldBlock` and the client sees a disconnect.
                        stream.set_nonblocking(false).unwrap();
                        let bytes = Arc::clone(&bytes);
                        let stop = Arc::clone(&done);
                        let sent = Arc::clone(&written);
                        let active = Arc::clone(&active_requests);
                        let max_active = Arc::clone(&peak_requests);
                        let disposition = disposition.clone();
                        clients.push(thread::spawn(move || {
                            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(now, Ordering::SeqCst);
                            let _ = respond(
                                stream,
                                &bytes,
                                disposition.as_deref(),
                                ranges,
                                delay,
                                &stop,
                                &sent,
                            );
                            active.fetch_sub(1, Ordering::SeqCst);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
            for client in clients {
                client.join().unwrap();
            }
        });
        Self {
            url,
            sent,
            max_active,
            stop,
            task: Some(task),
        }
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.task.take().unwrap().join().unwrap();
    }
}

fn respond(
    mut stream: TcpStream,
    bytes: &[u8],
    disposition: Option<&str>,
    ranges: bool,
    delay: Duration,
    stop: &AtomicBool,
    sent: &AtomicUsize,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let count = stream.read(&mut chunk)?;
        if count == 0 || request.len() > 16 * 1024 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..count]);
    }
    let request = String::from_utf8_lossy(&request);
    if request.starts_with("HEAD /missing") || request.starts_with("GET /missing") {
        return stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
    let range = ranges
        .then(|| {
            request.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                if !key.eq_ignore_ascii_case("range") {
                    return None;
                }
                let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
                let start = start.parse::<usize>().ok()?;
                let end = end.parse::<usize>().unwrap_or(bytes.len() - 1);
                Some((start, end))
            })
        })
        .flatten();
    let (start, end) = range.unwrap_or((0, bytes.len() - 1));
    if start > end || end >= bytes.len() {
        return stream.write_all(
            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }
    let status = if range.is_some() {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: \"fixture-v1\"\r\nConnection: close\r\n",
        end - start + 1
    )?;
    if ranges {
        write!(stream, "Accept-Ranges: bytes\r\n")?;
    }
    if range.is_some() {
        write!(
            stream,
            "Content-Range: bytes {start}-{end}/{}\r\n",
            bytes.len()
        )?;
    }
    if let Some(name) = disposition {
        write!(stream, "Content-Disposition: {name}\r\n")?;
    }
    write!(stream, "\r\n")?;
    if request.starts_with("HEAD ") {
        return Ok(());
    }
    for part in bytes[start..=end].chunks(16 * 1024) {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        stream.write_all(part)?;
        sent.fetch_add(part.len(), Ordering::SeqCst);
        if !delay.is_zero() {
            thread::sleep(delay);
        }
    }
    Ok(())
}
