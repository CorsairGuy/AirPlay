//! Буферизованный поток целиком: шифрованные кадры AAC по TCP → вывод в никуда.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ap_audio_stream::{Config, Stream};
use clock::{Anchor, NoTime};
use rtp::{AudioCipher, Header};
use tokio::io::AsyncWriteExt;

fn adts_frames(mut data: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    while data.len() > 7 && data[0] == 0xFF && data[1] & 0xF0 == 0xF0 {
        let len =
            ((data[3] as usize & 3) << 11) | ((data[4] as usize) << 3) | (data[5] as usize >> 5);
        let header = if data[1] & 1 == 0 { 9 } else { 7 };
        frames.push(&data[header..len]);
        data = &data[len..];
    }
    frames
}

fn config() -> Config {
    Config {
        device: "null".into(),
        output_latency: Duration::from_millis(20),
        fallback_latency: Duration::from_millis(50),
    }
}

async fn send_frames(stream: &Stream, shk: &[u8; 32]) -> usize {
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    let cipher = AudioCipher::new(shk);
    let mut wire = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        let h = Header {
            marker: false,
            payload_type: 0x60,
            seq: i as u16,
            timestamp: i as u32 * 1024,
            ssrc: 0x1600_0000,
        };
        rtp::frame_buffered(&cipher.seal(&h, f, i as u64), &mut wire);
    }
    let mut conn = tokio::net::TcpStream::connect(("127.0.0.1", stream.data_port))
        .await
        .unwrap();
    conn.write_all(&wire).await.unwrap();
    frames.len()
}

async fn wait_for(stream: &Stream, count: usize, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if stream.playout().buffered_packets() == count {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn plays_buffered_aac() {
    let shk = [7u8; 32];
    let stream = Stream::buffered(config(), &shk, Arc::new(NoTime))
        .await
        .unwrap();
    let sent = send_frames(&stream, &shk).await;
    assert!(
        wait_for(&stream, sent, Duration::from_secs(2)).await,
        "кадры не дошли"
    );

    // Без якоря не играем.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(stream.playout().buffered_packets(), sent);

    let start = Instant::now();
    stream.playout().set_anchor(Anchor {
        rtp: 0,
        network_ns: 0,
        timeline: 0,
        playing: true,
    });
    assert!(
        wait_for(&stream, 0, Duration::from_secs(5)).await,
        "не проиграно"
    );
    // Вывод идёт в реальном времени: ~0,5 с звука не могли проиграть мгновенно.
    let audio = Duration::from_secs_f64(sent as f64 * 1024.0 / 44100.0);
    assert!(
        start.elapsed() > audio / 2,
        "слишком быстро: {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn flush_drops_range() {
    let shk = [9u8; 32];
    let stream = Stream::buffered(config(), &shk, Arc::new(NoTime))
        .await
        .unwrap();
    let sent = send_frames(&stream, &shk).await;
    assert!(wait_for(&stream, sent, Duration::from_secs(2)).await);
    stream.playout().flush(None, 10 * 1024);
    assert_eq!(stream.playout().buffered_packets(), sent - 10);
    stream.playout().flush(Some(15 * 1024), 1_000_000);
    assert_eq!(stream.playout().buffered_packets(), 5);
}
