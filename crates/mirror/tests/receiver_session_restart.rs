//! Обрыв после нового config и повторное подключение (C80).
//!
//! Локальный TCP-мок, синтетические кадры. Проверяется, что новая сессия не
//! наследует счётчик кадров и состояние прежней: кадр с продолжающимся
//! счётчиком не проходит аутентификацию, а новая сессия начинается с нуля.

use std::sync::{Arc, Mutex};

use mirror::{Codec, HEADER_LEN, VideoSink, receive_authenticated};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Sink(Log);

impl VideoSink for Sink {
    fn config(&mut self, codec: Codec, _: &[u8]) {
        self.0 .0.lock().unwrap().push(format!("config {codec:?}"));
    }
    fn frame(&mut self, _: u64, bytes: &[u8]) {
        self.0
            .0
            .lock()
            .unwrap()
            .push(format!("frame {}", bytes.len()));
    }
    fn end(&mut self) {
        self.0 .0.lock().unwrap().push("end".into());
    }
}

async fn send_config(stream: &mut TcpStream) {
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&12u32.to_le_bytes());
    header[4] = 1; // Config
    header[8..16].copy_from_slice(&0u64.to_le_bytes());
    stream.write_all(&header).await.unwrap();
    stream.write_all(b"avc1config00").await.unwrap();
}

async fn send_frame(stream: &mut TcpStream, key: &[u8; 32], counter: u64, plain: &[u8]) {
    let mut header = [0u8; HEADER_LEN];
    // На проводе длина — шифротекст вместе с 16-байтовым тегом.
    header[..4].copy_from_slice(&((plain.len() + 16) as u32).to_le_bytes());
    header[4] = 0; // Frame
    header[8..16].copy_from_slice(&1_000_000_000u64.to_le_bytes());
    let body = crypto::aead::seal(key, &counter.to_le_bytes(), &header, plain);
    stream.write_all(&header).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

#[tokio::test]
async fn a_config_without_a_frame_closes_cleanly_and_the_sink_ends() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let log = Log::default();
    let sink_log = log.clone();
    let task = tokio::spawn(async move {
        receive_authenticated(listener, [3; 32], Box::new(Sink(sink_log))).await;
    });

    let mut client = TcpStream::connect(address).await.unwrap();
    send_config(&mut client).await;
    drop(client);
    task.await.unwrap();

    assert_eq!(log.events(), ["config H264", "end"]);
}

#[tokio::test]
async fn a_new_session_restarts_the_frame_counter() {
    let key = [3; 32];

    // Сессия 1: config и обрыв без кадра.
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_addr = first.local_addr().unwrap();
    let first_log = Log::default();
    let sink_log = first_log.clone();
    let task = tokio::spawn(async move {
        receive_authenticated(first, key, Box::new(Sink(sink_log))).await;
    });
    let mut client = TcpStream::connect(first_addr).await.unwrap();
    send_config(&mut client).await;
    drop(client);
    task.await.unwrap();
    assert_eq!(first_log.events(), ["config H264", "end"]);

    // Сессия 2: тот же ключ, но счётчик начинается с нуля.
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_addr = second.local_addr().unwrap();
    let second_log = Log::default();
    let sink_log = second_log.clone();
    let task = tokio::spawn(async move {
        receive_authenticated(second, key, Box::new(Sink(sink_log))).await;
    });
    let mut client = TcpStream::connect(second_addr).await.unwrap();
    send_config(&mut client).await;
    send_frame(&mut client, &key, 0, b"fresh").await;
    drop(client);
    task.await.unwrap();

    assert_eq!(
        second_log.events(),
        ["config H264", "frame 5", "end"],
        "новый сокет и новый счётчик, старая сессия не продолжается"
    );
    assert!(
        first_log.events().iter().all(|event| !event.starts_with("frame")),
        "в первой сессии кадров не было"
    );
}

#[tokio::test]
async fn a_frame_continuing_the_previous_counter_is_rejected_in_a_new_session() {
    let key = [4; 32];

    // Сессия 1 успевает проверить кадр со счётчиком 0.
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_addr = first.local_addr().unwrap();
    let log = Log::default();
    let sink_log = log.clone();
    let task = tokio::spawn(async move {
        receive_authenticated(first, key, Box::new(Sink(sink_log))).await;
    });
    let mut client = TcpStream::connect(first_addr).await.unwrap();
    send_frame(&mut client, &key, 0, b"one").await;
    drop(client);
    task.await.unwrap();
    assert_eq!(log.events(), ["frame 3", "end"]);

    // Сессия 2: кадр со счётчиком 1 (как будто сессия продолжалась) не проходит.
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_addr = second.local_addr().unwrap();
    let second_log = Log::default();
    let sink_log = second_log.clone();
    let task = tokio::spawn(async move {
        receive_authenticated(second, key, Box::new(Sink(sink_log))).await;
    });
    let mut client = TcpStream::connect(second_addr).await.unwrap();
    send_frame(&mut client, &key, 1, b"stale").await;
    drop(client);
    task.await.unwrap();

    assert_eq!(
        second_log.events(),
        ["end"],
        "старый ciphertext/счётчик не принимается новой сессией"
    );
}
