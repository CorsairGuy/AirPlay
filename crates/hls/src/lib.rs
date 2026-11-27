//! Ограниченный разбор и загрузка HLS-ресурсов для будущего AirPlay video
//! casting.
//!
//! This crate only accepts ordinary `http`/`https` URLs; the Apple-private
//! `mlhls://` and `nfhls://` schemes need protocol-specific resolvers.
//!
//! Fetching is bounded and rejects non-public DNS results. Optional FFmpeg
//! support demultiplexes finite VOD, decodes video into a generic `FrameSink`
//! and audio into an [`audio::Sink`], keeping the two streams on one
//! presentation clock, and offers segment-granular pause/resume and seek.
//! Live refresh, encrypted playlists and FairPlay remain unsupported.

mod fetch;
mod source;

pub use fetch::{Fetcher, MAX_SEGMENT_BYTES};
pub use source::{SegmentReader, VodSource};
#[cfg(test)]
mod testserver;

#[cfg(feature = "ffmpeg")]
mod demux;
#[cfg(feature = "ffmpeg")]
mod player;
#[cfg(feature = "ffmpeg")]
pub use demux::{ContainerHint, open_demux};
#[cfg(feature = "ffmpeg")]
pub use player::{AudioOptions, Playback, PlaybackOptions};

pub const MAX_PLAYLIST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PLAYLIST_LINES: usize = 32_768;
pub const MAX_SEGMENTS: usize = 10_000;
pub const MAX_VARIANTS: usize = 256;
pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url(reqwest::Url);

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("only http and https HLS URLs are supported")]
    UnsupportedScheme,
    #[error("invalid HLS URL")]
    InvalidUrl,
    #[error("HLS URL must not contain credentials")]
    Credentials,
    #[error("HLS URL has no host")]
    MissingHost,
    #[error("HLS URL points at a non-public address")]
    PrivateAddress,
    #[error("playlist exceeds configured limits")]
    Limit,
    #[error("invalid HLS playlist: {0}")]
    Playlist(String),
    #[error("unsupported HLS feature: {0}")]
    UnsupportedFeature(&'static str),
    #[error("encrypted or DRM HLS is not supported")]
    Encrypted,
    #[error("unsafe redirect")]
    UnsafeRedirect,
    #[error("HLS request failed")]
    Fetch,
    #[error("HLS request timed out")]
    Timeout,
    #[error("HLS server returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("HLS master playlist has no usable variant")]
    NoVariant,
    #[error("FFmpeg could not demultiplex the HLS stream")]
    Demux,
    #[error("video decoder failed: {0}")]
    Decode(String),
    #[error("audio output failed: {0}")]
    Audio(String),
    #[error("video worker failed: {0}")]
    Worker(String),
}

impl Url {
    pub fn parse(input: &str) -> Result<Self, Error> {
        let parsed = reqwest::Url::parse(input).map_err(|_| Error::InvalidUrl)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::UnsupportedScheme);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::Credentials);
        }
        let host = parsed.host_str().ok_or(Error::MissingHost)?;
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        if host.parse::<std::net::IpAddr>().is_ok_and(is_disallowed_ip) {
            return Err(Error::PrivateAddress);
        }
        Ok(Self(parsed))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn join(&self, reference: &str) -> Result<Self, Error> {
        let joined = self.0.join(reference).map_err(|_| Error::InvalidUrl)?;
        Self::parse(joined.as_str())
    }
}

fn is_disallowed_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a == 0
                || a >= 224
        }
        std::net::IpAddr::V6(ip) => {
            let octets = ip.octets();
            let mapped_v4 = octets[..10] == [0; 10] && octets[10..12] == [0xff, 0xff];
            let translated_v4 = octets[..12] == [0, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0];
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_multicast()
                || ip.segments()[0] & 0xffc0 == 0xfe80
                || ip.segments()[0] & 0xffc0 == 0xfec0
                || (ip.segments()[0] == 0x2001
                    && (ip.segments()[1] <= 0x01ff || ip.segments()[1] == 0x0db8))
                || ip.segments()[0] == 0x2002
                || ((mapped_v4 || translated_v4)
                    && is_disallowed_ip(
                        std::net::Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15])
                            .into(),
                    ))
        }
    }
}

pub fn validate_redirect(from: &Url, to: &Url) -> Result<(), Error> {
    if from.0.scheme() == "https" && to.0.scheme() != "https" {
        return Err(Error::UnsafeRedirect);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub variants: Vec<Variant>,
    pub segments: Vec<Segment>,
    pub live: bool,
    /// `EXT-X-MEDIA-SEQUENCE`: the sequence number of the first segment.
    /// Kept as confirmed playlist data; a VOD timeline is still the sum of
    /// `EXTINF` durations.
    pub media_sequence: u64,
    /// `EXT-X-DISCONTINUITY-SEQUENCE`: how many discontinuities preceded the
    /// first segment. Used to mark a timeline restart, not to shift playback.
    pub discontinuity_sequence: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub uri: Url,
    pub bandwidth: Option<u64>,
    pub codecs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub uri: Url,
    pub duration_ms: u64,
    pub initialization_segment: Option<Url>,
    /// `EXT-X-DISCONTINUITY` preceded this segment: its media timestamps may
    /// restart, so the demuxer must not treat the join as a continuous timeline.
    pub discontinuity: bool,
}

pub fn parse_playlist(base: &Url, bytes: &[u8]) -> Result<Playlist, Error> {
    if bytes.len() > MAX_PLAYLIST_BYTES {
        return Err(Error::Limit);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Playlist("not UTF-8".into()))?;
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() > MAX_PLAYLIST_LINES || lines.first().copied() != Some("#EXTM3U") {
        return Err(Error::Playlist("missing #EXTM3U or too many lines".into()));
    }
    let mut variants = Vec::new();
    let mut segments = Vec::new();
    let mut pending_variant = None;
    let mut pending_duration = None;
    let mut initialization_segment = None;
    let mut live = true;
    let mut pending_discontinuity = false;
    let mut media_sequence = 0u64;
    let mut discontinuity_sequence = 0u64;
    for line in lines.into_iter().skip(1) {
        if line == "#EXT-X-ENDLIST" {
            live = false;
        } else if line == "#EXT-X-DISCONTINUITY" {
            if pending_discontinuity {
                return Err(Error::Playlist("repeated EXT-X-DISCONTINUITY".into()));
            }
            pending_discontinuity = true;
        } else if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            media_sequence = value
                .trim()
                .parse()
                .map_err(|_| Error::Playlist("invalid EXT-X-MEDIA-SEQUENCE".into()))?;
        } else if let Some(value) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
            discontinuity_sequence = value
                .trim()
                .parse()
                .map_err(|_| Error::Playlist("invalid EXT-X-DISCONTINUITY-SEQUENCE".into()))?;
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            if variants.len() >= MAX_VARIANTS {
                return Err(Error::Limit);
            }
            if pending_variant.is_some() {
                return Err(Error::Playlist("variant URI is missing".into()));
            }
            pending_variant = Some((attr_u64(attrs, "BANDWIDTH"), attr_string(attrs, "CODECS")));
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            if attr_string(attrs, "BYTERANGE").is_some() {
                return Err(Error::UnsupportedFeature("initialization byte ranges"));
            }
            let reference = attr_string(attrs, "URI")
                .ok_or_else(|| Error::Playlist("EXT-X-MAP has no URI".into()))?;
            initialization_segment = Some(base.join(&reference)?);
        } else if let Some(value) = line.strip_prefix("#EXTINF:") {
            if pending_duration.is_some() {
                return Err(Error::Playlist("segment URI is missing".into()));
            }
            pending_duration = Some(
                value
                    .split(',')
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .map(|v| (v * 1000.0) as u64)
                    .ok_or_else(|| Error::Playlist("invalid EXTINF".into()))?,
            );
        } else if let Some(attrs) = line
            .strip_prefix("#EXT-X-KEY:")
            .or_else(|| line.strip_prefix("#EXT-X-SESSION-KEY:"))
        {
            let method = attr_string(attrs, "METHOD")
                .ok_or_else(|| Error::Playlist("key tag has no METHOD".into()))?;
            if method != "NONE" {
                return Err(Error::Encrypted);
            }
        } else if line.starts_with("#EXT-X-BYTERANGE:") {
            return Err(Error::UnsupportedFeature("byte ranges"));
        } else if !line.starts_with('#') {
            let uri = base.join(line)?;
            if let Some((bandwidth, codecs)) = pending_variant.take() {
                variants.push(Variant {
                    uri,
                    bandwidth,
                    codecs,
                });
            } else {
                if segments.len() >= MAX_SEGMENTS {
                    return Err(Error::Limit);
                }
                let duration_ms = pending_duration
                    .take()
                    .ok_or_else(|| Error::Playlist("segment has no EXTINF".into()))?;
                segments.push(Segment {
                    uri,
                    duration_ms,
                    initialization_segment: initialization_segment.clone(),
                    discontinuity: std::mem::take(&mut pending_discontinuity),
                });
            }
        }
    }
    if pending_variant.is_some() || pending_duration.is_some() {
        return Err(Error::Playlist("playlist ended before a URI".into()));
    }
    if pending_discontinuity {
        return Err(Error::Playlist(
            "EXT-X-DISCONTINUITY without a segment".into(),
        ));
    }
    Ok(Playlist {
        variants,
        segments,
        live,
        media_sequence,
        discontinuity_sequence,
    })
}

fn attr_string(attrs: &str, name: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index] == b',' || bytes[index].is_ascii_whitespace()) {
            index += 1;
        }
        let key_start = index;
        while index < bytes.len() && bytes[index] != b'=' && bytes[index] != b',' {
            index += 1;
        }
        if index == bytes.len() || bytes[index] != b'=' {
            break;
        }
        let key = attrs[key_start..index].trim();
        index += 1;
        let value = if index < bytes.len() && bytes[index] == b'"' {
            index += 1;
            let value_start = index;
            while index < bytes.len() && bytes[index] != b'"' {
                index += 1;
            }
            if index == bytes.len() {
                return None;
            }
            let value = attrs[value_start..index].to_owned();
            index += 1;
            value
        } else {
            let value_start = index;
            while index < bytes.len() && bytes[index] != b',' {
                index += 1;
            }
            attrs[value_start..index].trim().to_owned()
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        while index < bytes.len() && bytes[index] != b',' {
            index += 1;
        }
    }
    None
}

fn attr_u64(attrs: &str, name: &str) -> Option<u64> {
    attr_string(attrs, name).and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Векторы SSRF-защиты: адреса написаны руками, без DNS и сети.
    #[test]
    fn cast_urls_must_be_public_http_or_https() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/x.m3u8",
            "data:text/plain,hello",
            "https://user:pass@cdn.example/x.m3u8",
            "http://127.0.0.1/x.m3u8",
            "http://10.0.0.1/x.m3u8",
            "http://169.254.1.1/x.m3u8",
            "http://0.0.0.0/x.m3u8",
            "http://224.0.0.1/x.m3u8",
            "http://100.64.0.1/x.m3u8",
            "http://192.0.2.1/x.m3u8",
            "http://198.18.0.1/x.m3u8",
            "http://[::1]/x.m3u8",
            "http://[::ffff:127.0.0.1]/x.m3u8",
            "http://[::ffff:10.0.0.1]/x.m3u8",
            "http://[fe80::1]/x.m3u8",
            "http://[fd00::1]/x.m3u8",
            "http://[2001:db8::1]/x.m3u8",
            "http://[2002:7f00:1::]/x.m3u8",
            "http://[ff02::1]/x.m3u8",
        ] {
            assert!(Url::parse(url).is_err(), "{url} must be refused");
        }
        for url in [
            "https://cdn.example/video/master.m3u8",
            "http://8.8.8.8/x.m3u8",
            "http://[2606:4700:4700::1111]/x.m3u8",
            "http://[::ffff:8.8.8.8]/x.m3u8",
        ] {
            assert!(Url::parse(url).is_ok(), "{url} must be allowed");
        }
    }

    /// Ссылка в плейлисте и редирект не обходят проверку адреса.
    #[test]
    fn playlist_references_and_redirects_are_revalidated() {
        let base = Url::parse("https://cdn.example/video/master.m3u8").unwrap();
        assert!(base.join("seg-1.ts").is_ok());
        assert!(base.join("http://127.0.0.1/seg.ts").is_err());
        assert!(base.join("//[::1]/seg.ts").is_err());
        assert!(base.join("file:///etc/passwd").is_err());
        let https = Url::parse("https://cdn.example/a").unwrap();
        let downgrade = Url::parse("http://cdn.example/b").unwrap();
        let upgrade = Url::parse("https://cdn.example/c").unwrap();
        assert_eq!(
            validate_redirect(&https, &downgrade),
            Err(Error::UnsafeRedirect)
        );
        assert_eq!(validate_redirect(&https, &upgrade), Ok(()));
    }

    /// Разрыв таймлайна и номера последовательностей — подтверждённые данные.
    #[test]
    fn parses_discontinuity_and_media_sequence() {
        let base = Url::parse("https://cdn.example/v/index.m3u8").unwrap();
        let playlist = parse_playlist(
            &base,
            b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-DISCONTINUITY-SEQUENCE:2\n#EXTINF:1,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\nb.ts\n#EXT-X-ENDLIST\n",
        )
        .unwrap();
        assert_eq!(playlist.media_sequence, 7);
        assert_eq!(playlist.discontinuity_sequence, 2);
        assert_eq!(playlist.segments.len(), 2);
        assert!(!playlist.segments[0].discontinuity);
        assert!(playlist.segments[1].discontinuity);
        assert!(!playlist.live);
    }

    #[test]
    fn discontinuity_and_sequence_tags_are_strict() {
        let base = Url::parse("https://cdn.example/v/index.m3u8").unwrap();
        assert!(matches!(
            parse_playlist(
                &base,
                b"#EXTM3U\n#EXT-X-DISCONTINUITY\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\na.ts\n"
            ),
            Err(Error::Playlist(_))
        ));
        assert!(matches!(
            parse_playlist(
                &base,
                b"#EXTM3U\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\na.ts\n#EXT-X-DISCONTINUITY\n"
            ),
            Err(Error::Playlist(_))
        ));
        assert!(matches!(
            parse_playlist(
                &base,
                b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:x\n#EXTINF:1,\na.ts\n"
            ),
            Err(Error::Playlist(_))
        ));
        let live = parse_playlist(&base, b"#EXTM3U\n#EXTINF:1,\na.ts\n").unwrap();
        assert!(live.live, "a playlist without ENDLIST is still live");
        assert_eq!(live.media_sequence, 0);
    }

    #[test]
    fn parses_master_and_media_playlists() {
        let base = Url::parse("https://cdn.example/video/master.m3u8").unwrap();
        let master = parse_playlist(
            &base,
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100000,CODECS=\"avc1\"\nlow/index.m3u8\n",
        )
        .unwrap();
        assert_eq!(master.variants[0].bandwidth, Some(100000));
        let media =
            parse_playlist(&base, b"#EXTM3U\n#EXTINF:2.5,\nseg-1.ts\n#EXT-X-ENDLIST\n").unwrap();
        assert_eq!(media.segments[0].duration_ms, 2500);
        assert!(!media.live);
    }

    #[test]
    fn relative_segments_resolve_against_the_playlist_url() {
        let base = Url::parse("https://cdn.example/video/show/index.m3u8").unwrap();
        let playlist = parse_playlist(
            &base,
            b"#EXTM3U
#EXTINF:1,
seg.ts
#EXTINF:1,
../shared/seg2.ts
#EXTINF:1,
/root/seg3.ts
#EXTINF:1,
sub/../seg4.ts?token=1
#EXT-X-ENDLIST
",
        )
        .unwrap();
        let uris: Vec<_> = playlist
            .segments
            .iter()
            .map(|segment| segment.uri.as_str())
            .collect();
        assert_eq!(
            uris,
            [
                "https://cdn.example/video/show/seg.ts",
                "https://cdn.example/video/shared/seg2.ts",
                "https://cdn.example/root/seg3.ts",
                "https://cdn.example/video/show/seg4.ts?token=1"
            ]
        );
    }

    #[test]
    fn repeated_segments_are_not_silently_deduplicated() {
        let base = Url::parse("https://cdn.example/a.m3u8").unwrap();
        let playlist = parse_playlist(
            &base,
            b"#EXTM3U
#EXTINF:1,
loop.ts
#EXTINF:1,
loop.ts
#EXT-X-ENDLIST
",
        )
        .unwrap();
        assert_eq!(playlist.segments.len(), 2, "повтор в плейлисте сохраняется");
        assert_eq!(playlist.segments[0].uri, playlist.segments[1].uri);
    }

    #[test]
    fn a_relative_playlist_cannot_escape_to_another_host() {
        let base = Url::parse("https://cdn.example/video/index.m3u8").unwrap();
        // Абсолютный URL остаётся абсолютным, но проверка хоста — на загрузке.
        let playlist = parse_playlist(
            &base,
            b"#EXTM3U
#EXTINF:1,
https://other.example/seg.ts
#EXT-X-ENDLIST
",
        )
        .unwrap();
        assert_eq!(
            playlist.segments[0].uri.as_str(),
            "https://other.example/seg.ts"
        );
        assert_eq!(
            Url::parse("file:///etc/passwd").unwrap_err(),
            Error::UnsupportedScheme
        );
    }

    #[test]
    fn rejects_private_and_custom_urls() {
        assert_eq!(
            Url::parse("mlhls://opaque").unwrap_err(),
            Error::UnsupportedScheme
        );
        assert_eq!(
            Url::parse("http://127.0.0.1/x.m3u8").unwrap_err(),
            Error::PrivateAddress
        );
        assert_eq!(
            Url::parse("http://[::1]/x.m3u8").unwrap_err(),
            Error::PrivateAddress
        );
        assert_eq!(
            Url::parse(&format!("https://{}@host/x", "user")).unwrap_err(),
            Error::Credentials
        );
    }

    #[test]
    fn rejects_encrypted_playlists() {
        let base = Url::parse("https://cdn.example/a.m3u8").unwrap();
        assert_eq!(
            parse_playlist(&base, b"#EXTM3U\n#EXT-X-KEY:METHOD=AES-128\n").unwrap_err(),
            Error::Encrypted
        );
    }
}
