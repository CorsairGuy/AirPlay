//! Classic AirPlay 1 (RAOP): `ANNOUNCE` с SDP, UDP-`SETUP` без plist,
//! `RECORD`, `FLUSH` и `TEARDOWN`.
//!
//! Профиль сознательно узкий: принимаем только открытый звук. Шифрование
//! определяется наличием обёрнутого ключа (`a=rsaaeskey`/`a=fpaeskey`/
//! `a=aesiv`), а не списком `et`: `et:0,1` без ключа — открытый поток
//! (так же считает shairport-sync 5.6.3, `rtsp.c`, `handle_announce`).
//! Обёрнутый ключ требует RSA-операции приватным ключом приёмника; у
//! shairport-sync такой ключ есть (`pk` в TXT — его публичная половина),
//! у нас его нет, поэтому такой `ANNOUNCE` отклоняется, а не подтверждается
//! молча. Совместимость с реальными отправителями не проверена без устройства.

use audio_stream::{Stream, StreamKey};
use codec::{Codec, Format};
use rtsp::{Request, Response};
use tracing::{debug, info, warn};

/// Состояние classic-сессии: формат из SDP до `SETUP`.
#[derive(Default)]
pub struct Raop {
    format: Option<Format>,
}

impl Raop {
    /// `ANNOUNCE` с `application/sdp`: разбираем формат и запоминаем его для
    /// `SETUP`. Неподдержанный кодек — 415, шифрованный профиль — 501.
    pub fn announce(&mut self, req: &Request) -> Response {
        // A failed renegotiation must not reuse an earlier cleartext format.
        self.format = None;
        if !req
            .headers
            .get("Content-Type")
            .is_some_and(|value| value.to_ascii_lowercase().contains("application/sdp"))
        {
            warn!("ANNOUNCE без application/sdp");
            return Response::to(req, 415);
        }
        match parse_sdp(&req.body) {
            Ok(format) => {
                info!(?format, "ANNOUNCE (AirPlay 1)");
                self.format = Some(format);
                Response::to(req, 200)
            }
            Err(SdpError::Encrypted) => {
                warn!("ANNOUNCE: ключ, обёрнутый RSA, не поддерживается — поток отклонён");
                Response::to(req, 501)
            }
            Err(SdpError::Codec(why)) => {
                warn!(why, "ANNOUNCE: кодек не поддерживается");
                Response::to(req, 415)
            }
            Err(SdpError::Malformed) => Response::to(req, 400),
        }
    }

    /// `SETUP` с `Transport`: занимаем UDP-порты звука, управления и
    /// времени и отдаём их отправителю. `SETUP` без предшествующего
    /// `ANNOUNCE` — 400.
    pub fn setup(
        &mut self,
        req: &Request,
        cfg: audio_stream::Config,
        sender: std::net::IpAddr,
    ) -> Result<(Response, Stream), Response> {
        let Some(format) = self.format else {
            warn!("SETUP AirPlay 1 без ANNOUNCE");
            return Err(Response::to(req, 400));
        };
        let transport = req.headers.get("Transport").unwrap_or("");
        let sender_control = match parse_transport(transport) {
            Ok(control) => control,
            Err(TransportError::Unsupported) => {
                warn!(transport, "SETUP: профиль транспорта не поддерживается");
                return Err(Response::to(req, 501));
            }
            Err(TransportError::Malformed) => return Err(Response::to(req, 400)),
        };
        let stream = Stream::raop(cfg, StreamKey::Plain, format, sender_control, sender).map_err(|e| {
            warn!("сокеты AirPlay 1: {e}");
            Response::to(req, 500)
        })?;
        debug!(
            data = stream.data_port,
            control = stream.control_port,
            timing = stream.timing_port,
            "SETUP AirPlay 1"
        );
        let mut resp = Response::to(req, 200);
        resp.headers.insert(
            "Transport",
            format!(
                "RTP/AVP/UDP;unicast;mode=record;server_port={};control_port={};timing_port={}",
                stream.data_port, stream.control_port, stream.timing_port
            ),
        );
        resp.headers.insert("Session", "1");
        Ok((resp, stream))
    }

    /// `TEARDOWN`: следующий `SETUP` снова требует `ANNOUNCE`.
    pub fn reset(&mut self) {
        self.format = None;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SdpError {
    Malformed,
    /// `rsaaeskey`/`fpaeskey`/`aesiv`/`et` не 0.
    Encrypted,
    Codec(String),
}

#[derive(Debug, PartialEq, Eq)]
enum TransportError {
    Malformed,
    Unsupported,
}

/// Минимальный разбор SDP: `m=audio`, `a=rtpmap`, `a=fmtp` и признаки
/// шифрования. Прочие строки не обязательны.
fn parse_sdp(body: &[u8]) -> Result<Format, SdpError> {
    let text = std::str::from_utf8(body).map_err(|_| SdpError::Malformed)?;
    let mut audio = false;
    let mut name: Option<String> = None;
    let mut rate: Option<u32> = None;
    let mut channels: Option<u8> = None;
    let mut fmtp: Option<&str> = None;
    let mut key_wrapping = false;
    let mut et_seen = false;
    let mut plain_allowed = true;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("m=audio") {
            if !line.contains("RTP/AVP") {
                return Err(SdpError::Malformed);
            }
            audio = true;
        } else if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let params = value.split_once(' ').map(|(_, rest)| rest).unwrap_or("");
            let mut parts = params.split('/');
            name = parts.next().map(|n| n.trim().to_owned());
            rate = parts.next().and_then(|r| r.trim().parse().ok());
            channels = parts.next().and_then(|c| c.trim().parse().ok());
        } else if let Some(value) = line.strip_prefix("a=fmtp:") {
            if let Some((_, params)) = value.split_once(' ') {
                fmtp = Some(params.trim());
            }
        } else if line.starts_with("a=rsaaeskey:")
            || line.starts_with("a=fpaeskey:")
            || line.starts_with("a=aesiv:")
        {
            key_wrapping = true;
        } else if let Some(value) = line.strip_prefix("a=et:") {
            // `et` — список объявленных профилей, а не решение о шифровании.
            // `et:0,1` — обычное объявление («открытый» + заявка на ключ):
            // без `rsaaeskey`/`aesiv` поток открытый. Шифрование выдаёт
            // только наличие обёрнутого ключа (так же считает shairport-sync
            // 5.6.3, `rtsp.c`, `handle_announce`).
            et_seen = true;
            plain_allowed = value.split(',').any(|item| item.trim() == "0");
        }
    }
    if key_wrapping {
        return Err(SdpError::Encrypted);
    }
    if et_seen && !plain_allowed {
        return Err(SdpError::Encrypted);
    }
    if !audio {
        return Err(SdpError::Malformed);
    }
    let name = name.ok_or(SdpError::Malformed)?;
    let channels = channels.unwrap_or(2);
    match name.to_ascii_lowercase().as_str() {
        "applelossless" => {
            let fmtp = fmtp.ok_or_else(|| SdpError::Codec("ALAC без fmtp".into()))?;
            let numbers: Vec<u32> = fmtp
                .split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect();
            let get = |i: usize| numbers.get(i).copied();
            let frames = get(0).ok_or_else(|| SdpError::Codec("ALAC без frameLength".into()))?;
            let bit_depth = get(2).ok_or_else(|| SdpError::Codec("ALAC без bitDepth".into()))?;
            let alac_channels = get(6).unwrap_or(channels as u32);
            let sample_rate = get(10)
                .or(rate)
                .ok_or_else(|| SdpError::Codec("ALAC без sampleRate".into()))?;
            if !(1..=4096).contains(&frames)
                || !matches!(bit_depth, 16 | 24)
                || !(1..=2).contains(&alac_channels)
                || !(8_000..=192_000).contains(&sample_rate)
            {
                return Err(SdpError::Codec(format!(
                    "неподдержанные параметры ALAC: {frames} кадров, {bit_depth} бит, {alac_channels} кан., {sample_rate} Гц"
                )));
            }
            Ok(Format {
                codec: Codec::Alac {
                    bit_depth: bit_depth as u8,
                    frames_per_packet: frames,
                },
                sample_rate,
                channels: alac_channels as u8,
            })
        }
        "l16" => {
            let frames = fmtp
                .and_then(|f| f.split_whitespace().next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(352);
            let sample_rate = rate.ok_or_else(|| SdpError::Codec("L16 без частоты".into()))?;
            if !(1..=4096).contains(&frames)
                || !(1..=2).contains(&channels)
                || !(8_000..=192_000).contains(&sample_rate)
            {
                return Err(SdpError::Codec("неподдержанные параметры L16".into()));
            }
            Ok(Format {
                codec: Codec::Pcm {
                    frames_per_packet: frames,
                },
                sample_rate,
                channels,
            })
        }
        "mpeg4-generic" => {
            let sample_rate = rate.ok_or_else(|| SdpError::Codec("AAC без частоты".into()))?;
            if !(8_000..=192_000).contains(&sample_rate) || !(1..=2).contains(&channels) {
                return Err(SdpError::Codec("неподдержанные параметры AAC".into()));
            }
            Ok(Format {
                codec: Codec::Aac,
                sample_rate,
                channels,
            })
        }
        other => Err(SdpError::Codec(format!("кодек {other}"))),
    }
}

/// `RTP/AVP/UDP;unicast;mode=record;control_port=…;timing_port=…`.
/// Некоторые UDP-отправители также указывают `interleaved`; транспорт при этом остаётся UDP.
/// Возвращает `control_port` отправителя — туда шлём запросы пересылки.
fn parse_transport(value: &str) -> Result<Option<u16>, TransportError> {
    if value.trim().is_empty() {
        return Err(TransportError::Malformed);
    }
    let lower = value.to_ascii_lowercase();
    if !lower.contains("rtp/avp") {
        return Err(TransportError::Malformed);
    }
    if lower.split(';').next().map(str::trim) != Some("rtp/avp/udp") {
        return Err(TransportError::Unsupported);
    }
    let mut control = None;
    for param in lower.split(';').skip(1) {
        if let Some((key, value)) = param.split_once('=') {
            if key.trim() == "mode" && value.trim().trim_matches('"') != "record" {
                return Err(TransportError::Unsupported);
            }
            if key.trim() == "control_port" {
                control = value.trim().parse().ok();
            }
        }
    }
    Ok(control)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALAC: &str = "v=0\r\no=Sender 1 0 IN IP4 192.168.1.10\r\ns=AirTunes\r\n\
c=IN IP4 192.168.1.20\r\nt=0 0\r\nm=audio 0 RTP/AVP 96\r\n\
a=rtpmap:96 AppleLossless\r\na=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100\r\n";

    #[test]
    fn alac_sdp() {
        assert_eq!(
            parse_sdp(ALAC.as_bytes()).unwrap(),
            Format {
                codec: Codec::Alac {
                    bit_depth: 16,
                    frames_per_packet: 352
                },
                sample_rate: 44100,
                channels: 2,
            }
        );
    }

    #[test]
    fn alac_24_bit_48k_uses_fmtp_values() {
        let sdp = ALAC.replace("16 40 10 14 2", "24 40 10 14 2");
        let sdp = sdp.replace("0 44100", "0 48000").replace("352 0", "512 0");
        let format = parse_sdp(sdp.as_bytes()).unwrap();
        assert_eq!(format.sample_rate, 48000);
        assert_eq!(format.frames_per_packet(), 512);
        assert_eq!(
            format.codec,
            Codec::Alac {
                bit_depth: 24,
                frames_per_packet: 512
            }
        );
    }

    #[test]
    fn pcm_and_aac() {
        let pcm = ALAC
            .replace("AppleLossless", "L16/44100/2")
            .replace("a=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100\r\n", "");
        assert_eq!(
            parse_sdp(pcm.as_bytes()).unwrap().codec,
            Codec::Pcm {
                frames_per_packet: 352
            }
        );
        let aac = ALAC
            .replace("AppleLossless", "mpeg4-generic/44100/2")
            .replace(" 352 0 16 40 10 14 2 255 0 0 44100", " config=1210");
        assert_eq!(parse_sdp(aac.as_bytes()).unwrap().codec, Codec::Aac);
    }

    #[test]
    fn encryption_is_rejected_not_ignored() {
        for attr in [
            "a=rsaaeskey:AAAA\r\n",
            "a=fpaeskey:AAAA\r\n",
            "a=aesiv:AAAA\r\n",
            "a=et:1\r\n",
        ] {
            let sdp = format!("{ALAC}{attr}");
            assert_eq!(
                parse_sdp(sdp.as_bytes()),
                Err(SdpError::Encrypted),
                "{attr}"
            );
        }
        // et:0 — открытый звук.
        assert!(parse_sdp(format!("{ALAC}a=et:0\r\n").as_bytes()).is_ok());
        // et:0,1 — обычное объявление настоящего отправителя: заявка на ключ
        // без `rsaaeskey` — это открытый поток, а не шифрованный.
        assert!(parse_sdp(format!("{ALAC}a=et:0,1\r\n").as_bytes()).is_ok());
    }

    #[test]
    fn rejected_announce_clears_previous_format() {
        let mut raop = Raop::default();
        let mut request = Request::new("ANNOUNCE", "rtsp://test/1", "RTSP/1.0");
        request.headers.insert("Content-Type", "application/sdp");
        request.body = ALAC.as_bytes().to_vec();
        assert_eq!(raop.announce(&request).status, 200);
        request.body = format!("{ALAC}a=rsaaeskey:AAAA\r\n").into_bytes();
        assert_eq!(raop.announce(&request).status, 501);
        assert!(raop.format.is_none());
    }

    #[test]
    fn unsupported_codec_and_malformed_sdp() {
        let mp3 = ALAC.replace("AppleLossless", "MPA/44100/2");
        assert!(matches!(parse_sdp(mp3.as_bytes()), Err(SdpError::Codec(_))));
        let no_audio = ALAC.replace("m=audio", "m=video");
        assert_eq!(parse_sdp(no_audio.as_bytes()), Err(SdpError::Malformed));
        let channels = ALAC.replace("14 2 255", "14 6 255");
        assert!(matches!(
            parse_sdp(channels.as_bytes()),
            Err(SdpError::Codec(_))
        ));
    }

    #[test]
    fn transport_profiles() {
        assert_eq!(
            parse_transport("RTP/AVP/UDP;unicast;mode=record;control_port=6001;timing_port=6002"),
            Ok(Some(6001))
        );
        assert_eq!(parse_transport("RTP/AVP/UDP;unicast;mode=record"), Ok(None));
        assert_eq!(
            parse_transport("RTP/AVP;unicast;mode=record"),
            Err(TransportError::Unsupported)
        );
        assert_eq!(parse_transport(""), Err(TransportError::Malformed));
        assert_eq!(
            parse_transport("RTP/AVP/TCP;unicast;interleaved=0-1"),
            Err(TransportError::Unsupported)
        );
        assert_eq!(
            parse_transport("RTP/AVP/UDP;unicast;interleaved=0-1;mode=record"),
            Ok(None)
        );
        assert_eq!(
            parse_transport("RTP/AVP/TCP;unicast;description=udp"),
            Err(TransportError::Unsupported)
        );
        assert_eq!(
            parse_transport("RTP/AVP/UDP;unicast;mode=play"),
            Err(TransportError::Unsupported)
        );
    }
}
