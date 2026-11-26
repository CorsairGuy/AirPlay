//! Independent wire vectors for the PTP profile the core sends and answers.
//!
//! Every byte here is written out by hand from IEEE 1588-2019 clause 13
//! (34-byte common header, `sourcePortIdentity` at 20, `sequenceId` at 30,
//! `controlField` at 32, `logMessageInterval` at 33) and from the Apple unicast
//! negotiation TLVs (`0x0004` REQUEST_UNICAST_TRANSMISSION,
//! `0x0005` GRANT_UNICAST_TRANSMISSION, 12-byte grant). Nothing is produced by
//! the code under test, so a change in that code cannot silently rewrite the
//! expectation.
//!
//! A captured exchange with a real iPhone/Apple TV is still missing: these
//! vectors prove the implemented encoder/decoder match the documented layout,
//! not that the device speaks it.

use clock::domain::Domain;
use clock::ptp_sender::Master;
use std::net::IpAddr;

const PEER: &str = "192.0.2.1";
const MASTER_CLOCK: u64 = 0x0102_0304_0506_0708;
const MASTER_PORT: u16 = 0x8005;
const FOREIGN_CLOCK: u64 = 0x1122_3344_5566_7788;
const FOREIGN_PORT: u16 = 0x8006;

fn peer() -> IpAddr {
    PEER.parse().unwrap()
}

fn master() -> Master {
    let mut master = Master::new(Domain::new(MASTER_CLOCK, 1).unwrap()).unwrap();
    master.set_peers(&[peer()]).unwrap();
    master
}

fn domain() -> Domain {
    Domain::new(MASTER_CLOCK, 1).unwrap()
}

/// Common header written by hand, not by the encoder under test.
fn header(kind: u8, len: u16, clock: u64, port: u16, seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; usize::from(len)];
    packet[0] = 0x10 | kind; // transportSpecific 1, messageType
    packet[1] = 2; // versionPTP
    packet[2..4].copy_from_slice(&len.to_be_bytes());
    // domainNumber stays 0: Apple's profile rejects anything else.
    packet[20..28].copy_from_slice(&clock.to_be_bytes());
    packet[28..30].copy_from_slice(&port.to_be_bytes());
    packet[30..32].copy_from_slice(&seq.to_be_bytes());
    packet
}

fn identity(clock: u64, port: u16) -> Vec<u8> {
    let mut bytes = clock.to_be_bytes().to_vec();
    bytes.extend_from_slice(&port.to_be_bytes());
    bytes
}

#[test]
fn delay_request_gets_a_two_step_response_echoing_the_requester() {
    let mut master = master();
    let request = header(1, 44, FOREIGN_CLOCK, FOREIGN_PORT, 0x0042);
    assert_eq!(request.len(), 44);
    let ingress_ns = 1_700_000_000_123_456_789;
    let reply = master
        .receive(peer(), &request, domain().reading(ingress_ns))
        .expect("a well-formed Delay_Req is answered");
    assert_eq!(reply.port, 320);
    let bytes = reply.bytes;
    assert_eq!(bytes.len(), 54);
    assert_eq!(bytes[0], 0x19, "Delay_Resp, transportSpecific 1");
    assert_eq!(bytes[1], 2);
    assert_eq!(&bytes[2..4], &54u16.to_be_bytes());
    assert_eq!(bytes[4], 0, "domain 0");
    // 0x0400 unicast, 0x0200 twoStepFlag, 0x0008 ptpTimescale.
    assert_eq!(&bytes[6..8], &0x0608u16.to_be_bytes(), "flags");
    assert_eq!(&bytes[20..28], &MASTER_CLOCK.to_be_bytes());
    assert_eq!(&bytes[28..30], &MASTER_PORT.to_be_bytes());
    assert_eq!(&bytes[30..32], &0x0042u16.to_be_bytes(), "same sequenceId");
    assert_eq!(bytes[32], 0, "control field");
    assert_eq!(bytes[33], 0xfd, "logMessageInterval -3");
    let seconds = 1_700_000_000u64.to_be_bytes();
    assert_eq!(&bytes[34..40], &seconds[2..], "receiveTimestamp seconds");
    assert_eq!(
        &bytes[40..44],
        &123_456_789u32.to_be_bytes(),
        "receiveTimestamp nanoseconds"
    );
    assert_eq!(
        &bytes[44..54],
        &identity(FOREIGN_CLOCK, FOREIGN_PORT)[..],
        "requestingPortIdentity is echoed"
    );
}

#[test]
fn announce_dataset_fields_are_read_at_the_documented_offsets() {
    let mut packet = header(0xb, 76, FOREIGN_CLOCK, FOREIGN_PORT, 0x0007);
    packet[6..8].copy_from_slice(&0x0408u16.to_be_bytes());
    // 34..44 originTimestamp, 44..46 currentUtcOffset, 46 reserved.
    packet[47] = 0x41; // grandmasterPriority1
    packet[48] = 0x06; // grandmasterClockClass
    packet[49] = 0x21; // grandmasterClockAccuracy
    packet[50..52].copy_from_slice(&0x436au16.to_be_bytes());
    packet[52] = 0x7f; // grandmasterPriority2
    packet[53..61].copy_from_slice(&0x0000_1011_1213_1415u64.to_be_bytes());
    packet[61..63].copy_from_slice(&7u16.to_be_bytes());
    packet[63] = 0x20; // timeSource
    // The profile appends a 12-byte vendor extension; the parser must ignore it.
    packet[64..68].copy_from_slice(&[0, 8, 0, 8]);

    let dataset =
        clock::role::Dataset::from_announce(&packet).expect("64..=512 bytes are accepted");
    assert_eq!(dataset.priority1, 0x41);
    assert_eq!(dataset.clock_class, 0x06);
    assert_eq!(dataset.accuracy, 0x21);
    assert_eq!(dataset.variance, 0x436a);
    assert_eq!(dataset.priority2, 0x7f);
    assert_eq!(dataset.grandmaster, 0x0000_1011_1213_1415);
    assert_eq!(dataset.steps_removed, 7);
    assert_eq!(dataset.source_clock, FOREIGN_CLOCK);
    assert_eq!(dataset.source_port, FOREIGN_PORT);

    // A plain 64-byte Announce without the extension is equally valid; its
    // length field must agree with the buffer.
    let mut short = packet[..64].to_vec();
    short[2..4].copy_from_slice(&64u16.to_be_bytes());
    assert!(clock::role::Dataset::from_announce(&short).is_some());
}

#[test]
fn announce_rejections_are_exact() {
    let base = || {
        let mut packet = header(0xb, 76, FOREIGN_CLOCK, FOREIGN_PORT, 1);
        packet[47] = 128;
        packet[53..61].copy_from_slice(&FOREIGN_CLOCK.to_be_bytes());
        packet
    };
    assert!(clock::role::Dataset::from_announce(&base()).is_some());

    let mut wrong_domain = base();
    wrong_domain[4] = 1;
    assert!(clock::role::Dataset::from_announce(&wrong_domain).is_none());

    let mut wrong_type = base();
    wrong_type[0] = 0x0b; // no transportSpecific nibble
    assert!(clock::role::Dataset::from_announce(&wrong_type).is_none());

    let mut wrong_length = base();
    wrong_length[2..4].copy_from_slice(&72u16.to_be_bytes());
    assert!(clock::role::Dataset::from_announce(&wrong_length).is_none());

    let mut zero_grandmaster = base();
    zero_grandmaster[53..61].copy_from_slice(&[0; 8]);
    assert!(clock::role::Dataset::from_announce(&zero_grandmaster).is_none());

    let mut zero_port = base();
    zero_port[28..30].copy_from_slice(&[0; 2]);
    assert!(clock::role::Dataset::from_announce(&zero_port).is_none());

    let mut too_many_steps = base();
    too_many_steps[61..63].copy_from_slice(&255u16.to_be_bytes());
    assert!(clock::role::Dataset::from_announce(&too_many_steps).is_none());

    assert!(
        clock::role::Dataset::from_announce(&base()[..63]).is_none(),
        "shorter than the smallest Announce"
    );
}

/// Apple unicast negotiation request: common header, target identity, then
/// one 4-byte TLV header plus a 6-byte value.
fn signaling_request(kind: u8, interval: i8, duration: u32, target: [u8; 10]) -> Vec<u8> {
    let mut packet = header(0xc, 54, FOREIGN_CLOCK, FOREIGN_PORT, 0x0011);
    packet[34..44].copy_from_slice(&target);
    packet[44..48].copy_from_slice(&[0, 4, 0, 6]);
    packet[48] = kind << 4;
    packet[49] = interval as u8;
    packet[50..54].copy_from_slice(&duration.to_be_bytes());
    packet
}

#[test]
fn signaling_grants_the_two_implemented_cadences() {
    let mut master = master();
    let request = signaling_request(0xb, 0, 300, [0; 10]);
    let reply = master
        .receive(peer(), &request, domain().reading(1))
        .expect("Announce cadence is granted");
    let bytes = reply.bytes;
    assert_eq!(reply.port, 320);
    assert_eq!(bytes.len(), 56, "44 + one 12-byte grant");
    assert_eq!(bytes[0], 0x1c, "Signaling");
    assert_eq!(bytes[1], 2);
    assert_eq!(&bytes[2..4], &56u16.to_be_bytes());
    assert_eq!(&bytes[6..8], &0x0400u16.to_be_bytes(), "unicast only");
    assert_eq!(&bytes[20..28], &MASTER_CLOCK.to_be_bytes());
    assert_eq!(&bytes[30..32], &0x0011u16.to_be_bytes(), "same sequenceId");
    assert_eq!(bytes[32], 5, "control field for Signaling");
    assert_eq!(&bytes[33..34], &[0x7f], "logMessageInterval not applicable");
    assert_eq!(
        &bytes[34..44],
        &identity(FOREIGN_CLOCK, FOREIGN_PORT)[..],
        "grant targets the requester"
    );
    assert_eq!(&bytes[44..50], &[0, 5, 0, 8, 0xb0, 0], "Announce every 1 s");
    assert_eq!(&bytes[50..54], &300u32.to_be_bytes(), "granted duration");
    assert_eq!(&bytes[54..56], &[0, 1], "reserved + renew flag");
}

#[test]
fn signaling_caps_the_duration_and_grants_sync_at_minus_three() {
    let mut master = master();
    let request = signaling_request(0, -3, 5000, [0xff; 10]);
    let bytes = master
        .receive(peer(), &request, domain().reading(1))
        .expect("broadcast target is accepted")
        .bytes;
    assert_eq!(&bytes[44..50], &[0, 5, 0, 8, 0x00, 0xfd], "Sync at 2^-3 s");
    assert_eq!(&bytes[50..54], &300u32.to_be_bytes(), "duration is capped");
}

#[test]
fn signaling_rejections_do_not_invent_a_grant() {
    let mut master = master();
    let reading = domain().reading(1);
    // Cadence this master does not emit.
    assert!(
        master
            .receive(peer(), &signaling_request(0xb, -3, 300, [0; 10]), reading)
            .is_none()
    );
    // Zero duration is not a grant.
    assert!(
        master
            .receive(peer(), &signaling_request(0xb, 0, 0, [0; 10]), reading)
            .is_none()
    );
    // Another clock's identity is not a valid target.
    let foreign = identity(0x9999, 1).try_into().unwrap();
    assert!(
        master
            .receive(peer(), &signaling_request(0xb, 0, 300, foreign), reading)
            .is_none()
    );
    // Truncated header and odd TLV length.
    assert!(master.receive(peer(), &[0x1c, 2, 0, 40], reading).is_none());
    let mut odd = signaling_request(0xb, 0, 300, [0; 10]);
    odd[46..48].copy_from_slice(&5u16.to_be_bytes());
    assert!(master.receive(peer(), &odd, reading).is_none());
    // Own clock identity is never treated as a foreign requester.
    let mut own = signaling_request(0xb, 0, 300, [0; 10]);
    own[20..28].copy_from_slice(&MASTER_CLOCK.to_be_bytes());
    assert!(master.receive(peer(), &own, reading).is_none());
}
