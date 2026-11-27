//! Границы смены трека и асинхронные ошибки источника (C51).
//!
//! Источник — синтетический `AsyncRead`: нулевые сэмплы, без декодера, файлов и
//! устройств. Проверяется поколение кадров, которые попадают в очередь, а не
//! звучание: реальный приёмник не участвует.

use std::time::Duration;

use media::tx::continuous_source::{bounded_reader, End, State};
use media::tx::{FRAME_BYTES, PACKET_BYTES};
use tokio::sync::watch;

fn controls(generation: u64) -> watch::Sender<State> {
    let (tx, _rx) = watch::channel(State {
        generation,
        paused: false,
    });
    tx
}

/// Канал остановки: отправитель обязан жить, иначе источник считает остановку.
fn stop() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

#[tokio::test]
async fn frames_carry_the_generation_that_was_current_at_acquisition() {
    let control = controls(0);
    let _stop = stop();
    let (mut frames, acquisition) =
        bounded_reader(tokio::io::repeat(7), 4, control.subscribe(), _stop.1).unwrap();
    let source = tokio::spawn(acquisition);

    let first = tokio::time::timeout(Duration::from_secs(2), frames.recv())
        .await
        .expect("первый кадр")
        .unwrap();
    assert_eq!(first.generation, 0);

    control
        .send(State {
            generation: 1,
            paused: false,
        })
        .unwrap();

    let mut seen_new = false;
    for _ in 0..4 {
        let frame = tokio::time::timeout(Duration::from_secs(2), frames.recv())
            .await
            .expect("кадр после смены")
            .unwrap();
        assert!(frame.generation <= 1);
        if frame.generation == 1 {
            seen_new = true;
        } else {
            assert!(!seen_new, "старое поколение не возвращается после нового");
        }
    }
    source.abort();
}

#[tokio::test]
async fn a_paused_source_stops_producing_without_hanging() {
    let control = controls(0);
    let _stop = stop();
    let (mut frames, acquisition) =
        bounded_reader(tokio::io::repeat(0), 2, control.subscribe(), _stop.1).unwrap();
    let source = tokio::spawn(acquisition);

    control
        .send(State {
            generation: 0,
            paused: true,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(frames.try_recv().is_err(), "на паузе кадры не выдаются");

    // Снятие паузы возвращает поток без перезапуска задачи.
    control
        .send(State {
            generation: 1,
            paused: false,
        })
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), frames.recv())
        .await
        .expect("кадр после снятия паузы")
        .unwrap();
    assert_eq!(frame.generation, 1);
    source.abort();
}

#[tokio::test]
async fn a_frame_aligned_tail_is_completed_and_reported_as_eof() {
    let data = vec![0u8; PACKET_BYTES + FRAME_BYTES];
    let control = controls(0);
    let _stop = stop();
    let (mut frames, acquisition) =
        bounded_reader(std::io::Cursor::new(data), 4, control.subscribe(), _stop.1).unwrap();
    let end = acquisition.await.unwrap();
    assert_eq!(end, End::Eof);
    let mut count = 0;
    while frames.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 2, "хвост дополнен до пакета, а не потерян");
}

#[tokio::test]
async fn a_tail_that_splits_a_stereo_frame_is_an_error() {
    let data = vec![0u8; PACKET_BYTES + 1];
    let control = controls(0);
    let _stop = stop();
    let (mut frames, acquisition) =
        bounded_reader(std::io::Cursor::new(data), 4, control.subscribe(), _stop.1).unwrap();
    let error = acquisition.await.unwrap_err().to_string();
    assert!(error.contains("aligned"), "ошибка выравнивания: {error}");
    assert!(frames.try_recv().is_ok(), "первый пакет уже отдан");
}

#[tokio::test]
async fn an_explicit_stop_ends_the_source_without_draining() {
    let (stop_tx, stop_rx) = watch::channel(false);
    let data = vec![0u8; PACKET_BYTES * 8];
    let (frames, acquisition) = bounded_reader(
        std::io::Cursor::new(data),
        1,
        controls(0).subscribe(),
        stop_rx,
    )
    .unwrap();
    stop_tx.send(true).unwrap();
    assert_eq!(acquisition.await.unwrap(), End::Stopped);
    drop(frames);
}

#[tokio::test]
async fn queue_capacity_is_bounded() {
    let control = controls(0);
    let _stop = stop();
    for capacity in [0usize, 513] {
        let result = bounded_reader(
            tokio::io::repeat(0),
            capacity,
            control.subscribe(),
            _stop.1.clone(),
        );
        assert!(result.is_err(), "ёмкость {capacity} должна отклоняться");
    }
    assert!(bounded_reader(tokio::io::repeat(0), 512, control.subscribe(), _stop.1).is_ok());
}
