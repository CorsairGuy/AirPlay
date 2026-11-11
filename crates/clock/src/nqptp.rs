//! Optional NQPTP shared-memory codec, ported from Station's Claude branch
//! a538a30, airplay_sender/src/ptp.rs. No paths, mmap, sockets or commands.
//!
//! Native-endian, 8-byte-aligned ABI10/11 samples can feed an application clock adapter. Parsing
//! does NOT prove sender-master patch support or authorize an `M` command to a
//! daemon: an unpatched receiver must never get that command. The live reader
//! still owns memory ordering, sample freshness, epoch/role qualification and
//! conversion into the same clock domain used for media anchors.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub version: u16,
    pub clock_id: u64,
    pub local_time: u64,
    /// Modular local-to-master offset, retaining the signed two's-complement
    /// bit pattern. This is not an unsigned elapsed duration.
    pub offset: u64,
    pub clock_change_offset: Option<u64>,
    pub master_start_time: u64,
}
impl Snapshot {
    /// Parse a captured buffer only when both published copies match. Version11
    /// inserts clock_change_offset before master_start_time; do not mix layouts.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let version = u16::from_ne_bytes(bytes.get(..2)?.try_into().ok()?);
        let fields = match version {
            10 => 4,
            11 => 5,
            _ => return None,
        };
        let width = fields * 8;
        if bytes.get(8..8 + width)? != bytes.get(8 + width..8 + 2 * width)? {
            return None;
        }
        let field = |index: usize| -> Option<u64> {
            Some(u64::from_ne_bytes(
                bytes.get(8 + index * 8..16 + index * 8)?.try_into().ok()?,
            ))
        };
        let clock_id = field(0)?;
        if clock_id == 0 {
            return None;
        }
        Some(Self {
            version,
            clock_id,
            local_time: field(1)?,
            offset: field(2)?,
            clock_change_offset: if version == 11 { Some(field(3)?) } else { None },
            master_start_time: field(fields - 1)?,
        })
    }
    /// Same modular mapping as the source adapter. Caller must first qualify
    /// freshness/domain/epoch; this alone is not a valid live-clock reading.
    pub fn master_time(self, local_raw_ns: u64) -> u64 {
        local_raw_ns.wrapping_add(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(version: u16, fields: &[u64]) -> Vec<u8> {
        let mut bytes = vec![0; 8 + fields.len() * 16];
        bytes[..2].copy_from_slice(&version.to_ne_bytes());
        for copy in 0..2 {
            for (i, value) in fields.iter().enumerate() {
                let at = 8 + (copy * fields.len() + i) * 8;
                bytes[at..at + 8].copy_from_slice(&value.to_ne_bytes());
            }
        }
        bytes
    }
    #[test]
    fn both_abis_preserve_full_width_clock_and_correct_field_positions() {
        let id = 0xf842885f71750008;
        let old = Snapshot::parse(&sample(10, &[id, 100, 17, 50])).unwrap();
        let new = Snapshot::parse(&sample(11, &[id, 100, 17, 9, 50])).unwrap();
        assert_eq!(old.clock_id, id);
        assert_eq!(new.clock_id, id);
        assert_eq!(old.master_start_time, 50);
        assert_eq!(new.master_start_time, 50);
        assert_eq!(old.clock_change_offset, None);
        assert_eq!(new.clock_change_offset, Some(9));
        assert_eq!(old.master_time(100), 117);
    }
    #[test]
    fn torn_truncated_unknown_and_unpublished_samples_are_rejected() {
        let bytes = sample(11, &[1, 100, 17, 9, 50]);
        for end in 0..bytes.len() {
            assert!(Snapshot::parse(&bytes[..end]).is_none());
        }
        let mut torn = bytes.clone();
        let last = torn.len() - 1;
        torn[last] ^= 1;
        assert!(Snapshot::parse(&torn).is_none());
        assert!(Snapshot::parse(&sample(12, &[1, 100, 17, 9, 50])).is_none());
        assert!(Snapshot::parse(&sample(10, &[0, 100, 17, 50])).is_none());
    }
    #[test]
    fn negative_offsets_keep_modular_mapping_instead_of_saturating() {
        let snapshot = Snapshot::parse(&sample(10, &[1, 100, (-17_i64) as u64, 50])).unwrap();
        assert_eq!(snapshot.master_time(100), 83);
    }
}
