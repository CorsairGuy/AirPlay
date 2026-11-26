//! Read-only decode of the clock NQPTP publishes for a sender grandmaster.
//!
//! Layout is taken from the pinned Station slice (`a1aefc79`,
//! `src/airplay_sender/src/ptp.rs`): a native-endian `u16` structure version,
//! padding to the first 8-byte boundary, then the main and the secondary copy
//! of the same record. A record is `master_clock_id`, `local_time`,
//! `local_to_master_time_offset`, `clock_change_offset` (ABI11 only) and
//! `master_clock_start_time`. Both copies are compared field by field, so a
//! half-published write is rejected instead of being averaged or guessed.
//!
//! This is a decoder and a readiness marker, not an attestation: it cannot
//! prove that the daemon is patched for the grandmaster role, that the sample is
//! fresh, or that a RAW/Instant correlation is valid. Those stay with
//! [`crate::external::Qualifier`], its measured correlation and the owner of the
//! daemon evidence.

use crate::external;

/// NQPTP 1.2.8: four `u64` fields per copy, no `clock_change_offset`.
pub const VERSION_ABI10: u16 = 10;
/// NQPTP 1.2.9: five `u64` fields per copy, `clock_change_offset` before the
/// start marker.
pub const VERSION_ABI11: u16 = 11;

/// Structure version, native endianness.
const VERSION_AT: usize = 0;
/// First 8-byte boundary after the version and its padding.
const COPIES_AT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Buffer ends before a complete pair of copies.
    TooShort,
    /// Version the decoder has no verified layout for.
    UnknownVersion(u16),
    /// Main and secondary copies disagree: torn or half-published write.
    Torn,
    /// `master_clock_id` is zero: the daemon publishes no clock yet.
    Unpublished,
    /// The shared file is missing or unreadable.
    Unavailable,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVersion(version) => write!(f, "unknown NQPTP structure version {version}"),
            Self::TooShort => write!(f, "NQPTP structure is truncated"),
            Self::Torn => write!(f, "NQPTP copies disagree (torn write)"),
            Self::Unpublished => write!(f, "NQPTP publishes no clock"),
            Self::Unavailable => write!(f, "NQPTP structure is unavailable"),
        }
    }
}

impl std::error::Error for Error {}

/// One coherent published record. `clock_id`/`master_start_time` identify the
/// daemon instance and its epoch; freshness and role stay the owner's problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub clock_id: u64,
    pub local_time: u64,
    pub offset: u64,
    pub clock_change_offset: Option<u64>,
    pub master_start_time: u64,
}

impl Sample {
    /// Source-neutral observation for the qualified external-clock path.
    pub fn observation(self) -> external::Snapshot {
        external::Snapshot {
            clock_id: self.clock_id,
            local_time: self.local_time,
            offset: self.offset,
            clock_change_offset: self.clock_change_offset,
            master_start_time: self.master_start_time,
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, Error> {
    let field = bytes.get(at..at + 8).ok_or(Error::TooShort)?;
    Ok(u64::from_ne_bytes(
        field.try_into().map_err(|_| Error::TooShort)?,
    ))
}

/// Structure version, or `None` when the buffer is too short. A version this
/// decoder knows is the only readiness marker it can honestly provide: an
/// unpatched NQPTP never creates the structure with a master interface version.
pub fn version(bytes: &[u8]) -> Option<u16> {
    u16_at(bytes, VERSION_AT)
}

/// Decode one published record from two coherent copies.
pub fn decode(bytes: &[u8]) -> Result<Sample, Error> {
    let version = u16_at(bytes, VERSION_AT).ok_or(Error::TooShort)?;
    let fields = match version {
        VERSION_ABI10 => 4,
        VERSION_ABI11 => 5,
        other => return Err(Error::UnknownVersion(other)),
    };
    let main = COPIES_AT;
    let secondary = COPIES_AT + fields * 8;
    if bytes.len() < secondary + fields * 8 {
        return Err(Error::TooShort);
    }
    for index in 0..fields {
        if u64_at(bytes, main + index * 8)? != u64_at(bytes, secondary + index * 8)? {
            return Err(Error::Torn);
        }
    }
    let clock_id = u64_at(bytes, main)?;
    if clock_id == 0 {
        return Err(Error::Unpublished);
    }
    Ok(Sample {
        clock_id,
        local_time: u64_at(bytes, main + 8)?,
        offset: u64_at(bytes, main + 16)?,
        clock_change_offset: if fields == 5 {
            Some(u64_at(bytes, main + 24)?)
        } else {
            None
        },
        master_start_time: u64_at(bytes, main + (fields - 1) * 8)?,
    })
}

/// Read one published record from `path`.
///
/// Coherence comes from the two copies above, not from the read being atomic:
/// a write that lands between the copies is rejected and must be retried by the
/// owner. The file is opened read-only.
#[cfg(unix)]
pub fn read_path(path: &std::path::Path) -> Result<Sample, Error> {
    let bytes = std::fs::read(path).map_err(|_| Error::Unavailable)?;
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent fixture built from the documented offsets, not from a
    /// captured file.
    fn record(version: u16, fields: &[u64]) -> Vec<u8> {
        let mut bytes = vec![0u8; COPIES_AT + fields.len() * 16];
        bytes[VERSION_AT..VERSION_AT + 2].copy_from_slice(&version.to_ne_bytes());
        for (index, value) in fields.iter().enumerate() {
            let main = COPIES_AT + index * 8;
            bytes[main..main + 8].copy_from_slice(&value.to_ne_bytes());
            let secondary = COPIES_AT + fields.len() * 8 + index * 8;
            bytes[secondary..secondary + 8].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }

    #[test]
    fn abi10_record_maps_every_field_in_order() {
        let bytes = record(VERSION_ABI10, &[7, 100, 5, 900]);
        assert_eq!(version(&bytes), Some(VERSION_ABI10));
        let sample = decode(&bytes).unwrap();
        assert_eq!(
            sample,
            Sample {
                clock_id: 7,
                local_time: 100,
                offset: 5,
                clock_change_offset: None,
                master_start_time: 900,
            }
        );
        assert_eq!(
            sample.observation(),
            external::Snapshot {
                clock_id: 7,
                local_time: 100,
                offset: 5,
                clock_change_offset: None,
                master_start_time: 900,
            }
        );
        assert_eq!(sample.observation().master_time(1000), 1005);
    }

    #[test]
    fn abi11_inserts_clock_change_offset_before_the_start_marker() {
        let bytes = record(VERSION_ABI11, &[7, 100, 5, 42, 900]);
        let sample = decode(&bytes).unwrap();
        assert_eq!(sample.clock_change_offset, Some(42));
        assert_eq!(sample.master_start_time, 900, "start marker stays last");
        assert_eq!(sample.offset, 5);
    }

    #[test]
    fn two_different_copies_are_a_torn_write() {
        let mut bytes = record(VERSION_ABI11, &[7, 100, 5, 42, 900]);
        // Secondary copy of `local_time`.
        let secondary = COPIES_AT + 5 * 8 + 8;
        bytes[secondary..secondary + 8].copy_from_slice(&101u64.to_ne_bytes());
        assert_eq!(decode(&bytes), Err(Error::Torn));
    }

    #[test]
    fn zero_clock_id_is_unpublished_not_a_clock() {
        assert_eq!(
            decode(&record(VERSION_ABI10, &[0, 100, 5, 900])),
            Err(Error::Unpublished)
        );
    }

    #[test]
    fn unknown_and_truncated_structures_are_rejected() {
        assert_eq!(
            decode(&record(9, &[7, 100, 5, 900])),
            Err(Error::UnknownVersion(9))
        );
        let abi11 = record(VERSION_ABI11, &[7, 100, 5, 42, 900]);
        // Version says ABI11, buffer only holds ABI10.
        assert_eq!(decode(&abi11[..72]), Err(Error::TooShort));
        assert_eq!(decode(&[]), Err(Error::TooShort));
        assert_eq!(version(&[1]), None);
        // Trailing bytes after the pair are ignored.
        let mut longer = abi11.clone();
        longer.extend_from_slice(&[0xff; 8]);
        assert_eq!(decode(&longer).unwrap().clock_id, 7);
    }

    #[test]
    fn published_identity_and_marker_key_an_external_selection() {
        use crate::domain::Domain;
        use std::time::Duration;
        let sample = decode(&record(VERSION_ABI11, &[7, 100, 5, 42, 900])).unwrap();
        let observation = sample.observation();
        // The owner still attests daemon role; the decoder supplies only identity,
        // epoch marker and the optional change offset.
        let selection = external::Selection {
            daemon_instance: u128::from(observation.clock_id),
            domain: Domain::new(observation.clock_id, 1).unwrap(),
            master_start_raw_ns: observation.master_start_time,
            start_marker: external::StartMarker::RefreshedPublication,
            clock_change_offset: observation.clock_change_offset,
            sender_master_verified: true,
        };
        assert!(
            external::Qualifier::new(
                selection,
                Duration::from_millis(50),
                Duration::from_micros(500)
            )
            .is_ok()
        );
    }

    #[test]
    fn the_same_layout_is_read_on_little_and_big_endian_hosts() {
        // Native order is what the daemon writes; the fixture writes it the same
        // way, so the test states the assumption instead of hiding it.
        let bytes = record(VERSION_ABI10, &[1, 2, 3, 4]);
        assert_eq!(bytes[8..16], 1u64.to_ne_bytes());
        assert_eq!(decode(&bytes).unwrap().master_start_time, 4);
    }
}
