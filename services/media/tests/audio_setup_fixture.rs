//! D01/R01: обработчик SETUP на обезличенных plist fixtures, построенных по
//! shairport-sync 5.5.2 rtsp.c `handle_setup_2` и UxPlay 534e2cc0
//! `lib/raop_handlers.h`. Устройство и ключи реального отправителя не нужны.

use std::io::Cursor;
use std::sync::Arc;

use media::control::Remote;
use media::events;
use media::rx::{Media, MediaContext, SessionGroup};
use rtsp::Request;

struct NoGroups;

impl SessionGroup for NoGroups {
    fn set_group(&self, _owner: u64, _group: Option<String>) {}
}

fn context() -> MediaContext {
    MediaContext {
        player: Arc::new(media::rx::Player::new(
            audio_stream::Config {
                device: "null".into(),
                ..Default::default()
            },
            timesync::TimeSync::idle(1),
        )),
        #[cfg(feature = "mirroring")]
        mirror: None,
        #[cfg(feature = "hls")]
        hls: None,
        remote: Remote::new(),
        mrp: None,
        pairing_id: "fixture".into(),
        session_group: Arc::new(NoGroups),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/interop/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn setup(body: Vec<u8>) -> Request {
    let mut req = Request::new("SETUP", "/stream", "RTSP/1.0");
    req.body = body;
    req
}

fn teardown() -> Request {
    Request::new("TEARDOWN", "/stream", "RTSP/1.0")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reference_audio_descriptor_is_accepted_and_teardown_releases_it() {
    let media_ctx = context();
    let mut media = Media::new(
        &media_ctx,
        "127.0.0.1".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
    );
    let resp = media
        .handle(&setup(fixture("airplay2-audio-setup.plist")))
        .expect("SETUP обрабатывается медиачастью");
    assert_eq!(resp.status, 200);
    let reply = plist::Value::from_reader(Cursor::new(resp.body)).expect("ответ — plist");
    let streams = reply
        .as_dictionary()
        .and_then(|d| d.get("streams"))
        .and_then(|v| v.as_array())
        .expect("streams в ответе");
    let stream = streams[0].as_dictionary().unwrap();
    assert_eq!(
        stream.get("type").and_then(|v| v.as_unsigned_integer()),
        Some(96)
    );
    assert!(
        stream
            .get("dataPort")
            .and_then(|v| v.as_unsigned_integer())
            .is_some_and(|p| p != 0)
    );
    assert!(
        stream
            .get("controlPort")
            .and_then(|v| v.as_unsigned_integer())
            .is_some_and(|p| p != 0)
    );

    let resp = media.handle(&teardown()).unwrap();
    assert_eq!(resp.status, 200);
}

#[test]
fn a_short_stream_key_is_refused_without_accepting_it_as_a_working_key() {
    let media_ctx = context();
    let mut media = Media::new(
        &media_ctx,
        "127.0.0.1".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
    );
    let resp = media
        .handle(&setup(fixture("airplay2-audio-setup-short-shk.plist")))
        .unwrap();
    assert_eq!(resp.status, 500);
    let resp = media.handle(&teardown()).unwrap();
    assert_eq!(resp.status, 200);
}
