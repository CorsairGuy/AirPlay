//! Часы: PTP (профиль Apple) и привязка медиавремени к локальным часам.
//!
//! Без ввода-вывода: служба `timesync` кормит [`Ptp`] пакетами с портов
//! 319/320 и временем их приёма, а плееры спрашивают смещение часов
//! отправителя. Поведение повторяет nqptp из shairport-sync: мы только
//! слушаем Sync/Follow_Up мастера и сами в выборах не участвуем, кроме
//! «будящего» Announce для замолчавших часов.

mod filter;
pub mod ptp;

pub use filter::OffsetFilter;
pub use ptp::{Action, Ptp, Timeline};

/// Медиавремя AirPlay 2: момент `network_ns` на часах отправителя (PTP)
/// соответствует RTP-метке `rtp`, поток идёт со скоростью `rate` (0 — пауза).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub rtp: u32,
    pub network_ns: u64,
    pub timeline: u64,
    pub playing: bool,
}

impl Anchor {
    /// `networkTimeSecs` и `networkTimeFrac` (доля секунды в 2^-64) из
    /// SETRATEANCHORTIME в наносекунды.
    pub fn network_time(secs: u64, frac: u64) -> u64 {
        let frac_ns = ((frac >> 32) * 1_000_000_000) >> 32;
        secs.saturating_mul(1_000_000_000).saturating_add(frac_ns)
    }

    /// Локальное время (нс), когда должна прозвучать метка `rtp`.
    pub fn local_time_of(&self, rtp: u32, sample_rate: u32, offset_ns: i64) -> i64 {
        let frames = rtp.wrapping_sub(self.rtp) as i32 as i64;
        let network = self.network_ns as i64 + frames * 1_000_000_000 / sample_rate as i64;
        network - offset_ns
    }
}

/// Откуда плеер узнаёт смещение часов отправителя.
pub trait TimeSource: Send + Sync {
    /// `мастер − локальные` для шкалы `timeline`, нс.
    fn offset(&self, timeline: u64) -> Option<i64>;
}

/// Без PTP: смещения нет, плеер играет по своим часам.
pub struct NoTime;

impl TimeSource for NoTime {
    fn offset(&self, _timeline: u64) -> Option<i64> {
        None
    }
}

/// Монотонные локальные часы процесса, нс. Общая шкала для PTP и вывода звука.
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_time_from_fraction() {
        assert_eq!(Anchor::network_time(2, 1 << 63), 2_500_000_000);
        assert_eq!(Anchor::network_time(0, 0), 0);
    }

    #[test]
    fn local_time_follows_rtp() {
        let a = Anchor {
            rtp: 1000,
            network_ns: 10_000_000_000,
            timeline: 1,
            playing: true,
        };
        // 44100 кадров — ровно секунда, смещение мастер−локальные = 4 с.
        assert_eq!(
            a.local_time_of(1000 + 44100, 44100, 4_000_000_000),
            7_000_000_000
        );
        assert_eq!(a.local_time_of(1000 - 441, 44100, 0), 9_990_000_000);
    }
}
