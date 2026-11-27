//! Источник PCM: EOF на границе кадра, отмена, закрытие получателей (C49).
//!
//! Тесты работают через `media::tx::pcm_source::produce` и управляемые
//! `AsyncRead` (срезы и `duplex`), без звуковой карты и файлов пользователя.

use std::sync::Arc;
use std::time::Duration;

use media::tx::pcm_source::produce;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

fn channel() -> (
    broadcast::Sender<Arc<media::tx::AudioPacket>>,
    broadcast::Receiver<Arc<media::tx::AudioPacket>>,
) {
    broadcast::channel(8)
}

#[tokio::test]
async fn a_complete_final_frame_is_emitted_then_eof() {
    let (tx, mut rx) = channel();
    // Один полный S16BE стереокадр (4 байта) и конец потока.
    let frames = produce(&b"\x00\x01\x00\x02"[..], tx).await.unwrap();
    assert!(frames > 0);
    let packet = rx.try_recv().expect("пакет последнего кадра");
    assert_eq!(packet.frame_index(), 0);
    assert!(!packet.pcm().is_empty());
    // Больше ничего не приходит: EOF на границе кадра не рождает пустой пакет.
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn eof_in_the_middle_of_a_sample_is_an_error() {
    let (tx, _rx) = channel();
    let err = produce(&b"\x00\x01\x02"[..], tx).await.unwrap_err();
    assert!(
        err.to_string().contains("посреди"),
        "ожидалась ошибка границы семпла: {err}"
    );
}

#[tokio::test]
async fn producing_without_consumers_fails_instead_of_buffering_forever() {
    let (tx, rx) = channel();
    drop(rx);
    let err = produce(&b"\x00\x01\x00\x02"[..], tx).await.unwrap_err();
    assert!(err.to_string().contains("получател"), "{err}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_blocking_read_holds_output_and_cancellation_leaves_no_task() {
    let (tx, mut rx) = channel();
    let (mut input, reader) = tokio::io::duplex(16);
    let task = tokio::spawn(produce(reader, tx));

    // Пока данных нет, пакетов нет.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err()
    );

    input.write_all(&[1, 2, 3, 4]).await.unwrap();
    input.shutdown().await.unwrap();
    let _packet = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("пакет после первых данных")
        .unwrap();

    // Штатное завершение после EOF.
    input.shutdown().await.unwrap();
    assert!(task.await.unwrap().is_ok());

    // Отмена задачи на блокирующем чтении: ни паники, ни висящей задачи.
    let (tx2, _rx2) = channel();
    let (mut idle, reader) = tokio::io::duplex(16);
    let blocked = tokio::spawn(produce(reader, tx2));
    tokio::time::sleep(Duration::from_millis(30)).await;
    blocked.abort();
    assert!(blocked.await.is_err(), "задача отменена, а не упала");
    drop(idle);
}

#[tokio::test]
async fn closing_all_consumers_stops_the_source_with_an_error() {
    let (tx, rx) = channel();
    let (mut input, reader) = tokio::io::duplex(16);
    let task = tokio::spawn(produce(reader, tx));
    input.write_all(&[9, 9, 9, 9]).await.unwrap();
    input.shutdown().await.unwrap();
    // Закрываем единственного получателя до отправки.
    drop(rx);
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("источник обязан завершиться")
        .unwrap();
    assert!(result.is_err(), "без получателей источник не продолжает работу");
    drop(input);
}
