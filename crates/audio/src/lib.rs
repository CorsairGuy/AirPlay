//! Вывод звука: стерео i16 в звуковую карту с точной задержкой буфера.
//!
//! На Linux — ALSA напрямую. libasound подгружается при запуске, поэтому
//! для сборки и кросс-компиляции не нужны ни заголовки, ни pkg-config, а без
//! звуковой подсистемы бинарник всё равно запускается (с выводом в никуда).
//! На Windows и macOS — cpal (WASAPI и CoreAudio).

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(not(target_os = "linux"))]
mod system;

pub const CHANNELS: usize = 2;

/// Устройство вывода. `write` блокирует, пока в буфере карты нет места, и
/// тем самым задаёт темп конвейеру.
pub trait Sink: Send {
    fn sample_rate(&self) -> u32;
    /// Пишет интерлив-стерео целиком.
    fn write(&mut self, samples: &[i16]) -> io::Result<()>;
    /// Сколько кадров уже отдано карте, но ещё не прозвучало.
    fn delay_frames(&mut self) -> u32;
    /// Останавливает воспроизведение и выбрасывает буфер (пауза, перемотка).
    fn stop(&mut self);
    /// Громкость AirPlay в дБ (−30…0, −144 — без звука). Вывод с аппаратной
    /// громкостью применяет её сам и возвращает `true`; иначе плеер
    /// масштабирует сэмплы.
    fn set_volume(&mut self, _db: f32) -> bool {
        false
    }
}

/// Как открыть вывод для частоты `sample_rate` с буфером `latency`.
/// Встраивающая программа (например, под конкретное железо) передаёт
/// свою фабрику вместо имени устройства.
#[derive(Clone)]
pub struct SinkFactory(Arc<OpenFn>);

type OpenFn = dyn Fn(u32, Duration) -> io::Result<Box<dyn Sink>> + Send + Sync;

impl SinkFactory {
    pub fn new(
        f: impl Fn(u32, Duration) -> io::Result<Box<dyn Sink>> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(f))
    }

    pub fn open(&self, sample_rate: u32, latency: Duration) -> io::Result<Box<dyn Sink>> {
        (self.0)(sample_rate, latency)
    }
}

impl fmt::Debug for SinkFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SinkFactory")
    }
}

/// Открывает устройство: `null` — вывод в никуда с темпом реального времени,
/// `default` или имя ALSA вроде `hw:0,0` / `plughw:1` — звуковая карта.
pub fn open(device: &str, sample_rate: u32, latency: Duration) -> io::Result<Box<dyn Sink>> {
    if device == "null" {
        return Ok(Box::new(Null::new(sample_rate)));
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(alsa::Alsa::open(device, sample_rate, latency)?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Box::new(system::System::open(
            device,
            sample_rate,
            latency,
        )?))
    }
}

/// Вывод в никуда: принимает данные в темпе реального времени и держит
/// фиксированную задержку, как настоящая карта. Нужен для тестов и для
/// устройств без звука.
pub struct Null {
    rate: u32,
    /// Момент, когда прозвучит последний записанный кадр.
    played_until: Option<Instant>,
    buffer: Duration,
}

impl Null {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            played_until: None,
            buffer: Duration::from_millis(50),
        }
    }
}

impl Sink for Null {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let now = Instant::now();
        let frames = (samples.len() / CHANNELS) as u64;
        let len = Duration::from_nanos(frames * 1_000_000_000 / self.rate as u64);
        let start = self.played_until.filter(|t| *t > now).unwrap_or(now);
        let end = start + len;
        self.played_until = Some(end);
        // Как у карты: ждём, только когда буфер переполнен.
        if let Some(wait) = end.checked_duration_since(now + self.buffer) {
            std::thread::sleep(wait);
        }
        Ok(())
    }

    fn delay_frames(&mut self) -> u32 {
        let now = Instant::now();
        self.played_until
            .and_then(|t| t.checked_duration_since(now))
            .map_or(0, |d| {
                (d.as_nanos() * self.rate as u128 / 1_000_000_000) as u32
            })
    }

    fn stop(&mut self) {
        self.played_until = None;
    }
}

/// Громкость AirPlay в дБ (−30…0, −144 — без звука) в множитель Q15.
pub fn gain_q15(db: f32) -> i32 {
    if db <= -30.0 {
        return 0;
    }
    (10f32.powf(db.min(0.0) / 20.0) * 32768.0) as i32
}

/// Применяет громкость на месте.
pub fn apply_gain(samples: &mut [i16], gain_q15: i32) {
    if gain_q15 >= 32768 {
        return;
    }
    for s in samples {
        *s = ((*s as i32 * gain_q15) >> 15) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_keeps_real_time() {
        let mut s = Null::new(44100);
        let start = Instant::now();
        let chunk = vec![0i16; 4410 * CHANNELS]; // 100 мс
        for _ in 0..3 {
            s.write(&chunk).unwrap();
        }
        let elapsed = start.elapsed();
        // 300 мс данных минус 50 мс буфера.
        assert!(elapsed >= Duration::from_millis(240), "{elapsed:?}");
        // После сна задержка зависит от того, насколько планировщик проспал
        // (на машинах CI бывает больше буфера), поэтому только верхняя граница.
        assert!(s.delay_frames() <= 2300);
        s.stop();
        // Запись меньше буфера не ждёт: задержка — ровно записанное.
        s.write(&chunk[..882 * CHANNELS]).unwrap();
        let delay = s.delay_frames();
        assert!((700..=882).contains(&delay), "задержка {delay}");
        s.stop();
        assert_eq!(s.delay_frames(), 0);
    }

    #[test]
    fn volume_curve() {
        assert_eq!(gain_q15(0.0), 32768);
        assert_eq!(gain_q15(-144.0), 0);
        assert_eq!(gain_q15(-30.0), 0);
        let g = gain_q15(-6.0);
        assert!((16300..16500).contains(&g), "{g}");
        let mut s = [1000i16, -1000, i16::MAX];
        apply_gain(&mut s, g);
        assert_eq!(s[0], 501);
        assert_eq!(s[1], -502);
    }
}
