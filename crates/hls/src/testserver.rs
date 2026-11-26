//! Minimal HTTP server for playback tests. Serves `tests/fixtures/av` on a
//! loopback port, can truncate one response to imitate a dropped connection,
//! can hold a response open to imitate a slow download, and counts requests
//! per path. Test builds only.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct Plan {
    fail: Option<String>,
    stall: Option<(String, Duration)>,
    requests: HashMap<String, usize>,
}

pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    plan: Arc<Mutex<Plan>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let addr = listener.local_addr().expect("test server address");
        let stop = Arc::new(AtomicBool::new(false));
        let plan = Arc::new(Mutex::new(Plan::default()));
        let thread = {
            let stop = stop.clone();
            let plan = plan.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else {
                        continue;
                    };
                    let plan = plan.clone();
                    thread::spawn(move || handle(stream, &plan));
                }
            })
        };
        Self {
            addr,
            stop,
            plan,
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
        self.plan.lock().expect("plan").fail = Some(path.to_owned());
    }

    /// The next request for `path` gets its headers and half the body, then the
    /// connection is held open for `delay` before it closes.
    pub fn stall_next(&self, path: &str, delay: Duration) {
        self.plan.lock().expect("plan").stall = Some((path.to_owned(), delay));
    }

    /// How many requests for `path` reached the server.
    pub fn requests(&self, path: &str) -> usize {
        *self
            .plan
            .lock()
            .expect("plan")
            .requests
            .get(path)
            .unwrap_or(&0)
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

fn handle(mut stream: TcpStream, plan: &Mutex<Plan>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

    let (truncated, stalled) = {
        let mut plan = plan.lock().expect("plan");
        *plan.requests.entry(path.clone()).or_default() += 1;
        let truncated = if plan.fail.as_deref() == Some(path.as_str()) {
            plan.fail = None;
            true
        } else {
            false
        };
        let stalled = match plan.stall.as_ref() {
            Some((stalled_path, delay)) if *stalled_path == path => {
                let delay = *delay;
                plan.stall = None;
                Some(delay)
            }
            _ => None,
        };
        (truncated, stalled)
    };

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
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if truncated || stalled.is_some() {
        let _ = stream.write_all(&bytes[..bytes.len() / 2]);
        if let Some(delay) = stalled {
            thread::sleep(delay);
        }
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
