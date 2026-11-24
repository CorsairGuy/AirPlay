use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crypto::{Channel, kdf::ControlKeys};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

// SETUP and pairing are small. Keep peer-controlled buffering much smaller than
// the generic RTSP codec's limit, including when the header never terminates.
const MAX_BUFFER: usize = 1024 * 1024;

pub(crate) struct Connection {
    stream: TcpStream,
    channel: Option<Channel>,
    raw: Vec<u8>,
    plain: Vec<u8>,
}

impl Connection {
    pub(crate) fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            channel: None,
            raw: Vec::new(),
            plain: Vec::new(),
        }
    }

    pub(crate) fn encrypt(&mut self, keys: ControlKeys) {
        self.channel = Some(Channel::new(keys));
    }

    async fn fill(&mut self) -> Result<()> {
        let mut buf = [0u8; 4096];
        let n = self.stream.read(&mut buf).await?;
        if n == 0 {
            bail!("receiver connection closed");
        }
        if let Some(channel) = self.channel.as_mut() {
            self.raw.extend_from_slice(&buf[..n]);
            channel.open(&mut self.raw, &mut self.plain)?;
        } else {
            self.plain.extend_from_slice(&buf[..n]);
        }
        if self.raw.len() > MAX_BUFFER || self.plain.len() > MAX_BUFFER {
            bail!("receiver message exceeds sender buffer limit");
        }
        Ok(())
    }

    pub(crate) async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(channel) = self.channel.as_mut() {
            let mut sealed = Vec::new();
            channel.seal(bytes, &mut sealed);
            self.stream.write_all(&sealed).await?;
        } else {
            self.stream.write_all(bytes).await?;
        }
        Ok(())
    }

    async fn response(&mut self) -> Result<rtsp::Response> {
        loop {
            if let Some(response) = rtsp::decode_response(&mut self.plain)? {
                return Ok(response);
            }
            self.fill().await?;
        }
    }

    pub(crate) async fn request(
        &mut self,
        req: &rtsp::Request,
        deadline: Duration,
    ) -> Result<rtsp::Response> {
        // One deadline covers both write and all partial reads; a peer cannot
        // extend the timeout indefinitely by dripping one byte at a time.
        timeout(deadline, async {
            let mut bytes = Vec::new();
            req.encode(&mut bytes);
            self.send(&bytes).await?;
            let response = self.response().await?;
            if response.status != 200 {
                bail!("receiver returned status {}", response.status);
            }
            if let Some(expected) = req.headers.get("CSeq") {
                if response.headers.get("CSeq") != Some(expected) {
                    bail!("receiver response CSeq does not match request");
                }
            }
            Ok(response)
        })
        .await
        .context("receiver request deadline exceeded")?
    }

    pub(crate) async fn event_request(&mut self) -> Result<rtsp::Request> {
        loop {
            if let Some(request) = rtsp::decode_request(&mut self.plain)? {
                return Ok(request);
            }
            self.fill().await?;
        }
    }
}

pub(crate) struct Rtsp {
    pub(crate) conn: Connection,
    cseq: u32,
    uri: String,
    dacp: String,
    active_remote: u32,
    deadline: Duration,
}

impl Rtsp {
    pub(crate) fn new(
        conn: Connection,
        uri: String,
        dacp: String,
        active_remote: u32,
        deadline: Duration,
    ) -> Self {
        Self {
            conn,
            cseq: 0,
            uri,
            dacp,
            active_remote,
            deadline,
        }
    }

    pub(crate) async fn pair_post(&mut self, path: &str, body: Vec<u8>) -> Result<rtsp::Response> {
        self.pair_request(path, body, "4").await
    }

    pub(crate) async fn verify_post(&mut self, body: Vec<u8>) -> Result<rtsp::Response> {
        self.pair_request("/pair-verify", body, "3").await
    }

    pub(crate) async fn pin_post(&mut self, path: &str, body: Vec<u8>) -> Result<rtsp::Response> {
        self.pair_request(path, body, "3").await
    }

    async fn pair_request(
        &mut self,
        path: &str,
        body: Vec<u8>,
        hkp: &str,
    ) -> Result<rtsp::Response> {
        let mut req = rtsp::Request::new("POST", path, "HTTP/1.1");
        req.headers.insert("User-Agent", "AirPlay/320.20");
        req.headers.insert("Connection", "keep-alive");
        req.headers.insert("X-Apple-HKP", hkp);
        req.headers
            .insert("Content-Type", "application/octet-stream");
        req.body = body;
        self.conn.request(&req, self.deadline).await
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        path: Option<&str>,
        body: Vec<u8>,
        content_type: Option<&str>,
        extra: &[(&str, String)],
    ) -> Result<rtsp::Response> {
        let mut req = rtsp::Request::new(method, path.unwrap_or(&self.uri), "RTSP/1.0");
        let seq = self.cseq;
        self.cseq = self
            .cseq
            .checked_add(1)
            .ok_or_else(|| anyhow!("RTSP CSeq exhausted"))?;
        req.headers.insert("CSeq", seq.to_string());
        req.headers.insert("User-Agent", "AirPlay/550.10");
        req.headers.insert("DACP-ID", &self.dacp);
        req.headers.insert("Client-Instance", &self.dacp);
        req.headers
            .insert("Active-Remote", self.active_remote.to_string());
        for (name, value) in extra {
            req.headers.insert(*name, value);
        }
        if let Some(content_type) = content_type {
            req.headers.insert("Content-Type", content_type);
        }
        req.body = body;
        self.conn.request(&req, self.deadline).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_uses_the_shared_rtsp_codec() {
        let mut req = rtsp::Request::new("POST", "/pair-pin-start", "HTTP/1.1");
        let mut bytes = Vec::new();
        req.encode(&mut bytes);
        // The shared encoder adds an explicit length even for an empty body.
        req.headers.insert("Content-Length", "0");
        assert_eq!(rtsp::decode_request(&mut bytes).unwrap(), Some(req));
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn connection_handles_fragmented_loopback_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await.unwrap();
            for bytes in b"RTSP/1.0 200 OK\r\nCSeq: 7\r\nContent-Length: 3\r\n\r\nabc".chunks(3) {
                stream.write_all(bytes).await.unwrap();
            }
        });
        let mut conn = Connection::new(TcpStream::connect(endpoint).await.unwrap());
        let mut req = rtsp::Request::new("RECORD", "rtsp://local/1", "RTSP/1.0");
        req.headers.insert("CSeq", "7");
        let response = conn.request(&req, Duration::from_secs(1)).await.unwrap();
        assert_eq!(response.body, b"abc");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_rejects_a_response_for_another_cseq() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 1024];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            stream
                .write_all(b"RTSP/1.0 200 OK\r\nCSeq: 8\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let mut conn = Connection::new(TcpStream::connect(endpoint).await.unwrap());
        let mut req = rtsp::Request::new("RECORD", "rtsp://local/1", "RTSP/1.0");
        req.headers.insert("CSeq", "7");
        let error = conn
            .request(&req, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("CSeq"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_deadline_is_not_reset_by_partial_progress() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 1024];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            for byte in b"RTSP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n" {
                if stream.write_all(&[*byte]).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let mut conn = Connection::new(TcpStream::connect(endpoint).await.unwrap());
        let req = rtsp::Request::new("POST", "/pair-pin-start", "HTTP/1.1");
        let before = std::time::Instant::now();
        let error = conn
            .request(&req, Duration::from_millis(80))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(before.elapsed() < Duration::from_secs(1));
        server.abort();
        let _ = server.await;
    }
}
