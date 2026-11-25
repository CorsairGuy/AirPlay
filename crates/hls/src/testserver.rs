//! Minimal HTTP server for playback tests. Serves `tests/fixtures/av` on a
//! loopback port and can truncate one response to imitate a dropped connection.
//! Test builds only.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    fail: Arc<Mutex<Option<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let addr = listener.local_addr().expect("test server address");
        let stop = Arc::new(AtomicBool::new(false));
        let fail = Arc::new(Mutex::new(None));
        let thread = {
            let stop = stop.clone();
            let fail = fail.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else {
                        continue;
                    };
                    let fail = fail.clone();
                    thread::spawn(move || handle(stream, &fail));
                }
            })
        };
        Self {
            addr,
            stop,
            fail,
            thread: Some(thread),
        }
    }

    /// The tests use `localhost` because the production URL parser rejects
    /// private IP literals.
    pub fn url(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.addr.port())
    }

    /// The next request for `path` is answered with half of the body and a
    /// closed connection.
    pub fn fail_next(&self, path: &str) {
        *self.fail.lock().expect("fail flag") = Some(path.to_owned());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop so it observes the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle(mut stream: TcpStream, fail: &Mutex<Option<String>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

    let Some(name) = path.strip_prefix("/av/") else {
        respond(&mut stream, "404 Not Found", b"");
        return;
    };
    if name.contains("..") {
        respond(&mut stream, "400 Bad Request", b"");
        return;
    }
    let file = fixture_dir().join(name);
    let Ok(bytes) = std::fs::read(&file) else {
        respond(&mut stream, "404 Not Found", b"");
        return;
    };
    let content_type = if name.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp2t"
    };
    let truncated = {
        let mut fail = fail.lock().expect("fail flag");
        if fail.as_deref() == Some(path.as_str()) {
            *fail = None;
            true
        } else {
            false
        }
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if truncated {
        let _ = stream.write_all(&bytes[..bytes.len() / 2]);
        return;
    }
    let _ = stream.write_all(&bytes);
}

fn respond(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("av")
}
