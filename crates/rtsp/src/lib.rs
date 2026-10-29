//! Кодек RTSP/1.0 и HTTP/1.1 в том виде, в каком их использует AirPlay:
//! стартовая строка, заголовки, тело по `Content-Length`. Без ввода-вывода:
//! на вход байты, на выход сообщения, поэтому один код работает и в
//! приёмнике, и в отправителе.

use std::fmt::Write as _;

/// Предел на заголовки одного сообщения.
pub const MAX_HEAD: usize = 16 * 1024;
/// Предел на тело одного сообщения.
pub const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("заголовки длиннее {MAX_HEAD} байт")]
    HeadTooLong,
    #[error("тело длиннее {MAX_BODY} байт")]
    BodyTooLong,
    #[error("неверная стартовая строка")]
    BadStartLine,
    #[error("неверный заголовок")]
    BadHeader,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.0.retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
        self.0.push((name, value.into()));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    fn content_length(&self) -> Result<usize, Error> {
        match self.get("Content-Length") {
            None => Ok(0),
            Some(v) => v.trim().parse().map_err(|_| Error::BadHeader),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub uri: String,
    /// `RTSP/1.0` или `HTTP/1.1`; ответ повторяет версию запроса.
    pub version: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, uri: &str, version: &str) -> Self {
        Self {
            method: method.into(),
            uri: uri.into(),
            version: version.into(),
            headers: Headers::default(),
            body: Vec::new(),
        }
    }

    /// Путь без строки запроса (`/info?x` → `/info`).
    pub fn path(&self) -> &str {
        self.uri.split('?').next().unwrap_or("")
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut head = format!("{} {} {}\r\n", self.method, self.uri, self.version);
        write_headers(&mut head, &self.headers, self.body.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub version: String,
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Response {
    /// Ответ на запрос: та же версия протокола и тот же `CSeq`.
    pub fn to(req: &Request, status: u16) -> Self {
        let mut headers = Headers::default();
        if let Some(cseq) = req.headers.get("CSeq") {
            headers.insert("CSeq", cseq);
        }
        Self {
            version: req.version.clone(),
            status,
            reason: reason(status).into(),
            headers,
            body: Vec::new(),
        }
    }

    pub fn with_body(mut self, content_type: &str, body: Vec<u8>) -> Self {
        self.headers.insert("Content-Type", content_type);
        self.body = body;
        self
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut head = format!("{} {} {}\r\n", self.version, self.status, self.reason);
        write_headers(&mut head, &self.headers, self.body.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
    }
}

fn write_headers(head: &mut String, headers: &Headers, body_len: usize) {
    for (k, v) in headers.iter() {
        if !k.eq_ignore_ascii_case("Content-Length") {
            let _ = write!(head, "{k}: {v}\r\n");
        }
    }
    if body_len > 0 {
        let _ = write!(head, "Content-Length: {body_len}\r\n");
    }
    head.push_str("\r\n");
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        470 => "Connection Authorization Required",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

/// Разбор начала буфера: стартовая строка, заголовки и длина всего сообщения.
fn parse_head(buf: &[u8]) -> Result<Option<(Vec<String>, Headers, usize)>, Error> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > MAX_HEAD {
            Err(Error::HeadTooLong)
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEAD {
        return Err(Error::HeadTooLong);
    }
    let text = std::str::from_utf8(&buf[..end]).map_err(|_| Error::BadHeader)?;
    let mut lines = text.split("\r\n");
    let start: Vec<String> = lines
        .next()
        .ok_or(Error::BadStartLine)?
        .splitn(3, ' ')
        .map(str::to_owned)
        .collect();
    if start.len() < 2 {
        return Err(Error::BadStartLine);
    }
    let mut headers = Headers::default();
    for line in lines {
        let (k, v) = line.split_once(':').ok_or(Error::BadHeader)?;
        headers.0.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let body_len = headers.content_length()?;
    if body_len > MAX_BODY {
        return Err(Error::BodyTooLong);
    }
    Ok(Some((start, headers, end + 4 + body_len)))
}

/// Извлекает из начала буфера полный запрос, если он уже пришёл целиком.
pub fn decode_request(buf: &mut Vec<u8>) -> Result<Option<Request>, Error> {
    let Some((start, headers, total)) = parse_head(buf)? else {
        return Ok(None);
    };
    if start.len() != 3 {
        return Err(Error::BadStartLine);
    }
    if buf.len() < total {
        return Ok(None);
    }
    let head_len = total - headers.content_length()?;
    let body = buf[head_len..total].to_vec();
    buf.drain(..total);
    let [method, uri, version]: [String; 3] = start.try_into().unwrap();
    Ok(Some(Request {
        method,
        uri,
        version,
        headers,
        body,
    }))
}

/// Извлекает из начала буфера полный ответ.
pub fn decode_response(buf: &mut Vec<u8>) -> Result<Option<Response>, Error> {
    let Some((start, headers, total)) = parse_head(buf)? else {
        return Ok(None);
    };
    if buf.len() < total {
        return Ok(None);
    }
    let status = start[1].parse().map_err(|_| Error::BadStartLine)?;
    let head_len = total - headers.content_length()?;
    let body = buf[head_len..total].to_vec();
    buf.drain(..total);
    Ok(Some(Response {
        version: start[0].clone(),
        status,
        reason: start.get(2).cloned().unwrap_or_default(),
        headers,
        body,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_arrives_in_pieces() {
        let wire = b"POST /pair-setup RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 4\r\nX-Apple-HKP: 4\r\n\r\nabcdGET /info RTSP/1.0\r\nCSeq: 4\r\n\r\n";
        let mut buf = Vec::new();
        let mut got = Vec::new();
        for b in wire {
            buf.push(*b);
            while let Some(req) = decode_request(&mut buf).unwrap() {
                got.push(req);
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].path(), "/pair-setup");
        assert_eq!(got[0].body, b"abcd");
        assert_eq!(got[0].headers.get("x-apple-hkp"), Some("4"));
        assert_eq!(got[1].method, "GET");
        assert!(buf.is_empty());
    }

    #[test]
    fn response_echoes_cseq_and_version() {
        let mut buf = b"GET /info?x=1 RTSP/1.0\r\nCSeq: 7\r\n\r\n".to_vec();
        let req = decode_request(&mut buf).unwrap().unwrap();
        assert_eq!(req.path(), "/info");
        let resp = Response::to(&req, 200).with_body("text/plain", b"hi".to_vec());
        let mut out = Vec::new();
        resp.encode(&mut out);
        let mut back = out.clone();
        let parsed = decode_response(&mut back).unwrap().unwrap();
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.version, "RTSP/1.0");
        assert_eq!(parsed.headers.get("CSeq"), Some("7"));
        assert_eq!(parsed.body, b"hi");
    }

    #[test]
    fn limits_are_enforced() {
        let mut buf = vec![b'a'; MAX_HEAD + 1];
        assert_eq!(decode_request(&mut buf), Err(Error::HeadTooLong));
        let mut buf = format!(
            "POST / RTSP/1.0\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        )
        .into_bytes();
        assert_eq!(decode_request(&mut buf), Err(Error::BodyTooLong));
    }
}
