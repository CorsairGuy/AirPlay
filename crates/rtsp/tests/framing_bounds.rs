//! Границы последовательности RTSP и межкадровых данных (C42).
//!
//! Проверяется, что разбор не сдвигает границу следующего запроса, что
//! `Content-Length` и `CSeq` не влияют друг на друга и что ошибка не съедает
//! чужие байты.

use rtsp::{Error, MAX_BODY, MAX_HEAD, decode_request, decode_response};

#[test]
fn body_boundary_is_exact_for_two_pipelined_requests() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"POST /pair-setup RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 3\r\n\r\nabc");
    buf.extend_from_slice(b"SETUP /stream RTSP/1.0\r\nCSeq: 2\r\nContent-Length: 0\r\n\r\n");
    buf.extend_from_slice(b"TEARDOWN /stream RTSP/1.0\r\nCSeq: 3\r\n\r\n");

    let first = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!((first.method.as_str(), first.body.as_slice()), ("POST", &b"abc"[..]));
    let second = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(second.method, "SETUP");
    assert!(second.body.is_empty());
    let third = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(third.method, "TEARDOWN");
    assert_eq!(third.headers.get("CSeq"), Some("3"));
    assert!(buf.is_empty());
}

#[test]
fn header_terminator_split_across_reads_is_not_mistaken_for_a_boundary() {
    // "\r\n\r" приходит отдельно от последнего "\n".
    let mut buf = b"GET /info RTSP/1.0\r\nCSeq: 9\r\n\r".to_vec();
    assert_eq!(decode_request(&mut buf), Ok(None));
    buf.push(b'\n');
    let req = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(req.headers.get("CSeq"), Some("9"));
}

#[test]
fn content_length_in_middle_of_headers_is_honoured() {
    let mut buf =
        b"ANNOUNCE /x RTSP/1.0\r\nContent-Length: 2\r\nCSeq: 4\r\nContent-Type: text/parameters\r\n\r\nhi"
            .to_vec();
    let req = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(req.body, b"hi");
    assert_eq!(req.headers.get("Content-Type"), Some("text/parameters"));
    assert!(buf.is_empty());
}

#[test]
fn error_leaves_the_buffer_untouched_for_diagnosis() {
    let mut buf = format!(
        "POST / RTSP/1.0\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    )
    .into_bytes();
    let before = buf.clone();
    assert_eq!(decode_request(&mut buf), Err(Error::BodyTooLong));
    assert_eq!(buf, before, "ошибка не должна съедать байты");
}

#[test]
fn head_limit_allows_exactly_max_head() {
    // Стартовая строка и терминатор ровно по границе.
    let pad = MAX_HEAD - 4 - b"GET / RTSP/1.0\r\nX: ".len();
    let mut buf = format!("GET / RTSP/1.0\r\nX: {}\r\n\r\n", "a".repeat(pad)).into_bytes();
    assert_eq!(buf.len(), MAX_HEAD);
    let req = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(req.method, "GET");

    // Проверяется позиция терминатора: +5 байт сдвигают его за предел.
    let mut over = format!("GET / RTSP/1.0\r\nX: {}\r\n\r\n", "a".repeat(pad + 5)).into_bytes();
    assert_eq!(decode_request(&mut over), Err(Error::HeadTooLong));
}

#[test]
fn non_utf8_and_control_free_headers() {
    // Байты вне UTF-8 в заголовке — ошибка заголовка, а не паника.
    let mut bad = b"GET /x RTSP/1.0\r\nX-Bad: \xc3\x28\r\n\r\n".to_vec();
    assert_eq!(decode_request(&mut bad), Err(Error::BadHeader));

    // Пустое значение допустимо и сохраняется как пустая строка.
    let mut ok = b"GET /x RTSP/1.0\r\nX-Empty:\r\n\r\n".to_vec();
    let req = decode_request(&mut ok).unwrap().unwrap();
    assert_eq!(req.headers.get("X-Empty"), Some(""));
}

#[test]
fn response_with_body_and_trailing_request_keeps_next_boundary() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"RTSP/1.0 200 OK\r\nCSeq: 5\r\nContent-Length: 4\r\n\r\nbody");
    buf.extend_from_slice(b"GET /info RTSP/1.0\r\nCSeq: 6\r\n\r\n");

    let resp = decode_response(&mut buf).unwrap().unwrap();
    assert_eq!((resp.status, resp.body.as_slice()), (200, &b"body"[..]));
    let req = decode_request(&mut buf).unwrap().unwrap();
    assert_eq!(req.headers.get("CSeq"), Some("6"));
    assert!(buf.is_empty());
}

#[test]
fn limits_do_not_shift_with_query_or_version() {
    // Один и тот же Content-Length при HTTP/1.1 и RTSP/1.0 даёт одну границу.
    for version in ["RTSP/1.0", "HTTP/1.1"] {
        let mut buf = format!("POST /p?x=1 {version}\r\nContent-Length: 1\r\n\r\naZ").into_bytes();
        let req = decode_request(&mut buf).unwrap().unwrap();
        assert_eq!(req.version, version);
        assert_eq!(req.body, b"a");
        assert_eq!(buf, b"Z");
    }
}
