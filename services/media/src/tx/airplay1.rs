//! AirPlay 1 (RAOP) sender profile.
//!
//! This is a deliberately small RAOP path: `_raop._tcp` advertisement parsing,
//! PCM/ALAC negotiation, plain RTSP announce/setup/record, UDP RTP audio,
//! volume and teardown. It does not share the AirPlay 2 session/PTP engine.
//!
//! AES-CBC payload encryption is supported when the caller supplies the 16-byte
//! key and IV. RSA transport of that key (`rsaaeskey`) and Apple-Challenge are
//! not implemented, so a stock receiver that requires them needs that exchange
//! from the caller or a future change. No hardware compatibility is claimed.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crypto::cbc::AudioCbc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

use super::{FRAMES_PER_PACKET, PACKET_BYTES, Volume};

const RTP_PAYLOAD: u8 = 96;
const RTSP_TIMEOUT: Duration = Duration::from_secs(5);

/// Advertised RAOP `cn` codecs relevant here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// 16-bit big-endian PCM (`cn` 0).
    Pcm,
    /// Apple Lossless, encoded as escape (uncompressed) ALAC frames (`cn` 1).
    Alac,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub name: String,
    pub address: SocketAddr,
    pub pcm: bool,
    pub alac: bool,
    /// `et` includes AES encryption (bit 1/4); informational for the caller.
    pub encryption: bool,
}

impl Receiver {
    /// `_raop._tcp` instance is normally `AABBCCDDEEFF@Name`.
    pub fn from_raop(
        instance: &str,
        address: SocketAddr,
        txt: &BTreeMap<String, String>,
    ) -> Result<Self> {
        if instance.is_empty() || instance.len() > 256 || instance.chars().any(char::is_control) {
            bail!("invalid RAOP instance name");
        }
        if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
            bail!("invalid RAOP address");
        }
        let codecs = txt.get("cn").map(String::as_str).unwrap_or("0,1");
        let parsed: Vec<u8> = codecs
            .split(',')
            .filter_map(|word| word.trim().parse().ok())
            .collect();
        if parsed.is_empty() || parsed.iter().any(|codec| *codec > 4) {
            bail!("invalid RAOP codec list");
        }
        // `et=0,1` is cleartext plus AES; `et=4` is RSA key exchange.
        let encryption = txt
            .get("et")
            .is_some_and(|et| et.split(',').any(|v| v.trim() == "1" || v.trim() == "4"));
        let name = instance
            .split_once('@')
            .map(|(_, name)| name)
            .unwrap_or(instance)
            .to_owned();
        Ok(Self {
            name,
            address,
            pcm: parsed.contains(&0),
            alac: parsed.contains(&1),
            encryption,
        })
    }

    /// ALAC is preferred when both sides support it; PCM is the fallback.
    pub fn negotiate(&self, prefer_alac: bool) -> Result<Codec> {
        if prefer_alac && self.alac {
            Ok(Codec::Alac)
        } else if self.pcm {
            Ok(Codec::Pcm)
        } else if self.alac {
            Ok(Codec::Alac)
        } else {
            bail!("receiver advertises neither PCM nor ALAC")
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub receiver: Receiver,
    pub codec: Codec,
    pub volume: Volume,
    /// Optional AES-128-CBC key/IV for the RTP payload.
    pub audio_key: Option<[u8; 16]>,
    pub audio_iv: Option<[u8; 16]>,
    pub request_timeout: Duration,
}

impl SessionConfig {
    pub fn new(receiver: Receiver, codec: Codec) -> Self {
        Self {
            receiver,
            codec,
            volume: Volume::db(-20.0).expect("constant"),
            audio_key: None,
            audio_iv: None,
            request_timeout: RTSP_TIMEOUT,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.codec == Codec::Alac && !self.receiver.alac {
            bail!("receiver does not advertise ALAC");
        }
        if self.codec == Codec::Pcm && !self.receiver.pcm {
            bail!("receiver does not advertise PCM");
        }
        if self.audio_key.is_some() != self.audio_iv.is_some() {
            bail!("audio key and IV must be supplied together");
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(60) {
            bail!("invalid RTSP timeout");
        }
        if !self.volume.as_db().is_finite()
            || !(self.volume.as_db() == -144.0 || (-30.0..=0.0).contains(&self.volume.as_db()))
        {
            bail!("invalid volume");
        }
        Ok(())
    }
}

/// One connected AirPlay 1 audio session. The caller supplies packets already
/// in the stream clock; this module owns RAOP framing and the RTSP lifecycle.
pub struct Session {
    rtsp: TcpStream,
    audio: UdpSocket,
    cseq: u32,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    cipher: Option<AudioCbc>,
    config: SessionConfig,
    audio_target: SocketAddr,
    recorded: bool,
}

impl Session {
    pub async fn connect(config: SessionConfig) -> Result<Self> {
        config.validate()?;
        let stream = timeout(
            config.request_timeout,
            TcpStream::connect(config.receiver.address),
        )
        .await
        .context("RAOP connect deadline")??;
        stream.set_nodelay(true)?;
        let local = stream.local_addr()?.ip();
        let audio = UdpSocket::bind((local, 0)).await?;
        let audio_target = config.receiver.address;
        let mut session = Self {
            rtsp: stream,
            audio,
            cseq: 1,
            sequence: rand::random(),
            timestamp: rand::random(),
            ssrc: rand::random(),
            cipher: match (config.audio_key, config.audio_iv) {
                (Some(key), Some(iv)) => Some(AudioCbc::new(&key, &iv)),
                _ => None,
            },
            config,
            audio_target,
            recorded: false,
        };
        let uri = format!(
            "rtsp://{}/{}",
            session.config.receiver.address, session.ssrc
        );
        // OPTIONS proves the peer is RAOP and exposes methods without changing state.
        session.request("OPTIONS", "*", &[], &[]).await?;
        session
            .request(
                "ANNOUNCE",
                &uri,
                &[("Content-Type", "application/sdp")],
                session.sdp(&uri).as_bytes(),
            )
            .await?;
        let transport = format!(
            "RTP/AVP/UDP;unicast;mode=record;control_port={};timing_port={}",
            session.audio.local_addr()?.port(),
            session.audio.local_addr()?.port()
        );
        let setup = session
            .request("SETUP", &uri, &[("Transport", &transport)], &[])
            .await?;
        let server_port = setup
            .headers
            .get("Transport")
            .and_then(|value| {
                value
                    .split(';')
                    .find_map(|part| part.strip_prefix("server_port="))
            })
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .context("RAOP SETUP response has no valid server_port")?;
        session.audio_target = SocketAddr::new(session.config.receiver.address.ip(), server_port);
        session.request("RECORD", &uri, &[], &[]).await?;
        session.recorded = true;
        Ok(session)
    }

    pub fn codec(&self) -> Codec {
        self.config.codec
    }

    fn sdp(&self, uri: &str) -> String {
        let fmtp = match self.config.codec {
            Codec::Alac => {
                "a=rtpmap:96 AppleLossless\r\na=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100\r\n"
            }
            Codec::Pcm => "a=rtpmap:96 L16/44100/2\r\n",
        };
        format!(
            "v=0\r\n\
             o=iTunes {uri} 0 IN IP4 127.0.0.1\r\n\
             s=iTunes\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=audio 0 RTP/AVP 96\r\n\
             {fmtp}"
        )
    }

    async fn request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<rtsp::Response> {
        let mut req = rtsp::Request::new(method, uri, "RTSP/1.0");
        req.headers.insert("CSeq", self.cseq.to_string());
        req.headers.insert("User-Agent", "airplay-raop/1");
        for (name, value) in headers {
            req.headers.insert(*name, *value);
        }
        req.body = body.to_vec();
        let mut wire = Vec::new();
        req.encode(&mut wire);
        let deadline = tokio::time::Instant::now() + self.config.request_timeout;
        tokio::time::timeout_at(deadline, self.rtsp.write_all(&wire))
            .await
            .context("RAOP request deadline")??;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let response = loop {
            if let Some(response) = rtsp::decode_response(&mut buffer)
                .map_err(|e| anyhow::anyhow!("RAOP response: {e}"))?
            {
                break response;
            }
            let n = tokio::time::timeout_at(deadline, self.rtsp.read(&mut chunk))
                .await
                .context("RAOP response deadline")??;
            if n == 0 {
                bail!("RAOP connection closed during {method}");
            }
            buffer.extend_from_slice(&chunk[..n]);
        };
        self.cseq += 1;
        if response.status != 200 {
            bail!("RAOP {method} rejected with {}", response.status);
        }
        Ok(response)
    }

    /// Send one 352-frame packet. `pcm` is interleaved 16-bit big-endian.
    pub async fn send(&mut self, pcm: &[u8]) -> Result<()> {
        if !self.recorded {
            bail!("RAOP session is not recording");
        }
        if pcm.len() != PACKET_BYTES {
            bail!("RAOP packet must be {PACKET_BYTES} bytes");
        }
        let payload = match self.config.codec {
            Codec::Pcm => pcm.to_vec(),
            Codec::Alac => alac_escape_frame(pcm)?,
        };
        let mut payload = payload;
        if let Some(cipher) = &self.cipher {
            cipher.encrypt(&mut payload);
        }
        let mut packet = Vec::with_capacity(12 + payload.len());
        packet.extend_from_slice(&[0x80, RTP_PAYLOAD]);
        packet.extend_from_slice(&self.sequence.to_be_bytes());
        packet.extend_from_slice(&self.timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());
        packet.extend_from_slice(&payload);
        self.audio.send_to(&packet, self.audio_target).await?;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(FRAMES_PER_PACKET as u32);
        Ok(())
    }

    pub async fn set_volume(&mut self, volume: Volume) -> Result<()> {
        let uri = format!("rtsp://{}/{}", self.config.receiver.address, self.ssrc);
        let body = format!("volume: {:.6}\r\n", volume.as_db());
        self.request(
            "SET_PARAMETER",
            &uri,
            &[("Content-Type", "text/parameters")],
            body.as_bytes(),
        )
        .await?;
        self.config.volume = volume;
        Ok(())
    }

    /// Explicit teardown; dropping the socket also ends the session.
    pub async fn finish(mut self) -> Result<()> {
        let uri = format!("rtsp://{}/{}", self.config.receiver.address, self.ssrc);
        let result = self.request("TEARDOWN", &uri, &[], &[]).await;
        self.recorded = false;
        let _ = self.rtsp.shutdown().await;
        result.map(|_| ())
    }
}

/// ALAC escape frame: 1 CPE (stereo), 0 unknown fields, uncompressed flag,
/// then raw 16-bit samples and the END element. Mirrors the decoder's accepted
/// layout and needs no lossless compressor.
fn alac_escape_frame(pcm: &[u8]) -> Result<Vec<u8>> {
    if pcm.len() != FRAMES_PER_PACKET * 4 {
        bail!("ALAC packet must contain {FRAMES_PER_PACKET} stereo frames");
    }
    let mut bits = BitWriter::default();
    bits.put(1, 3); // channel element: stereo pair
    bits.put(0, 4);
    bits.put(0, 12);
    bits.put(0, 1); // standard frame size
    bits.put(0, 2);
    bits.put(1, 1); // escape / not compressed
    for sample in pcm.chunks_exact(2) {
        bits.put(u16::from_be_bytes([sample[0], sample[1]]) as u32, 16);
    }
    bits.put(7, 3); // END
    Ok(bits.finish())
}

/// MSB-first bit writer, local to the uncompressed ALAC frame above.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    current: u8,
    used: u8,
}

impl BitWriter {
    fn put(&mut self, value: u32, bits: u8) {
        for shift in (0..bits).rev() {
            self.current = (self.current << 1) | ((value >> shift) & 1) as u8;
            self.used += 1;
            if self.used == 8 {
                self.bytes.push(self.current);
                self.current = 0;
                self.used = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used != 0 {
            self.bytes.push(self.current << (8 - self.used));
        }
        self.bytes
    }
}

/// Discovery adapter for one `_raop._tcp` service. The caller supplies the
/// reachable address (including IPv6 scope) and remains responsible for browse.
pub fn receiver_from_service(
    service: &mdns::discovery::ResolvedService,
    address: SocketAddr,
) -> Result<Receiver> {
    if !service.ty_domain.eq_ignore_ascii_case(mdns::RAOP) {
        bail!("not a RAOP service");
    }
    let mut txt = BTreeMap::new();
    for key in [
        "cn", "et", "am", "vs", "sf", "md", "ft", "tp", "vn", "vv", "da",
    ] {
        if let Some(value) = service.get_property_val_str(key) {
            txt.insert(key.to_owned(), value.to_string());
        }
    }
    Receiver::from_raop(&service.fullname, address, &txt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn raop(cn: &str, et: &str) -> Receiver {
        Receiver::from_raop(
            "AABBCCDDEEFF@Kitchen",
            (Ipv4Addr::LOCALHOST, 5000).into(),
            &BTreeMap::from([
                ("cn".into(), cn.into()),
                ("et".into(), et.into()),
                ("am".into(), "TestSpeaker".into()),
            ]),
        )
        .unwrap()
    }

    #[test]
    fn raop_announcement_selects_advertised_codecs() {
        let both = raop("0,1", "0,1");
        assert_eq!(both.name, "Kitchen");
        assert_eq!(both.negotiate(true).unwrap(), Codec::Alac);
        assert_eq!(both.negotiate(false).unwrap(), Codec::Pcm);
        assert!(both.encryption);
        let alac = raop("1", "0");
        assert_eq!(alac.negotiate(true).unwrap(), Codec::Alac);
        assert_eq!(
            alac.negotiate(false).unwrap(),
            Codec::Alac,
            "ALAC is the only advertised codec"
        );
        let pcm = raop("0", "0");
        assert_eq!(pcm.negotiate(true).unwrap(), Codec::Pcm);
        assert!(raop("2", "0").negotiate(true).is_err());
        assert!(
            Receiver::from_raop(
                "x",
                (Ipv4Addr::LOCALHOST, 5000).into(),
                &BTreeMap::from([("cn".into(), "".into())])
            )
            .is_err()
        );
        assert!(
            Receiver::from_raop(
                "x",
                (Ipv4Addr::LOCALHOST, 5000).into(),
                &BTreeMap::from([("cn".into(), "0,9".into())])
            )
            .is_err()
        );
    }

    #[test]
    fn session_config_does_not_claim_an_unadvertised_codec() {
        let pcm = raop("0", "0");
        assert!(
            SessionConfig::new(pcm.clone(), Codec::Alac)
                .validate()
                .is_err()
        );
        assert!(SessionConfig::new(pcm, Codec::Pcm).validate().is_ok());
        let mut config = SessionConfig::new(raop("0,1", "0"), Codec::Alac);
        config.audio_key = Some([1; 16]);
        assert!(config.validate().is_err(), "key without IV");
        config.audio_iv = Some([2; 16]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn escape_alac_frame_starts_with_escape_header_and_raw_samples() {
        let mut pcm = vec![0u8; PACKET_BYTES];
        for (i, frame) in pcm.chunks_exact_mut(4).enumerate() {
            let left = (i as i16).to_be_bytes();
            let right = (-(i as i16)).to_be_bytes();
            frame.copy_from_slice(&[left[0], left[1], right[0], right[1]]);
        }
        let encoded = alac_escape_frame(&pcm).unwrap();
        // 3+4+12+1+2+1 = 23 header bits, then samples.
        assert_eq!(encoded[0], 0b0010_0000, "channel element starts the header");
        assert_eq!(encoded[1], 0);
        assert_eq!(
            encoded[2] & 0b0000_0010,
            0b0000_0010,
            "escape bit is the 23rd bit"
        );
        assert!(encoded.len() > PACKET_BYTES);
        assert!(encoded.len() < PACKET_BYTES + 16);
    }

    #[tokio::test]
    async fn fake_receiver_sees_raop_sequence_volume_and_audio() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let audio = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let audio_address = audio.local_addr().unwrap();
        let key = [7u8; 16];
        let iv = [9u8; 16];

        let fake = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut methods = Vec::new();
            let mut volume = None;
            let mut sdp = None;
            let mut buffer = Vec::new();
            for _ in 0..5 {
                let req = loop {
                    if let Some(req) = rtsp::decode_request(&mut buffer).unwrap() {
                        break req;
                    }
                    let mut chunk = [0u8; 2048];
                    let n = tcp.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    buffer.extend_from_slice(&chunk[..n]);
                };
                methods.push(req.method.clone());
                if req.method == "ANNOUNCE" {
                    sdp = Some(String::from_utf8(req.body.clone()).unwrap());
                }
                if req.method == "SET_PARAMETER" {
                    volume = Some(String::from_utf8(req.body.clone()).unwrap());
                }
                let mut response = rtsp::Response::to(&req, 200);
                if req.method == "SETUP" {
                    response.headers.insert(
                        "Transport",
                        format!(
                            "RTP/AVP/UDP;unicast;server_port={};control_port={};timing_port={}",
                            audio_address.port(),
                            audio_address.port(),
                            audio_address.port()
                        ),
                    );
                }
                let mut wire = Vec::new();
                response.encode(&mut wire);
                tcp.write_all(&wire).await.unwrap();
            }
            // Two audio packets, decrypted with the caller-supplied key/IV.
            let audio = audio.into_std().unwrap();
            audio.set_nonblocking(false).unwrap();
            audio
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut packets = Vec::new();
            let mut first_sequence = None;
            for index in 0..2u16 {
                let mut buf = [0u8; 2048];
                let (n, _) = audio.recv_from(&mut buf).unwrap();
                let packet = &buf[..n];
                assert_eq!(packet[0], 0x80);
                assert_eq!(packet[1], 96);
                let sequence = u16::from_be_bytes(packet[2..4].try_into().unwrap());
                let expected = *first_sequence.get_or_insert(sequence);
                assert_eq!(sequence, expected.wrapping_add(index));
                let mut payload = packet[12..].to_vec();
                AudioCbc::new(&key, &iv).decrypt(&mut payload);
                packets.push(payload);
            }
            // The session stays open until TEARDOWN, which must also be answered.
            let req = loop {
                if let Some(req) = rtsp::decode_request(&mut buffer).unwrap() {
                    break req;
                }
                let mut chunk = [0u8; 2048];
                let n = tcp.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                buffer.extend_from_slice(&chunk[..n]);
            };
            assert_eq!(req.method, "TEARDOWN");
            methods.push(req.method.clone());
            let response = rtsp::Response::to(&req, 200);
            let mut wire = Vec::new();
            response.encode(&mut wire);
            tcp.write_all(&wire).await.unwrap();
            (methods, sdp.unwrap(), volume.unwrap(), packets)
        });

        let mut config = SessionConfig::new(raop("0,1", "1"), Codec::Alac);
        config.receiver.address = address;
        config.audio_key = Some(key);
        config.audio_iv = Some(iv);
        let mut session = Session::connect(config).await.unwrap();
        let pcm: Vec<u8> = (0..PACKET_BYTES).map(|i| (i % 251) as u8).collect();
        session.send(&pcm).await.unwrap();
        session.send(&pcm).await.unwrap();
        session
            .set_volume(Volume::db(-12.0).unwrap())
            .await
            .unwrap();
        session.finish().await.unwrap();

        let (methods, sdp, volume, packets) = timeout(Duration::from_secs(5), fake)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            methods,
            [
                "OPTIONS",
                "ANNOUNCE",
                "SETUP",
                "RECORD",
                "SET_PARAMETER",
                "TEARDOWN"
            ]
        );
        assert!(sdp.contains("m=audio 0 RTP/AVP 96"));
        assert!(sdp.contains("AppleLossless"));
        assert_eq!(volume, "volume: -12.000000\r\n");
        assert_eq!(packets.len(), 2);
        assert_eq!(alac_escape_frame(&pcm).unwrap(), packets[0]);
        assert_eq!(packets[0], packets[1]);
    }

    #[tokio::test]
    async fn rejected_announce_stops_before_record() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let fake = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            for expected in ["OPTIONS", "ANNOUNCE"] {
                let req = loop {
                    if let Some(req) = rtsp::decode_request(&mut buffer).unwrap() {
                        break req;
                    }
                    let mut chunk = [0u8; 1024];
                    let n = tcp.read(&mut chunk).await.unwrap();
                    buffer.extend_from_slice(&chunk[..n]);
                };
                assert_eq!(req.method, expected);
                let mut response =
                    rtsp::Response::to(&req, if expected == "OPTIONS" { 200 } else { 403 });
                if expected == "OPTIONS" {
                    response.headers.insert("Public", "ANNOUNCE, SETUP, RECORD");
                }
                let mut wire = Vec::new();
                response.encode(&mut wire);
                tcp.write_all(&wire).await.unwrap();
            }
        });
        let mut config = SessionConfig::new(raop("0", "0"), Codec::Pcm);
        config.receiver.address = address;
        assert!(Session::connect(config).await.is_err());
        fake.await.unwrap();
    }
}
