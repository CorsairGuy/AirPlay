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
        ..Config::default()
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
    let stream = Stream::buffered(config(), &shk, Arc::new(NoTime)).unwrap();
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
    let stream = Stream::buffered(config(), &shk, Arc::new(NoTime)).unwrap();
    let sent = send_frames(&stream, &shk).await;
    assert!(wait_for(&stream, sent, Duration::from_secs(2)).await);
    stream.playout().flush(None, 10 * 1024);
    assert_eq!(stream.playout().buffered_packets(), sent - 10);
    stream.playout().flush(Some(15 * 1024), 1_000_000);
    assert_eq!(stream.playout().buffered_packets(), 5);
}

/// Вывод встраивающей программы: громкость берёт на себя, сэмплы получает
/// без масштабирования.
struct HardwareVolume {
    inner: audio::Null,
    volume: Arc<std::sync::Mutex<Option<f32>>>,
    peak: Arc<std::sync::atomic::AtomicU32>,
}

impl audio::Sink for HardwareVolume {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        let peak = samples
            .iter()
            .map(|s| s.unsigned_abs() as u32)
            .max()
            .unwrap_or(0);
        self.peak
            .fetch_max(peak, std::sync::atomic::Ordering::Relaxed);
        self.inner.write(samples)
    }
    fn delay_frames(&mut self) -> u32 {
        self.inner.delay_frames()
    }
    fn stop(&mut self) {
        self.inner.stop()
    }
    fn set_volume(&mut self, db: f32) -> bool {
        *self.volume.lock().unwrap() = Some(db);
        true
    }
}

#[tokio::test]
async fn custom_sink_with_hardware_volume() {
    let volume = Arc::new(std::sync::Mutex::new(None));
    let peak = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let (v, p) = (volume.clone(), peak.clone());
    let cfg = Config {
        sink: Some(audio::SinkFactory::new(move |rate, _| {
            Ok(Box::new(HardwareVolume {
                inner: audio::Null::new(rate),
                volume: v.clone(),
                peak: p.clone(),
            }))
        })),
        ..config()
    };
    let shk = [5u8; 32];
    let stream = Stream::buffered(cfg, &shk, Arc::new(NoTime)).unwrap();
    let sent = send_frames(&stream, &shk).await;
    assert!(wait_for(&stream, sent, Duration::from_secs(2)).await);
    // −29 дБ цифрой почти заглушили бы синус; вывод должен получить его целым.
    stream.playout().set_volume(-29.0);
    stream.playout().set_anchor(Anchor {
        rtp: 0,
        network_ns: 0,
        timeline: 0,
        playing: true,
    });
    assert!(wait_for(&stream, 0, Duration::from_secs(5)).await);
    assert_eq!(*volume.lock().unwrap(), Some(-29.0));
    let peak = peak.load(std::sync::atomic::Ordering::Relaxed);
    assert!(peak > 2000, "сэмплы масштабированы: {peak}");
}

/// Realtime как у зеркалирования: AES-CBC по UDP, время из синхропакета 0x54.
#[tokio::test]
async fn realtime_aes_cbc_with_sync() {
    let key = [4u8; 16];
    let iv = [5u8; 16];
    let format = codec::Format::from_ssrc(0x1600_0000).unwrap();
    let stream = Stream::realtime(
        config(),
        ap_audio_stream::StreamKey::AesCbc { key, iv },
        format,
        Arc::new(NoTime),
    )
    .unwrap();
    let cbc = crypto::cbc::AudioCbc::new(&key, &iv);
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // Синхропакет: метка 0 звучит в момент NTP «сейчас»; без часов NTP
    // плеер всё равно играет по своим.
    let mut sync = [0u8; 20];
    sync[..4].copy_from_slice(&[0x90, 0xD4, 0, 7]);
    sock.send_to(&sync, ("127.0.0.1", stream.control_port))
        .await
        .unwrap();

    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    for (i, f) in frames.iter().enumerate() {
        let h = Header {
            marker: false,
            payload_type: 0x60,
            seq: i as u16,
            timestamp: i as u32 * 1024,
            ssrc: 0,
        };
        let mut packet = h.encode().to_vec();
        let mut body = f.to_vec();
        cbc.encrypt(&mut body);
        packet.extend_from_slice(&body);
        sock.send_to(&packet, ("127.0.0.1", stream.data_port))
            .await
            .unwrap();
    }
    // Всё дошло и проиграно: буфер опустел.
    let start = Instant::now();
    let mut seen = 0;
    while start.elapsed() < Duration::from_secs(5) {
        let n = stream.playout().buffered_packets();
        seen = seen.max(n);
        if seen > 0 && n == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(seen > 0, "пакеты не дошли");
    assert_eq!(stream.playout().buffered_packets(), 0, "не проиграно");
}
