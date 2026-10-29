//! Фильтр смещения часов мастера относительно локальных.
//!
//! Задержка в сети только уменьшает наблюдаемое смещение
//! `мастер − приём`, поэтому истинное смещение — верхняя огибающая
//! выборок: рост принимаем быстро, падение — очень медленно.

/// Скачок, после которого считаем, что часы мастера переставили.
const STEP_NS: i64 = 100_000_000;
/// Сколько подряд выборок со скачком нужно, чтобы принять новое значение.
const STEP_CONFIRM: u32 = 8;
/// Первые выборки принимаем целиком, пока фильтр не устоялся.
const WARMUP: u32 = 8;

#[derive(Debug, Clone, Default)]
pub struct OffsetFilter {
    estimate: i64,
    samples: u32,
    outliers: u32,
}

impl OffsetFilter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Текущая оценка `мастер − локальные`, нс; `None` до первой выборки.
    pub fn offset(&self) -> Option<i64> {
        (self.samples > 0).then_some(self.estimate)
    }

    pub fn update(&mut self, sample: i64) -> i64 {
        if self.samples == 0 {
            self.estimate = sample;
            self.samples = 1;
            return sample;
        }
        let diff = sample - self.estimate;
        if diff.abs() > STEP_NS {
            self.outliers += 1;
            if self.outliers >= STEP_CONFIRM {
                self.reset();
                return self.update(sample);
            }
            return self.estimate;
        }
        self.outliers = 0;
        self.samples = self.samples.saturating_add(1);
        self.estimate += if diff > 0 {
            if self.samples <= WARMUP {
                diff
            } else {
                diff / 16
            }
        } else {
            diff / 256
        };
        self.estimate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_upper_envelope() {
        let mut f = OffsetFilter::default();
        assert_eq!(f.offset(), None);
        // Истинное смещение 1 с, задержка сети 0–2 мс.
        for i in 0..200 {
            let delay = (i * 7919 % 2000) * 1000;
            f.update(1_000_000_000 - delay);
        }
        let off = f.offset().unwrap();
        // Равномерная задержка 0–2 мс — худший случай; в обычной сети разброс меньше.
        assert!((1_000_000_000 - off).abs() < 500_000, "оценка {off}");
    }

    #[test]
    fn ignores_single_jump_and_follows_step() {
        let mut f = OffsetFilter::default();
        for _ in 0..20 {
            f.update(5_000_000);
        }
        assert_eq!(f.update(900_000_000), 5_000_000, "одиночный выброс");
        for _ in 0..STEP_CONFIRM {
            f.update(900_000_000);
        }
        assert_eq!(f.offset(), Some(900_000_000), "часы переставили");
    }
}
