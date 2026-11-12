//! No-I/O PCM sender packetization, based on Station feature/group-local-sync
//! commit 0df97db7860b510fb456215504bafcdcea672502, airplay_sender/src/main.rs.
//! Reuses this crate's Header and AudioCipher. No discovery, clock reading or sockets.
use crate::{AudioCipher, Header};
use std::fmt;

/// Negotiated sender profile: at most 352 S16BE stereo frames per RTP packet.
pub const MAX_PCM_FRAMES: usize = 352;
const PCM_FRAME_BYTES: usize = 4;
pub const RETRANSMIT_CACHE_PACKETS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketizeError {
    InvalidPcmLength,
    NonMonotonicFrameIndex,
    FrameIndexOverflow,
    NonceExhausted,
    InvalidRenderLatency,
}

impl fmt::Display for PacketizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPcmLength => {
                "PCM packet must contain 1..=352 complete S16BE stereo frames"
            }
            Self::NonMonotonicFrameIndex => "PCM frame index overlaps previously packetized frames",
            Self::FrameIndexOverflow => "PCM frame index overflow",
            Self::NonceExhausted => "audio nonce exhausted; establish a new keyed session",
            Self::InvalidRenderLatency => "receiver render latency exceeds total playout lead",
        })
    }
}
impl std::error::Error for PacketizeError {}

/// One encrypted audio packet and an anchor computed from the same producer epoch.
/// The caller sends sync initially and periodically (normally once per second).
pub struct Packetized {
    pub audio: Vec<u8>,
    pub header: Header,
    pub sync: [u8; 20],
}

/// AirPlay NTP sync packet. RTP source/destination differ by latency_frames.
/// produced_ntp is supplied by the shared PCM producer, not a per-target handshake.
pub fn sync_packet(
    timestamp: u32,
    latency_frames: u32,
    produced_ntp: u64,
    first: bool,
) -> [u8; 20] {
    let mut packet = [0; 20];
    packet[0] = if first { 0x90 } else { 0x80 };
    packet[1] = 0xd4;
    packet[2..4].copy_from_slice(&7u16.to_be_bytes());
    packet[4..8].copy_from_slice(&timestamp.wrapping_sub(latency_frames).to_be_bytes());
    packet[8..16].copy_from_slice(&produced_ntp.to_be_bytes());
    packet[16..20].copy_from_slice(&timestamp.to_be_bytes());
    packet
}

/// The control header's request sequence (bytes2..4) is not the lost audio sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetransmitRequest {
    pub first_sequence: u16,
    pub count: u16,
}

impl RetransmitRequest {
    pub fn parse(packet: &[u8]) -> Option<Self> {
        if packet.len() < 8 || packet[0] >> 6 != 2 || packet[1] & 0x7f != 0x55 {
            return None;
        }
        Some(Self {
            first_sequence: u16::from_be_bytes([packet[4], packet[5]]),
            count: u16::from_be_bytes([packet[6], packet[7]]),
        })
    }
}

/// State belongs to exactly one newly keyed target session. Do not recreate this
/// object with the same key to reset its nonce; reconnect with fresh key material.
/// Not Clone: sequence wrap must never reset the independent 64-bit nonce.
pub struct PcmPacketizer {
    cipher: AudioCipher,
    ssrc: u32,
    sequence: u16,
    next_nonce: Option<u64>,
    latency_frames: u32,
    mapping_lead_frames: u32,
    first_index: Option<u64>,
    next_min_index: Option<u64>,
    cache: Vec<Option<(u16, Vec<u8>)>>,
}

impl PcmPacketizer {
    pub fn new(key: [u8; 32], ssrc: u32, sequence: u16, latency_frames: u32) -> Self {
        Self {
            cipher: AudioCipher::new(&key),
            ssrc,
            sequence,
            next_nonce: Some(0),
            latency_frames,
            mapping_lead_frames: latency_frames,
            first_index: None,
            next_min_index: None,
            cache: (0..RETRANSMIT_CACHE_PACKETS).map(|_| None).collect(),
        }
    }

    /// AP2 adds its render minimum after the RTP/NTP mapping. Keep the RTP
    /// origin and total local deadline, but put only the remaining lead in sync.
    pub fn with_render_latency(
        key: [u8; 32],
        ssrc: u32,
        sequence: u16,
        total_frames: u32,
        render_frames: u32,
    ) -> Result<Self, PacketizeError> {
        let lead = total_frames
            .checked_sub(render_frames)
            .ok_or(PacketizeError::InvalidRenderLatency)?;
        let mut packetizer = Self::new(key, ssrc, sequence, total_frames);
        packetizer.mapping_lead_frames = lead;
        Ok(packetizer)
    }

    /// Timestamp announced by the caller's initial FLUSH before audio starts.
    pub fn initial_timestamp(&self) -> u32 {
        self.latency_frames
    }
    pub fn mapping_lead_frames(&self) -> u32 {
        self.mapping_lead_frames
    }
    pub fn next_sequence(&self) -> u16 {
        self.sequence
    }
    pub fn cached_packets(&self) -> usize {
        self.cache.iter().filter(|v| v.is_some()).count()
    }
    /// Drop obsolete ciphertext without rewinding sequence, nonce, timestamp
    /// origin or first-marker state. Never recreate the packetizer for a seek.
    pub fn clear_retransmit_cache(&mut self) {
        for entry in &mut self.cache {
            *entry = None;
        }
    }

    pub fn packetize(
        &mut self,
        pcm: &[u8],
        frame_index: u64,
        produced_ntp: u64,
    ) -> Result<Packetized, PacketizeError> {
        if pcm.is_empty()
            || pcm.len() > MAX_PCM_FRAMES * PCM_FRAME_BYTES
            || pcm.len() % PCM_FRAME_BYTES != 0
        {
            return Err(PacketizeError::InvalidPcmLength);
        }
        if self.next_min_index.is_some_and(|next| frame_index < next) {
            return Err(PacketizeError::NonMonotonicFrameIndex);
        }
        let next_index = frame_index
            .checked_add((pcm.len() / PCM_FRAME_BYTES) as u64)
            .ok_or(PacketizeError::FrameIndexOverflow)?;
        let nonce = self.next_nonce.ok_or(PacketizeError::NonceExhausted)?;
        let first = self.first_index.is_none();
        let origin = self.first_index.unwrap_or(frame_index);
        let timestamp = self
            .latency_frames
            .wrapping_add(frame_index.wrapping_sub(origin) as u32);
        let header = Header {
            marker: first,
            payload_type: 96,
            seq: self.sequence,
            timestamp,
            ssrc: self.ssrc,
        };
        let audio = self.cipher.seal(&header, pcm, nonce);
        let sync = sync_packet(timestamp, self.mapping_lead_frames, produced_ntp, first);
        self.cache[self.sequence as usize % RETRANSMIT_CACHE_PACKETS] =
            Some((self.sequence, audio.clone()));
        self.first_index = Some(origin);
        self.next_min_index = Some(next_index);
        self.sequence = self.sequence.wrapping_add(1);
        self.next_nonce = nonce.checked_add(1);
        Ok(Packetized {
            audio,
            header,
            sync,
        })
    }

    /// Re-send original ciphertext and nonce, without encrypting or advancing state.
    /// Work/output are capped to512 requested sequence positions even for count65535.
    /// Authenticate/filter the control source address at the I/O boundary.
    pub fn retransmit(&self, request: &[u8]) -> Vec<Vec<u8>> {
        let Some(request) = RetransmitRequest::parse(request) else {
            return Vec::new();
        };
        let mut packets = Vec::new();
        for offset in 0..usize::from(request.count).min(RETRANSMIT_CACHE_PACKETS) {
            let wanted = request.first_sequence.wrapping_add(offset as u16);
            match &self.cache[wanted as usize % RETRANSMIT_CACHE_PACKETS] {
                Some((sequence, audio)) if *sequence == wanted => {
                    let mut reply = Vec::with_capacity(4 + audio.len());
                    reply.extend_from_slice(&[0x80, 0xd6]);
                    reply.extend_from_slice(&wanted.to_be_bytes());
                    reply.extend_from_slice(audio);
                    packets.push(reply);
                }
                _ => {}
            }
        }
        packets
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ap2_render_min_is_counted_once_without_changing_audio_timestamp() {
        for render in [0, 8_820, 11_025, 22_050] {
            let mut corrected =
                super::PcmPacketizer::with_render_latency([7; 32], 1, 9, 22_050, render).unwrap();
            let mut legacy = super::PcmPacketizer::new([7; 32], 1, 9, 22_050);
            assert_eq!(corrected.initial_timestamp(), legacy.initial_timestamp());
            for frame in [0, u32::MAX as u64 + 352] {
                let p = corrected.packetize(&[0; 1408], frame, 123).unwrap();
                let old = legacy.packetize(&[0; 1408], frame, 123).unwrap();
                assert_eq!(p.audio, old.audio);
                let mapped = u32::from_be_bytes(p.sync[4..8].try_into().unwrap());
                assert_eq!(p.header.timestamp.wrapping_sub(mapped) + render, 22_050);
                assert_eq!(u64::from_be_bytes(p.sync[8..16].try_into().unwrap()), 123);
            }
        }
        assert!(super::PcmPacketizer::with_render_latency([0; 32], 1, 1, 22_050, 22_051).is_err());
    }
    use super::*;
    fn request(first: u16, count: u16) -> [u8; 8] {
        let mut p = [0x80, 0x55, 0xaa, 0xbb, 0, 0, 0, 0];
        p[4..6].copy_from_slice(&first.to_be_bytes());
        p[6..8].copy_from_slice(&count.to_be_bytes());
        p
    }
    fn sender(seq: u16) -> PcmPacketizer {
        PcmPacketizer::new([3; 32], 0x12345678, seq, 44_100)
    }
    #[test]
    fn clearing_retransmits_keeps_cipher_nonce_sequence_and_media_epoch() {
        let mut sender = sender(u16::MAX);
        let first = sender.packetize(&[7; 1408], 1000, 1).unwrap();
        assert_eq!(sender.retransmit(&request(first.header.seq, 1)).len(), 1);
        sender.clear_retransmit_cache();
        assert_eq!(sender.cached_packets(), 0);
        assert!(sender.retransmit(&request(first.header.seq, 1)).is_empty());
        let next = sender.packetize(&[8; 1408], 1352, 2).unwrap();
        assert_eq!(next.header.seq, 0);
        assert_eq!(
            next.header.timestamp,
            first.header.timestamp.wrapping_add(352)
        );
        assert!(!next.header.marker);
        assert_eq!(&next.audio[next.audio.len() - 8..], &1u64.to_le_bytes());
        assert_eq!(
            &sender.retransmit(&request(0, 1))[0][4..],
            next.audio.as_slice()
        );
    }
    #[test]
    fn pcm_encrypts_once_with_first_marker_and_shared_producer_epoch() {
        let mut sender = sender(7);
        assert_eq!(sender.initial_timestamp(), 44_100);
        let pcm = vec![0x23; 1408];
        let first = sender.packetize(&pcm, 1000, 0x0102030405060708).unwrap();
        let later = sender.packetize(&pcm, 1704, 0x1112131415161718).unwrap();
        assert!(first.header.marker);
        assert!(!later.header.marker);
        assert_eq!(first.header.timestamp, 44_100);
        assert_eq!(
            later.header.timestamp, 44_804,
            "dropped producer packet retains timeline gap"
        );
        assert_eq!(&first.sync[8..16], &0x0102030405060708u64.to_be_bytes());
        for (packet, nonce) in [(first, 0u64), (later, 1)] {
            assert_eq!(
                &packet.audio[packet.audio.len() - 8..],
                &nonce.to_le_bytes()
            );
            let mut audio = packet.audio;
            let (h, data) = AudioCipher::new(&[3; 32]).open(&mut audio).unwrap();
            assert_eq!(h, packet.header);
            assert_eq!(data, pcm);
        }
    }
    #[test]
    fn sequence_and_timestamp_wrap_do_not_repeat_nonce_or_marker() {
        let mut sender = PcmPacketizer::new([3; 32], 1, u16::MAX, u32::MAX - 1);
        let a = sender.packetize(&[0; 8], 0, 1).unwrap();
        let b = sender.packetize(&[0; 8], 2, 2).unwrap();
        assert_eq!(a.header.seq, u16::MAX);
        assert_eq!(b.header.seq, 0);
        assert_eq!(b.header.timestamp, 0);
        assert!(!b.header.marker);
        assert_eq!(&b.audio[b.audio.len() - 8..], &1u64.to_le_bytes());
        assert_eq!(sender.retransmit(&request(u16::MAX, 2)).len(), 2);
    }
    #[test]
    fn retry_is_exact_original_packet_and_does_not_consume_nonce() {
        let mut sender = sender(7);
        let packet = sender.packetize(&[1; 4], 0, 1).unwrap();
        let replies = sender.retransmit(&request(7, 1));
        assert_eq!(&replies[0][..4], &[0x80, 0xd6, 0, 7]);
        assert_eq!(&replies[0][4..], packet.audio);
        assert_eq!(sender.retransmit(&request(7, 1)), replies);
        let next = sender.packetize(&[1; 4], 1, 2).unwrap();
        assert_eq!(next.header.seq, 8);
        assert_eq!(&next.audio[next.audio.len() - 8..], &1u64.to_le_bytes());
    }
    #[test]
    fn cache_and_request_work_are_bounded_and_evicted_alias_is_not_returned() {
        let mut sender = sender(0);
        for i in 0..513 {
            sender.packetize(&[0; 4], i, i).unwrap();
        }
        assert_eq!(sender.cached_packets(), 512);
        assert!(sender.retransmit(&request(0, 1)).is_empty());
        assert_eq!(sender.retransmit(&request(512, 1)).len(), 1);
        assert_eq!(sender.retransmit(&request(1, u16::MAX)).len(), 512);
        assert!(sender.retransmit(&request(1, 0)).is_empty());
    }
    #[test]
    fn malformed_control_packets_do_not_trigger_retry() {
        let mut sender = sender(7);
        sender.packetize(&[0; 4], 0, 1).unwrap();
        for len in 0..8 {
            assert!(sender.retransmit(&request(7, 1)[..len]).is_empty());
        }
        let mut p = request(7, 1);
        p[0] = 0;
        assert!(sender.retransmit(&p).is_empty());
        p = request(7, 1);
        p[1] = 0x54;
        assert!(sender.retransmit(&p).is_empty());
        p = request(7, 1);
        p[1] = 0xd5;
        assert_eq!(sender.retransmit(&p).len(), 1);
    }
    #[test]
    fn invalid_input_does_not_advance_sequence_marker_nonce_or_origin() {
        let mut sender = sender(3);
        for pcm in [vec![], vec![0; 3], vec![0; 1412]] {
            assert!(matches!(
                sender.packetize(&pcm, 10, 1),
                Err(PacketizeError::InvalidPcmLength)
            ));
        }
        assert!(matches!(
            sender.packetize(&[0; 4], u64::MAX, 1),
            Err(PacketizeError::FrameIndexOverflow)
        ));
        assert_eq!(sender.next_sequence(), 3);
        let a = sender.packetize(&[0; 8], 10, 1).unwrap();
        assert!(a.header.marker);
        assert_eq!(a.header.timestamp, 44_100);
        assert!(matches!(
            sender.packetize(&[0; 4], 11, 1),
            Err(PacketizeError::NonMonotonicFrameIndex)
        ));
        let b = sender.packetize(&[0; 4], 12, 2).unwrap();
        assert_eq!(b.header.seq, 4);
        assert_eq!(&b.audio[b.audio.len() - 8..], &1u64.to_le_bytes());
    }
    #[test]
    fn nonce_exhaustion_is_terminal_but_cached_retry_still_works() {
        let mut sender = sender(7);
        sender.next_nonce = Some(u64::MAX);
        let last = sender.packetize(&[0; 4], 0, 1).unwrap();
        assert_eq!(&last.audio[last.audio.len() - 8..], &u64::MAX.to_le_bytes());
        assert!(matches!(
            sender.packetize(&[0; 4], 1, 2),
            Err(PacketizeError::NonceExhausted)
        ));
        assert_eq!(sender.next_sequence(), 8);
        assert_eq!(sender.retransmit(&request(7, 1)).len(), 1);
    }
    #[test]
    fn sync_wire_fixture_and_wrapping_latency() {
        assert_eq!(
            sync_packet(0x11223344, 4, 0x0102030405060708, true),
            [
                0x90, 0xd4, 0, 7, 0x11, 0x22, 0x33, 0x40, 1, 2, 3, 4, 5, 6, 7, 8, 0x11, 0x22, 0x33,
                0x44
            ]
        );
        let normal = sync_packet(1, 2, 0, false);
        assert_eq!(&normal[..4], &[0x80, 0xd4, 0, 7]);
        assert_eq!(&normal[4..8], &u32::MAX.to_be_bytes());
    }
    #[test]
    fn targets_joining_at_different_frames_keep_the_same_presentation_epoch() {
        let mut early = sender(0);
        let mut late = sender(99);
        early.packetize(&[0; 4], 0, 100u64 << 32).unwrap();
        late.packetize(&[0; 4], 44_100, 101u64 << 32).unwrap();
        let a = early.packetize(&[0; 4], 88_200, 102u64 << 32).unwrap();
        let b = late.packetize(&[0; 4], 88_200, 102u64 << 32).unwrap();
        assert_ne!(
            a.header.timestamp, b.header.timestamp,
            "each target has its own RTP origin"
        );
        for p in [a, b] {
            let source = u32::from_be_bytes(p.sync[4..8].try_into().unwrap());
            let ntp = u64::from_be_bytes(p.sync[8..16].try_into().unwrap());
            let latency = p.header.timestamp.wrapping_sub(source);
            let presentation = ntp + (((u64::from(latency)) << 32) / 44_100);
            assert_eq!(
                presentation,
                103u64 << 32,
                "shared NTP producer epoch + one second latency"
            );
        }
    }
}
