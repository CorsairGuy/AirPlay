//! AP2 realtime PTP anchor encoding only. Not selected by PcmPacketizer.
//!
//! Field ordering checked against OwnTone rtp_common.c and Music Assistant
//! ap2_client.c (0a58c239). PTP's 28-byte d7 anchor is a different wire shape
//! from NTP's 20-byte d4 packet; do not change only the timingProtocol string.

/// The caller owns frame_1/frame_2 semantics and latency for the negotiated
/// receiver profile. master_ns and clock_id must name the same clock domain;
/// they must come from a frozen media timeline, not the changing queue head.
pub fn ptp_sync_packet(
    frame_1: u32,
    frame_2: u32,
    master_ns: u64,
    clock_id: u64,
    first: bool,
) -> Option<[u8; 28]> {
    if clock_id == 0 {
        return None;
    }
    let mut packet = [0; 28];
    packet[0] = if first { 0x90 } else { 0x80 };
    packet[1] = 0xd7;
    packet[2..4].copy_from_slice(&6u16.to_be_bytes());
    packet[4..8].copy_from_slice(&frame_1.to_be_bytes());
    packet[8..16].copy_from_slice(&master_ns.to_be_bytes());
    packet[16..20].copy_from_slice(&frame_2.to_be_bytes());
    packet[20..28].copy_from_slice(&clock_id.to_be_bytes());
    Some(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_wire_vector_preserves_fields_without_ntp_conversion() {
        assert_eq!(
            ptp_sync_packet(
                0x11223344,
                0x55667788,
                0x0102030405060708,
                0x8899aabbccddeeff,
                true
            )
            .unwrap(),
            [
                0x90, 0xd7, 0, 6, 0x11, 0x22, 0x33, 0x44, 1, 2, 3, 4, 5, 6, 7, 8, 0x55, 0x66, 0x77,
                0x88, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff
            ]
        );
        let regular = ptp_sync_packet(0, u32::MAX, 999_999_999, 1, false).unwrap();
        assert_eq!(regular[0], 0x80);
        assert_eq!(&regular[16..20], &u32::MAX.to_be_bytes());
        assert!(ptp_sync_packet(0, 0, 0, 0, false).is_none());
    }
}
