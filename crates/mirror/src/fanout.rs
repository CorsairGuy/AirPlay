//! Раздача одного видеопотока нескольким потребителям.
//!
//! Потребители получают уже расшифрованные NAL-блоки — до декодирования.
//! Поэтому каждый декодирует сам: два потребителя стоят двух декодеров.
//! Общего декодированного кадра здесь нет намеренно: `Picture` внутри держит
//! кадр FFmpeg, он не `Clone`, и раскладывать его по ссылкам пришлось бы
//! через `Arc`, что меняет владение у всех выходов сразу.
//!
//! Вызовы идут по очереди в том же потоке, что и приём. Медленный
//! потребитель (например, [`crate::VideoSink`] поверх декодера с
//! блокирующей очередью) притормаживает остальных — это осознанный
//! backpressure: лучше не терять кадры, чем рассыпать картинку до
//! следующего ключевого.

use crate::{Codec, VideoSink};

/// Синхронная раздача одному или нескольким выходам.
pub struct FanOutSink {
    sinks: Vec<Box<dyn VideoSink>>,
}

impl FanOutSink {
    pub fn new(sinks: Vec<Box<dyn VideoSink>>) -> Self {
        Self { sinks }
    }

    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl VideoSink for FanOutSink {
    fn config(&mut self, codec: Codec, record: &[u8]) {
        for sink in &mut self.sinks {
            sink.config(codec, record);
        }
    }

    fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
        for sink in &mut self.sinks {
            sink.frame(pts_ns, nals);
        }
    }

    fn end(&mut self) {
        for sink in &mut self.sinks {
            sink.end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq)]
    enum Event {
        Config(Codec, Vec<u8>),
        Frame(u64, Vec<u8>),
        End,
    }

    struct Recorder(Arc<Mutex<Vec<Event>>>);

    impl VideoSink for Recorder {
        fn config(&mut self, codec: Codec, record: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .push(Event::Config(codec, record.to_vec()));
        }

        fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .push(Event::Frame(pts_ns, nals.to_vec()));
        }

        fn end(&mut self) {
            self.0.lock().unwrap().push(Event::End);
        }
    }

    #[test]
    fn every_consumer_sees_the_same_stream_in_order() {
        let a = Arc::new(Mutex::new(Vec::new()));
        let b = Arc::new(Mutex::new(Vec::new()));
        let mut sink = FanOutSink::new(vec![
            Box::new(Recorder(a.clone())),
            Box::new(Recorder(b.clone())),
        ]);
        assert_eq!(sink.len(), 2);

        sink.config(Codec::H265, b"hvcC");
        sink.frame(10, b"frame-1");
        sink.frame(20, b"frame-2");
        sink.end();

        let expected = vec![
            Event::Config(Codec::H265, b"hvcC".to_vec()),
            Event::Frame(10, b"frame-1".to_vec()),
            Event::Frame(20, b"frame-2".to_vec()),
            Event::End,
        ];
        assert_eq!(*a.lock().unwrap(), expected);
        assert_eq!(*b.lock().unwrap(), expected);
    }

    #[test]
    fn single_and_empty_fanout_do_not_panic() {
        let only = Arc::new(Mutex::new(Vec::new()));
        let mut sink = FanOutSink::new(vec![Box::new(Recorder(only.clone()))]);
        sink.frame(1, b"x");
        sink.end();
        assert_eq!(
            *only.lock().unwrap(),
            vec![Event::Frame(1, b"x".to_vec()), Event::End]
        );

        let mut empty = FanOutSink::new(Vec::new());
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        empty.config(Codec::H264, b"avcC");
        empty.frame(2, b"y");
        empty.end();
    }
}
