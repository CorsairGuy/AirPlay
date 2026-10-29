//! Когда показывать кадр. Кадры приходят из декодера с моментом показа по
//! локальным часам; [`Pacer`] держит их до этого момента. Если показ
//! отстал и наступило время сразу нескольких кадров, показывается
//! последний, а остальные отбрасываются: задержка не копится.

use std::collections::VecDeque;

/// Больше кадров не держим: при 60 к/с это 2 с.
const MAX_QUEUE: usize = 120;

pub struct Pacer<T> {
    queue: VecDeque<(i64, T)>,
    skip_late: bool,
    dropped: u64,
}

impl<T> Pacer<T> {
    /// `skip_late == false` — показывать все кадры подряд, даже опоздавшие.
    pub fn new(skip_late: bool) -> Self {
        Self {
            queue: VecDeque::new(),
            skip_late,
            dropped: 0,
        }
    }

    /// Кадр к показу в момент `due` (нс, [`clock::now_ns`]).
    pub fn push(&mut self, due: i64, item: T) {
        if self.queue.len() >= MAX_QUEUE {
            self.queue.pop_front();
            self.dropped += 1;
        }
        // Почти всегда в конец: декодер отдаёт кадры в порядке показа.
        let at = self
            .queue
            .iter()
            .rposition(|(d, _)| *d <= due)
            .map_or(0, |i| i + 1);
        self.queue.insert(at, (due, item));
    }

    /// Когда наступит время следующего кадра.
    pub fn next_due(&self) -> Option<i64> {
        self.queue.front().map(|(d, _)| *d)
    }

    /// Кадр, время которого наступило к `now`.
    pub fn pop_due(&mut self, now: i64) -> Option<T> {
        if self.next_due()? > now {
            return None;
        }
        if self.skip_late {
            while self.queue.get(1).is_some_and(|(d, _)| *d <= now) {
                self.queue.pop_front();
                self.dropped += 1;
            }
        }
        self.queue.pop_front().map(|(_, t)| t)
    }

    /// Отбросить кадр, не ставя в очередь (опоздал ещё до неё).
    pub fn skip(&mut self) {
        self.dropped += 1;
    }

    /// Сколько кадров отброшено с начала.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_until_due() {
        let mut p = Pacer::new(true);
        p.push(100, 'a');
        p.push(200, 'b');
        assert_eq!(p.next_due(), Some(100));
        assert_eq!(p.pop_due(99), None);
        assert_eq!(p.pop_due(100), Some('a'));
        assert_eq!(p.pop_due(150), None);
        assert_eq!(p.pop_due(250), Some('b'));
        assert!(p.is_empty());
        assert_eq!(p.dropped(), 0);
    }

    #[test]
    fn late_frames_skip_to_newest() {
        let mut p = Pacer::new(true);
        for (i, due) in [10, 20, 30, 40].into_iter().enumerate() {
            p.push(due, i);
        }
        assert_eq!(p.pop_due(35), Some(2));
        assert_eq!(p.dropped(), 2);
        assert_eq!(p.pop_due(35), None);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn without_skipping_shows_all() {
        let mut p = Pacer::new(false);
        p.push(10, 1);
        p.push(10, 2);
        assert_eq!(p.pop_due(50), Some(1));
        assert_eq!(p.pop_due(50), Some(2));
    }

    #[test]
    fn keeps_order_and_bounds_queue() {
        let mut p = Pacer::new(false);
        p.push(30, 'c');
        p.push(10, 'a');
        p.push(20, 'b');
        assert_eq!(p.pop_due(100), Some('a'));
        assert_eq!(p.pop_due(100), Some('b'));
        assert_eq!(p.pop_due(100), Some('c'));

        let mut p = Pacer::new(false);
        for i in 0..MAX_QUEUE as i64 + 5 {
            p.push(i, i);
        }
        assert_eq!(p.len(), MAX_QUEUE);
        assert_eq!(p.dropped(), 5);
        assert_eq!(p.pop_due(i64::MAX), Some(5));
    }
}
