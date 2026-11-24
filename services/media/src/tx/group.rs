//! Inactive group-coordination primitives. No sockets, playback or discovery.
//!
//! A successful SETUP is session readiness, not evidence of group adoption.
//! Callers supply observations tagged with the connection's Attempt; they must
//! not relabel an old observation as belonging to a replacement connection.
use crate::tx::Identity;
use anyhow::{Result, bail};
use std::net::IpAddr;

const MAX_MEMBERS: usize = 64;

pub use coordinator::group::{ClockDomain, GroupContext};

fn uuid(value: u128) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        value >> 96,
        (value >> 80) & 0xffff,
        (value >> 64) & 0xffff,
        (value >> 48) & 0xffff,
        value & 0xffffffffffff
    )
}

/// Offline PTP initial-SETUP encoder, NOT a PTP engine. The caller must supply
/// a live negotiated clock, event transport and verified sender-role policy.
/// Wire shapes checked against OwnTone, Music Assistant and node-airplay.
pub fn ptp_setup(
    context: &GroupContext,
    identity: &Identity,
    session: u128,
    peer_id: u128,
    addresses: &[IpAddr],
) -> Result<plist::Value> {
    use plist::Value as V;
    let ClockDomain::Ptp(clock) = context.clock() else {
        bail!("PTP context required");
    };
    if session == 0
        || session == context.uuid()
        || peer_id == 0
        || addresses.is_empty()
        || addresses.len() > MAX_MEMBERS
    {
        bail!("session, peer and reachable addresses are required");
    }
    if addresses
        .iter()
        .any(|ip| ip.is_unspecified() || ip.is_multicast())
    {
        bail!("timing peers require unicast addresses");
    }
    if [
        &identity.device_id,
        &identity.name,
        &identity.model,
        &identity.source_version,
    ]
    .iter()
    .any(|s| s.is_empty() || s.len() > 256)
    {
        bail!("invalid sender identity");
    }
    let peer = V::Dictionary(
        [
            ("ID".to_owned(), V::String(uuid(peer_id))),
            ("DeviceType".to_owned(), V::Integer(0.into())),
            // PTP identity occupies all 64 bits. Binary plists encode its signed
            // bit pattern; casting must not clamp high-bit-set Apple clock IDs.
            ("ClockID".to_owned(), V::Integer((clock as i64).into())),
            (
                "SupportsClockPortMatchingOverride".to_owned(),
                V::Boolean(false),
            ),
            ("Addresses".to_owned(), peers(addresses)?),
        ]
        .into_iter()
        .collect::<plist::Dictionary>(),
    );
    Ok(V::Dictionary(
        [
            ("timingProtocol".to_owned(), V::String("PTP".into())),
            ("deviceID".to_owned(), V::String(identity.device_id.clone())),
            (
                "macAddress".to_owned(),
                V::String(identity.device_id.clone()),
            ),
            ("name".to_owned(), V::String(identity.name.clone())),
            ("model".to_owned(), V::String(identity.model.clone())),
            (
                "sourceVersion".to_owned(),
                V::String(identity.source_version.clone()),
            ),
            ("sessionUUID".to_owned(), V::String(uuid(session))),
            ("groupUUID".to_owned(), V::String(uuid(context.uuid()))),
            (
                "groupContainsGroupLeader".to_owned(),
                V::Boolean(context.contains_discoverable_leader()),
            ),
            ("timingPeerInfo".to_owned(), peer.clone()),
            ("timingPeerList".to_owned(), V::Array(vec![peer])),
        ]
        .into_iter()
        .collect::<plist::Dictionary>(),
    ))
}

/// SETPEERS body only: actual transport and receiver acceptance remain external.
pub fn peers(addresses: &[IpAddr]) -> Result<plist::Value> {
    if addresses.is_empty()
        || addresses.len() > MAX_MEMBERS
        || addresses
            .iter()
            .any(|ip| ip.is_unspecified() || ip.is_multicast())
    {
        bail!("invalid timing peer list");
    }
    let mut seen = std::collections::BTreeSet::new();
    Ok(plist::Value::Array(
        addresses
            .iter()
            .filter(|ip| seen.insert(**ip))
            .map(|ip| plist::Value::String(ip.to_string()))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> GroupContext {
        GroupContext::new(123, ClockDomain::Ptp(0xf842885f71750008), true).unwrap()
    }
    #[test]
    fn ptp_wire_roundtrip_keeps_clock_bits_and_session_group_distinct() {
        let identity = Identity {
            device_id: "02:00:00:00:00:01".into(),
            name: "Test Sender".into(),
            model: "TestModel".into(),
            source_version: "1".into(),
        };
        let ips = ["192.0.2.1".parse().unwrap()];
        let value = ptp_setup(&context(), &identity, 456, 789, &ips).unwrap();
        let mut bytes = Vec::new();
        value.to_writer_binary(&mut bytes).unwrap();
        let decoded = plist::Value::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let d = decoded.as_dictionary().unwrap();
        assert_eq!(d["groupUUID"].as_string(), Some(uuid(123).as_str()));
        assert_ne!(d["groupUUID"], d["sessionUUID"]);
        assert!(!d.contains_key("timingPort"));
        assert_eq!(
            d["timingPeerList"].as_array().unwrap()[0],
            d["timingPeerInfo"]
        );
        let clock = d["timingPeerInfo"].as_dictionary().unwrap()["ClockID"]
            .as_signed_integer()
            .unwrap();
        assert_eq!(clock as u64, 0xf842885f71750008);
        assert_eq!(
            peers(&[ips[0], ips[0]]).unwrap().as_array().unwrap().len(),
            1
        );
        assert!(peers(&["0.0.0.0".parse().unwrap()]).is_err());
        assert!(
            ptp_setup(
                &GroupContext::new(1, ClockDomain::Ntp, false).unwrap(),
                &identity,
                2,
                3,
                &ips
            )
            .is_err()
        );
    }
}
