//! Ограниченный разбор и загрузка HLS-ресурсов для будущего AirPlay video
//! casting.
//!
//! This crate only accepts ordinary `http`/`https` URLs; the Apple-private
//! `mlhls://` and `nfhls://` schemes need protocol-specific resolvers.
//!
//! Fetching is bounded and rejects non-public DNS results. Optional FFmpeg
//! support demultiplexes finite VOD and decodes video into a generic
//! `FrameSink`; live refresh, audio, and full AirPlay playback controls remain
//! unsupported. The binary's opt-in `hls` feature handles only start/stop.

mod fetch;
mod source;

pub use fetch::{Fetcher, MAX_SEGMENT_BYTES};
pub use source::{SegmentReader, VodSource};
#[cfg(feature = "ffmpeg")]
mod demux;
#[cfg(feature = "ffmpeg")]
pub use demux::{ContainerHint, open_demux};

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
    for line in lines.into_iter().skip(1) {
        if line == "#EXT-X-ENDLIST" {
            live = false;
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
                });
            }
        }
    }
    if pending_variant.is_some() || pending_duration.is_some() {
        return Err(Error::Playlist("playlist ended before a URI".into()));
    }
    Ok(Playlist {
        variants,
        segments,
        live,
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
