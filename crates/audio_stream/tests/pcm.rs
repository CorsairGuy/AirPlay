//! Сквозные тесты приёма звука: синтетический отправитель шлёт известные
//! данные, приёмник расшифровывает и декодирует их, а тест проверяет PCM,
//! который дошёл до вывода. Ни один тест не считает воспроизведение успешным
//! по одному факту доставки пакетов.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audio_stream::{Config, Stream, StreamKey};
use clock::{Anchor, NoTime};
use codec::{Codec, Format};
use rtp::{AudioCipher, Header};
use tokio::io::AsyncWriteExt;

/// Записывающий вывод: копит PCM, считает `stop()` и запоминает громкость.
#[derive(Default)]
struct State {
    samples: Mutex<Vec<i16>>,
    stops: AtomicUsize,
    volume: Mutex<Option<f32>>,
}

struct Recorder {
    inner: audio::Null,
    state: Arc<State>,
}

impl audio::Sink for Recorder {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        self.state
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(samples);
        self.inner.write(samples)
    }

    fn delay_frames(&mut self) -> u32 {
        self.inner.delay_frames()
    }

    fn stop(&mut self) {
        self.state.stops.fetch_add(1, Ordering::Relaxed);
        self.inner.stop();
    }

    fn set_volume(&mut self, db: f32) -> bool {
        *self.state.volume.lock().unwrap_or_else(|e| e.into_inner()) = Some(db);
        false
    }
}

fn recorder() -> (audio::SinkFactory, Arc<State>) {
    let state = Arc::new(State::default());
    let sink_state = state.clone();
    let factory = audio::SinkFactory::new(move |rate, _| {
        Ok(Box::new(Recorder {
            inner: audio::Null::new(rate),
            state: sink_state.clone(),
        }) as Box<dyn audio::Sink>)
    });
    (factory, state)
}

fn config(sink: audio::SinkFactory) -> Config {
    Config {
        device: "null".into(),
        output_latency: Duration::from_millis(20),
        fallback_latency: Duration::from_millis(20),
        sink: Some(sink),
        ..Config::default()
    }
}

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

/// Шифрованные пакеты буферизованного потока для кадров `range`.
fn buffered_wire(frames: &[&[u8]], shk: &[u8; 32], range: std::ops::Range<usize>) -> Vec<u8> {
    let cipher = AudioCipher::new(shk);
    let mut wire = Vec::new();
    for i in range {
        let header = Header {
            marker: false,
            payload_type: 0x60,
            seq: i as u16,
            timestamp: i as u32 * 1024,
            ssrc: 0x1600_0000,
        };
        rtp::frame_buffered(&cipher.seal(&header, frames[i], i as u64), &mut wire);
    }
    wire
}

async fn connect(stream: &Stream) -> tokio::net::TcpStream {
    tokio::net::TcpStream::connect(("127.0.0.1", stream.data_port))
        .await
        .expect("подключение к буферизованному потоку")
}

fn anchor(playing: bool) -> Anchor {
    Anchor {
        rtp: 0,
        network_ns: 0,
        timeline: 0,
        playing,
    }
}

async fn wait_until(mut ready: impl FnMut() -> bool, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ready()
}

fn recorded(state: &State) -> Vec<i16> {
    state
        .samples
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Левый канал без ведущей и хвостовой тишины.
fn audible(pcm: &[i16]) -> Vec<i16> {
    let left: Vec<i16> = pcm.iter().step_by(2).copied().collect();
    let first = left.iter().position(|s| s.unsigned_abs() > 100);
    let last = left.iter().rposition(|s| s.unsigned_abs() > 100);
    match (first, last) {
        (Some(first), Some(last)) if last > first => left[first..=last].to_vec(),
        _ => Vec::new(),
    }
}

fn peak(samples: &[i16]) -> u32 {
    samples
        .iter()
        .map(|s| s.unsigned_abs() as u32)
        .max()
        .unwrap_or(0)
}

fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// Частота по переходам через ноль — для проверки, что звучит ожидаемый тон.
fn frequency(samples: &[i16], rate: f64) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();
    crossings as f64 * rate / (2.0 * samples.len() as f64)
}

/// Различные значения подряд, без нулей: порядок и содержимое realtime-пакетов.
fn runs(samples: &[i16]) -> Vec<i16> {
    let mut out = Vec::new();
    for s in samples.iter().copied().filter(|s| *s != 0) {
        if out.last() != Some(&s) {
            out.push(s);
        }
    }
    out
}

#[tokio::test]
async fn buffered_known_aac_reaches_the_sink_as_pcm() {
    let (factory, state) = recorder();
    let shk = [3u8; 32];
    let stream = Stream::buffered(config(factory), &shk, Arc::new(NoTime)).unwrap();
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));

    let mut conn = connect(&stream).await;
    conn.write_all(&buffered_wire(&frames, &shk, 0..frames.len()))
        .await
        .unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == frames.len(),
            Duration::from_secs(3)
        )
        .await,
        "пакеты не дошли: {} из {}",
        stream.playout().buffered_packets(),
        frames.len()
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0,
            Duration::from_secs(10)
        )
        .await,
        "не проиграно: осталось {}",
        stream.playout().buffered_packets()
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Ожидаемый PCM — независимое декодирование тех же кадров.
    let format = Format::from_ssrc(0x1600_0000).unwrap();
    let mut decoder = codec::Decoder::new(format).unwrap();
    let mut expected = Vec::new();
    for frame in &frames {
        decoder.decode(frame, &mut expected).unwrap();
    }
    let expected_left: Vec<i16> = expected.iter().step_by(2).copied().collect();

    let pcm = recorded(&state);
    let played = audible(&pcm);
    assert!(!played.is_empty(), "в вывод не пришёл звук");
    let played_frames = pcm.len() / 2;
    let expected_frames = expected.len() / 2;
    assert!(
        played_frames + 4410 >= expected_frames,
        "потеряно больше 100 мс: {played_frames} из {expected_frames} кадров"
    );

    let expected_peak = peak(&expected_left);
    assert!(expected_peak > 2000, "эталон тихий: {expected_peak}");
    let ratio = peak(&played) as f64 / expected_peak as f64;
    assert!(
        (0.85..=1.15).contains(&ratio),
        "амплитуда не совпала: {} против {expected_peak}",
        peak(&played)
    );
    let rms_ratio = rms(&played) / rms(&expected_left).max(1.0);
    assert!(
        (0.8..=1.2).contains(&rms_ratio),
        "энергия не совпала: {rms_ratio:.3}"
    );
    let tone = frequency(&played, 44_100.0);
    assert!(
        (400.0..=480.0).contains(&tone),
        "частота {tone:.1} Гц вместо 440"
    );
}

#[tokio::test]
async fn buffered_pause_stops_output_and_resume_continues() {
    let (factory, state) = recorder();
    let shk = [4u8; 32];
    let stream = Stream::buffered(config(factory), &shk, Arc::new(NoTime)).unwrap();
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    let mut conn = connect(&stream).await;
    conn.write_all(&buffered_wire(&frames, &shk, 0..frames.len()))
        .await
        .unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == frames.len(),
            Duration::from_secs(3)
        )
        .await
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(|| recorded(&state).len() >= 4096, Duration::from_secs(5)).await,
        "звук не пошёл до паузы"
    );

    stream.playout().set_anchor(anchor(false));
    assert!(
        wait_until(
            || state.stops.load(Ordering::Relaxed) > 0,
            Duration::from_secs(2)
        )
        .await,
        "вывод не остановлен на паузе"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let before = recorded(&state).len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = recorded(&state).len();
    assert!(
        after <= before + 2048,
        "звук шёл на паузе: {before} -> {after}"
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || recorded(&state).len() > after + 8192,
            Duration::from_secs(5)
        )
        .await,
        "после возобновления звук не пошёл"
    );
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0,
            Duration::from_secs(10)
        )
        .await,
        "после возобновления не доиграно"
    );
}

/// Отправитель открывает новое TCP-подключение, не закрывая старое: прежнее
/// должно быть заменено, иначе приём висит на нём и звук пропадает.
#[tokio::test]
async fn buffered_new_connection_replaces_the_open_one() {
    let (factory, state) = recorder();
    let shk = [5u8; 32];
    let stream = Stream::buffered(config(factory), &shk, Arc::new(NoTime)).unwrap();
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    let half = frames.len() / 2;

    let mut first = connect(&stream).await;
    first
        .write_all(&buffered_wire(&frames, &shk, 0..half))
        .await
        .unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == half,
            Duration::from_secs(3)
        )
        .await
    );

    // Старое подключение остаётся открытым.
    let mut second = connect(&stream).await;
    second
        .write_all(&buffered_wire(&frames, &shk, half..frames.len()))
        .await
        .unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == frames.len(),
            Duration::from_secs(3)
        )
        .await,
        "новое подключение не принято: {} из {}",
        stream.playout().buffered_packets(),
        frames.len()
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0,
            Duration::from_secs(10)
        )
        .await
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!audible(&recorded(&state)).is_empty(), "звук не дошёл");
}

#[tokio::test]
async fn buffered_reconnects_after_the_connection_drops() {
    let (factory, state) = recorder();
    let shk = [6u8; 32];
    let stream = Stream::buffered(config(factory), &shk, Arc::new(NoTime)).unwrap();
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    let half = frames.len() / 2;

    {
        let mut first = connect(&stream).await;
        first
            .write_all(&buffered_wire(&frames, &shk, 0..half))
            .await
            .unwrap();
    }
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == half,
            Duration::from_secs(3)
        )
        .await
    );

    let mut second = connect(&stream).await;
    second
        .write_all(&buffered_wire(&frames, &shk, half..frames.len()))
        .await
        .unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == frames.len(),
            Duration::from_secs(3)
        )
        .await,
        "после обрыва новое подключение не принято"
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0,
            Duration::from_secs(10)
        )
        .await
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!audible(&recorded(&state)).is_empty());
}

/// Realtime (тип 96) с PCM: известные сэмплы должны дойти до вывода
/// по порядку. Датаграмма больше 2048 байт — регрессия на обрезанный буфер
/// приёма (пакет ALAC 24 бита по 352 кадра не влезал).
#[tokio::test]
async fn realtime_known_pcm_arrives_intact() {
    let (factory, state) = recorder();
    let format = Format {
        codec: Codec::Pcm {
            frames_per_packet: 1024,
        },
        sample_rate: 44_100,
        channels: 2,
    };
    let stream = Stream::realtime(
        config(factory),
        StreamKey::ChaCha([7u8; 32]),
        format,
        Arc::new(NoTime),
    )
    .unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let mut sync = [0u8; 28];
    sync[0] = 0x90;
    sync[1] = 0xD7;
    sock.send_to(&sync, ("127.0.0.1", stream.control_port))
        .await
        .unwrap();

    let cipher = AudioCipher::new(&[7u8; 32]);
    let packets = 10u16;
    for i in 0..packets {
        let value = 1000 + i as i16 * 100;
        let mut payload = Vec::with_capacity(4096);
        for _ in 0..1024 * 2 {
            payload.extend_from_slice(&value.to_be_bytes());
        }
        let header = Header {
            marker: false,
            payload_type: 0x60,
            seq: i,
            timestamp: u32::from(i) * 1024,
            ssrc: 0,
        };
        let sealed = cipher.seal(&header, &payload, u64::from(i));
        assert!(
            sealed.len() > 2048,
            "датаграмма {} байт не проверяет старый буфер приёма",
            sealed.len()
        );
        sock.send_to(&sealed, ("127.0.0.1", stream.data_port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0 && !recorded(&state).is_empty(),
            Duration::from_secs(5)
        )
        .await,
        "realtime-пакеты не проиграны"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;

    let played = runs(&recorded(&state));
    assert!(
        played.len() >= 8,
        "слишком мало пакетов до вывода: {played:?}"
    );
    assert_eq!(
        *played.last().unwrap(),
        1000 + (packets as i16 - 1) * 100,
        "последний пакет не дошёл: {played:?}"
    );
    for pair in played.windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            100,
            "пакеты перепутаны или потеряны: {played:?}"
        );
    }
}

#[tokio::test]
async fn realtime_pause_stops_and_resume_continues() {
    let (factory, state) = recorder();
    let format = Format {
        codec: Codec::Pcm {
            frames_per_packet: 1024,
        },
        sample_rate: 44_100,
        channels: 2,
    };
    let stream = Stream::realtime(
        config(factory),
        StreamKey::ChaCha([8u8; 32]),
        format,
        Arc::new(NoTime),
    )
    .unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut sync = [0u8; 28];
    sync[0] = 0x90;
    sync[1] = 0xD7;
    sock.send_to(&sync, ("127.0.0.1", stream.control_port))
        .await
        .unwrap();

    let cipher = AudioCipher::new(&[8u8; 32]);
    let send = |i: u16| {
        let mut payload = Vec::with_capacity(4096);
        for _ in 0..1024 * 2 {
            payload.extend_from_slice(&(1000 + i as i16).to_be_bytes());
        }
        let header = Header {
            marker: false,
            payload_type: 0x60,
            seq: i,
            timestamp: u32::from(i) * 1024,
            ssrc: 0,
        };
        cipher.seal(&header, &payload, u64::from(i))
    };
    for i in 0..4u16 {
        sock.send_to(&send(i), ("127.0.0.1", stream.data_port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        wait_until(|| recorded(&state).len() >= 2048, Duration::from_secs(5)).await,
        "realtime не зазвучал"
    );

    stream.playout().set_anchor(anchor(false));
    assert!(
        wait_until(
            || state.stops.load(Ordering::Relaxed) > 0,
            Duration::from_secs(2)
        )
        .await,
        "вывод не остановлен на паузе"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let before = recorded(&state).len();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(recorded(&state).len() <= before + 2048, "звук шёл на паузе");

    stream.playout().set_anchor(anchor(true));
    for i in 4..8u16 {
        sock.send_to(&send(i), ("127.0.0.1", stream.data_port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        wait_until(
            || recorded(&state).len() > before + 8192,
            Duration::from_secs(5)
        )
        .await,
        "после возобновления realtime не зазвучал"
    );
}

/// Пакет с неизвестным SSRC не должен останавливать вывод навсегда.
#[tokio::test]
async fn buffered_unknown_ssrc_is_skipped_not_stalled() {
    let (factory, state) = recorder();
    let shk = [9u8; 32];
    let stream = Stream::buffered(config(factory), &shk, Arc::new(NoTime)).unwrap();
    let frames = adts_frames(include_bytes!("../../codec/tests/data/sine440.adts"));
    let cipher = AudioCipher::new(&shk);

    let mut wire = Vec::new();
    let unknown = Header {
        marker: false,
        payload_type: 0x60,
        seq: 0,
        timestamp: 0,
        ssrc: 0x0BAD_0000,
    };
    rtp::frame_buffered(&cipher.seal(&unknown, b"\x00\x00", 0), &mut wire);
    for (i, frame) in frames.iter().take(8).enumerate() {
        let header = Header {
            marker: false,
            payload_type: 0x60,
            seq: i as u16 + 1,
            timestamp: (i as u32 + 1) * 1024,
            ssrc: 0x1600_0000,
        };
        rtp::frame_buffered(&cipher.seal(&header, frame, i as u64 + 1), &mut wire);
    }
    let mut conn = connect(&stream).await;
    conn.write_all(&wire).await.unwrap();
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 9,
            Duration::from_secs(3)
        )
        .await
    );

    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || stream.playout().buffered_packets() == 0,
            Duration::from_secs(5)
        )
        .await,
        "вывод замер на неизвестном SSRC"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !audible(&recorded(&state)).is_empty(),
        "известные пакеты после неизвестного не сыграны"
    );
}

/// Громкость, пришедшая до SETUP, применяется к потоку, когда он появился.
#[tokio::test]
async fn volume_is_applied_to_the_output() {
    let (factory, state) = recorder();
    let stream = Stream::buffered(config(factory), &[11u8; 32], Arc::new(NoTime)).unwrap();
    stream.playout().set_volume(-12.0);
    stream.playout().push(0, 0x1600_0000, vec![0u8; 8]);
    stream.playout().set_anchor(anchor(true));
    assert!(
        wait_until(
            || state
                .volume
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some(),
            Duration::from_secs(3)
        )
        .await,
        "громкость не дошла до вывода"
    );
    let volume = *state.volume.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(volume, Some(-12.0));
}
