//! Buffered-аудио (type 103) как сессия: SETUP, data-порт, RECORD, FLUSH,
//! TEARDOWN по управляющему RTSP-соединению.
//!
//! [`BufferedLink`](super::buffered_link::BufferedLink) умеет только писать
//! кадры в уже открытый сокет. Здесь появляется последовательность запросов,
//! которую приёмник ждёт до и после этого: SETUP со стримом 103 (приёмник
//! отвечает `dataPort`), RECORD, затем FLUSH с `RTP-Info` и TEARDOWN.
//!
//! Границы, важные для достоверности:
//! - пароль/сопряжение не выдумываются: [`BufferedSession::open`] требует
//!   сохранённые [`PairingCredentials`] и делает pair-verify, как realtime
//!   `Session`; PIN-pairing и transient-режим здесь не реализованы;
//! - AAC не кодируется — на вход идут готовые кадры или ADTS;
//! - аппаратной проверки нет. Тесты гоняют последовательность запросов и
//!   доставку кадров по loopback против фейкового RTSP-приёмника (без
//!   шифрования канала), а не против Apple TV.

use std::io::Cursor;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use pair::{Identity as PairIdentity, hap::ControllerSetup};
use plist::Value;
use rand::RngCore;
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::buffered::{AAC_FRAMES_PER_PACKET, StreamSetup};
use super::buffered_link::{BufferedLink, StreamReply};
use super::session::{PairingCredentials, setup_proof, verify_pairing};
use super::transport::{Connection, Rtsp};

/// SSRC профиля AAC-LC 44,1 кГц стерео.
pub const AAC_LC_44100_SSRC: u32 = 0x1600_0000;

/// Параметры buffered-сессии. `control_port` — порт управляющего канала,
/// который открывает отправитель и объявляет в SETUP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferedConfig {
    pub endpoint: SocketAddr,
    pub control_port: u16,
    pub shk: [u8; 32],
    pub stream_connection_id: u64,
    pub ssrc: u32,
    pub seq: u16,
    pub timestamp: u32,
    pub request_timeout: Duration,
}

impl BufferedConfig {
    pub fn aac_44100(
        endpoint: SocketAddr,
        control_port: u16,
        shk: [u8; 32],
        stream_connection_id: u64,
    ) -> Self {
        Self {
            endpoint,
            control_port,
            shk,
            stream_connection_id,
            ssrc: AAC_LC_44100_SSRC,
            seq: 0,
            timestamp: 0,
            request_timeout: Duration::from_secs(8),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.endpoint.port() == 0 {
            bail!("порт приёмника должен быть ненулевым");
        }
        if self.control_port == 0 {
            bail!("controlPort должен быть ненулевым");
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(30) {
            bail!("срок RTSP-запроса должен быть положительным и не больше 30 с");
        }
        Ok(())
    }
}

/// Открытая buffered-сессия: управляющее соединение и канал данных.
pub struct BufferedSession {
    rtsp: Rtsp,
    link: BufferedLink,
    reply: StreamReply,
    setup: StreamSetup,
}

impl BufferedSession {
    /// Подключается по TCP, проходит pair-verify по сохранённым ключам и
    /// выполняет SETUP/RECORD. `uri` — как у realtime-сессии:
    /// `rtsp://<локальный ip>/<session id>`.
    pub async fn open(
        stream: TcpStream,
        uri: String,
        credentials: &PairingCredentials,
        config: BufferedConfig,
    ) -> Result<Self> {
        config.validate()?;
        stream.set_nodelay(true)?;
        let mut rng = rand::thread_rng();
        let dacp = format!("{:016X}", rng.next_u64());
        let active_remote = rng.next_u32();
        let mut rtsp = Rtsp::new(
            Connection::new(stream),
            uri,
            dacp,
            active_remote,
            config.request_timeout,
        );
        let (keys, _shared) = verify_pairing(&mut rtsp, credentials).await?;
        rtsp.conn.encrypt(keys);
        Self::start(rtsp, config).await
    }

    /// То же, но с transient-сопряжением по фиксированному PIN `3939` — так
    /// же, как realtime `Session` без сохранённых ключей. Ничего не
    /// сохраняет: ключи живут только в этом соединении.
    pub async fn pair_transient(
        stream: TcpStream,
        uri: String,
        device_id: &str,
        config: BufferedConfig,
    ) -> Result<Self> {
        config.validate()?;
        stream.set_nodelay(true)?;
        let mut rng = rand::thread_rng();
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        let mut rtsp = Rtsp::new(
            Connection::new(stream),
            uri,
            format!("{:016X}", rng.next_u64()),
            rng.next_u32(),
            config.request_timeout,
        );
        rtsp.pair_post("/pair-pin-start", Vec::new()).await?;
        let pair = ControllerSetup::new(
            PairIdentity::from_seed(device_id.to_owned(), &seed),
            "3939",
            true,
        );
        let m2 = rtsp.pair_post("/pair-setup", pair.m1()).await?;
        let (pair, m3) = timeout(config.request_timeout, setup_proof(pair, m2.body))
            .await
            .context("срок доказательства сопряжения")??;
        let m4 = rtsp.pair_post("/pair-setup", m3).await?;
        let keys = pair
            .check_m4(&m4.body)?
            .ok_or_else(|| anyhow!("transient-сопряжение не выдало ключи канала"))?;
        rtsp.conn.encrypt(keys);
        Self::start(rtsp, config).await
    }

    /// SETUP → dataPort → RECORD на уже готовом (и, если нужно,
    /// зашифрованном) управляющем соединении.
    pub(crate) async fn start(mut rtsp: Rtsp, config: BufferedConfig) -> Result<Self> {
        config.validate()?;
        let setup = StreamSetup {
            control_port: config.control_port,
            shk: config.shk,
            stream_connection_id: config.stream_connection_id,
            sample_rate: 44_100,
            frames_per_packet: AAC_FRAMES_PER_PACKET,
        };
        let body = binary_plist(Value::Dictionary(setup.setup_plist()))?;
        let response = rtsp
            .request(
                "SETUP",
                None,
                body,
                Some("application/x-apple-binary-plist"),
                &[],
            )
            .await
            .context("SETUP buffered-потока")?;
        let value = Value::from_reader(Cursor::new(response.body))
            .context("ответ SETUP не является plist")?;
        let reply = StreamReply::from_setup_reply(
            value.as_dictionary().context("ответ SETUP не словарь")?,
        )?;
        reply.require_buffered()?;
        let addr = SocketAddr::new(config.endpoint.ip(), reply.data_port);
        let link = BufferedLink::connect(
            addr,
            &setup,
            config.ssrc,
            config.seq,
            config.timestamp,
            config.request_timeout,
        )
        .await?;
        rtsp.request("RECORD", None, Vec::new(), None, &[])
            .await
            .context("RECORD buffered-потока")?;
        Ok(Self {
            rtsp,
            link,
            reply,
            setup,
        })
    }

    /// Ответ SETUP: порты и глубина буфера, которые назвал приёмник.
    pub fn reply(&self) -> StreamReply {
        self.reply
    }

    pub fn frames_sent(&self) -> u64 {
        self.link.frames_sent()
    }

    pub fn next_sequence(&self) -> u16 {
        self.link.next_sequence()
    }

    pub fn next_timestamp(&self) -> u32 {
        self.link.next_timestamp()
    }

    pub async fn send_frame(&mut self, frame: &[u8]) -> Result<()> {
        self.link.send_frame(frame).await
    }

    pub async fn send_frames(&mut self, frames: &[&[u8]]) -> Result<usize> {
        self.link.send_frames(frames).await
    }

    /// Отправляет ADTS-поток (уже закодированный AAC-LC).
    pub async fn send_adts(&mut self, data: &[u8]) -> Result<usize> {
        self.link.send_adts(data).await
    }

    /// FLUSH с `RTP-Info` о следующем пакете, закрытие канала данных и
    /// TEARDOWN. Возвращает число отправленных кадров.
    pub async fn finish(mut self) -> Result<u64> {
        let frames = self.link.frames_sent();
        let info = format!(
            "seq={};rtptime={}",
            self.link.next_sequence(),
            self.link.next_timestamp()
        );
        let result = self
            .rtsp
            .request(
                "FLUSH",
                None,
                Vec::new(),
                None,
                &[
                    ("Range", "npt=0-".into()),
                    ("Session", self.setup.stream_connection_id.to_string()),
                    ("RTP-Info", info),
                ],
            )
            .await
            .context("FLUSH buffered-потока");
        self.link.close().await?;
        result?;
        self.rtsp
            .request("TEARDOWN", None, Vec::new(), None, &[])
            .await
            .context("TEARDOWN buffered-потока")?;
        Ok(frames)
    }
}

fn binary_plist(value: Value) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    value.to_writer_binary(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtp::AudioCipher;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn reply_body(data_port: u16) -> Vec<u8> {
        let mut stream = plist::Dictionary::new();
        stream.insert("type".into(), Value::Integer(103u64.into()));
        stream.insert("dataPort".into(), Value::Integer((data_port as u64).into()));
        stream.insert("controlPort".into(), Value::Integer(7002u64.into()));
        stream.insert("audioBufferSize".into(), Value::Integer(16384u64.into()));
        let mut body = plist::Dictionary::new();
        body.insert(
            "streams".into(),
            Value::Array(vec![Value::Dictionary(stream)]),
        );
        binary_plist(Value::Dictionary(body)).unwrap()
    }

    fn ok_response(cseq: &str, body: &[u8]) -> Vec<u8> {
        let mut wire = format!(
            "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        wire.extend_from_slice(body);
        wire
    }

    /// Фейковый приёмник: отвечает на SETUP/RECORD/FLUSH/TEARDOWN и
    /// собирает кадры с data-порта.
    async fn fake_receiver(
        control: TcpListener,
        data: TcpListener,
    ) -> (
        tokio::task::JoinHandle<Vec<String>>,
        tokio::task::JoinHandle<Vec<u8>>,
        u16,
    ) {
        let data_port = data.local_addr().unwrap().port();
        let frames = tokio::spawn(async move {
            let (mut socket, _) = data.accept().await.unwrap();
            let mut wire = Vec::new();
            socket.read_to_end(&mut wire).await.unwrap();
            wire
        });
        let requests = tokio::spawn(async move {
            let (mut socket, _) = control.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut methods = Vec::new();
            let mut chunk = [0u8; 2048];
            while methods.len() < 4 {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                while let Some(request) = rtsp::decode_request(&mut buf).unwrap() {
                    let cseq = request
                        .headers
                        .get("CSeq")
                        .map(str::to_owned)
                        .unwrap_or_default();
                    methods.push(format!(
                        "{} {}",
                        request.method,
                        if request.method == "FLUSH" {
                            request
                                .headers
                                .get("RTP-Info")
                                .map(str::to_owned)
                                .unwrap_or_default()
                        } else {
                            String::new()
                        }
                    ));
                    let body = if request.method == "SETUP" {
                        reply_body(data_port)
                    } else {
                        Vec::new()
                    };
                    socket.write_all(&ok_response(&cseq, &body)).await.unwrap();
                }
            }
            methods
        });
        (requests, frames, data_port)
    }

    #[tokio::test]
    async fn session_runs_setup_record_flush_teardown() {
        let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let control_addr = control.local_addr().unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (requests, frames, data_port) = fake_receiver(control, data).await;

        let stream = TcpStream::connect(control_addr).await.unwrap();
        let mut config = BufferedConfig::aac_44100(control_addr, 7001, [3; 32], 0x0102_0304);
        config.seq = 100;
        config.timestamp = 2000;
        let rtsp = Rtsp::new(
            Connection::new(stream),
            "rtsp://127.0.0.1/1".into(),
            "DACP".into(),
            1,
            Duration::from_secs(2),
        );
        let mut session = BufferedSession::start(rtsp, config).await.unwrap();
        assert_eq!(session.reply().data_port, data_port);
        assert_eq!(session.reply().audio_buffer_size, Some(16384));
        session.send_frames(&[b"one", b"two"]).await.unwrap();
        assert_eq!(session.frames_sent(), 2);
        assert_eq!(session.finish().await.unwrap(), 2);

        let methods = requests.await.unwrap();
        assert_eq!(methods[0], "SETUP ");
        assert_eq!(methods[1], "RECORD ");
        assert_eq!(methods[2], "FLUSH seq=102;rtptime=4048");
        assert_eq!(methods[3], "TEARDOWN ");

        let wire = frames.await.unwrap();
        let cipher = AudioCipher::new(&[3; 32]);
        let mut buf = wire;
        let mut seen = Vec::new();
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            let (header, payload) = cipher.open(&mut packet).unwrap();
            seen.push((header.seq, payload.to_vec()));
        }
        assert_eq!(seen, vec![(100, b"one".to_vec()), (101, b"two".to_vec())]);
    }

    #[tokio::test]
    async fn wrong_stream_type_is_refused_before_record() {
        let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let control_addr = control.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = control.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            let _ = socket.read(&mut buf).await.unwrap();
            let mut stream = plist::Dictionary::new();
            stream.insert("type".into(), Value::Integer(96u64.into()));
            stream.insert("dataPort".into(), Value::Integer(7100u64.into()));
            let mut body = plist::Dictionary::new();
            body.insert(
                "streams".into(),
                Value::Array(vec![Value::Dictionary(stream)]),
            );
            let body = binary_plist(Value::Dictionary(body)).unwrap();
            socket.write_all(&ok_response("0", &body)).await.unwrap();
        });
        let stream = TcpStream::connect(control_addr).await.unwrap();
        let config = BufferedConfig::aac_44100(control_addr, 7001, [1; 32], 1);
        let rtsp = Rtsp::new(
            Connection::new(stream),
            "rtsp://127.0.0.1/1".into(),
            "DACP".into(),
            1,
            Duration::from_secs(2),
        );
        let error = match BufferedSession::start(rtsp, config).await {
            Ok(_) => panic!("приёмник подтвердил не buffered-поток"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("103"), "{error}");
        server.await.unwrap();
    }

    #[test]
    fn config_rejects_zero_ports_and_long_timeouts() {
        let mut config =
            BufferedConfig::aac_44100("127.0.0.1:7000".parse().unwrap(), 7001, [0; 32], 1);
        config.validate().unwrap();
        config.control_port = 0;
        assert!(config.validate().is_err());
        config.control_port = 7001;
        config.endpoint = "127.0.0.1:0".parse().unwrap();
        assert!(config.validate().is_err());
        config.endpoint = "127.0.0.1:7000".parse().unwrap();
        config.request_timeout = Duration::from_secs(31);
        assert!(config.validate().is_err());
    }
}
