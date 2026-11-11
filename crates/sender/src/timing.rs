//! One immutable PTP plan binds SETUP, SETPEERS and d7 media anchors. No I/O.
//! Session transport still defaults to NTP; this plan does not start a clock,
//! negotiate group adoption or establish freshness/role qualification itself.
use crate::{
    Identity,
    group::{self, ClockDomain, GroupContext},
};
use anyhow::{Result, bail};
use clock::domain::{Domain, Reading};
use std::net::IpAddr;

#[derive(Clone, Debug)]
pub struct PtpPlan {
    context: GroupContext,
    domain: Domain,
    peer_id: u128,
    local_addresses: Vec<IpAddr>,
    peers: Vec<IpAddr>,
}
impl PtpPlan {
    /// The role owner supplies one qualified domain/epoch for the whole group.
    /// Local reachable addresses are explicit; the peer list includes the sender,
    /// matching the verified Station SETPEERS path. Never substitute NTP readings.
    pub fn new(
        context: GroupContext,
        domain: Domain,
        peer_id: u128,
        local_addresses: Vec<IpAddr>,
        receivers: Vec<IpAddr>,
    ) -> Result<Self> {
        if context.clock() != ClockDomain::Ptp(domain.id())
            || peer_id == 0
            || local_addresses.is_empty()
            || receivers.is_empty()
        {
            bail!("PTP context/domain, peer identity and reachable addresses are required");
        }
        group::peers(&local_addresses)?;
        group::peers(&receivers)?;
        let mut peers = Vec::new();
        for address in receivers.iter().chain(&local_addresses) {
            if !peers.contains(address) {
                peers.push(*address);
            }
        }
        group::peers(&peers)?;
        Ok(Self {
            context,
            domain,
            peer_id,
            local_addresses,
            peers,
        })
    }
    pub fn domain(&self) -> Domain {
        self.domain
    }
    pub fn group(&self) -> &GroupContext {
        &self.context
    }
    pub fn setup(&self, identity: &Identity, session: u128) -> Result<plist::Value> {
        group::ptp_setup(
            &self.context,
            identity,
            session,
            self.peer_id,
            &self.local_addresses,
        )
    }
    pub fn setpeers(&self) -> Result<plist::Value> {
        group::peers(&self.peers)
    }
    /// `rtp` is the current session's packetized timestamp. `mapping_lead`
    /// already excludes receiver render minimum, as in Station cf1894c.
    /// Reading must map the shared PCM producer instant, not a new send-time read.
    pub fn anchor(
        &self,
        rtp: u32,
        mapping_lead: u32,
        reading: Reading,
        first: bool,
    ) -> Result<[u8; 28]> {
        if reading.domain() != self.domain {
            bail!("PTP media anchor has a different clock or epoch");
        }
        rtp::ptp_sync_packet(
            rtp.wrapping_sub(mapping_lead),
            rtp,
            reading.ns(),
            self.domain.id(),
            first,
        )
        .ok_or_else(|| anyhow::anyhow!("invalid PTP anchor clock"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CLOCK: u64 = 0xf842885f71750008;
    fn plan() -> PtpPlan {
        PtpPlan::new(
            GroupContext::new(123, ClockDomain::Ptp(CLOCK), true).unwrap(),
            Domain::new(CLOCK, 1).unwrap(),
            456,
            vec!["192.0.2.1".parse().unwrap()],
            vec!["192.0.2.2".parse().unwrap(), "192.0.2.2".parse().unwrap()],
        )
        .unwrap()
    }
    #[test]
    fn setup_peers_and_anchor_use_one_clock_and_correct_frame_mapping() {
        let plan = plan();
        let setup = plan
            .setup(&Identity::new("sender", "generic"), 789)
            .unwrap();
        let setup = setup.as_dictionary().unwrap();
        assert_eq!(
            setup.get("timingProtocol").unwrap().as_string(),
            Some("PTP")
        );
        let peer = setup
            .get("timingPeerInfo")
            .unwrap()
            .as_dictionary()
            .unwrap();
        assert_eq!(
            peer.get("ClockID").unwrap().as_signed_integer().unwrap() as u64,
            CLOCK
        );
        let peers = plan.setpeers().unwrap();
        let peers = peers.as_array().unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[1].as_string(), Some("192.0.2.1"));
        let anchor = plan
            .anchor(100_000, 66_150, plan.domain().reading(9_876_543_210), true)
            .unwrap();
        assert_eq!(&anchor[..4], &[0x90, 0xd7, 0, 6]);
        assert_eq!(u32::from_be_bytes(anchor[4..8].try_into().unwrap()), 33_850);
        assert_eq!(
            u32::from_be_bytes(anchor[16..20].try_into().unwrap()),
            100_000
        );
        assert_eq!(
            u64::from_be_bytes(anchor[20..28].try_into().unwrap()),
            CLOCK
        );
    }
    #[test]
    fn wrong_clock_epoch_and_invalid_session_are_rejected() {
        let plan = plan();
        assert!(
            plan.anchor(1, 10, Domain::new(CLOCK, 2).unwrap().reading(100), false)
                .is_err()
        );
        assert!(
            plan.anchor(1, 10, Domain::new(77, 1).unwrap().reading(100), false)
                .is_err()
        );
        assert!(
            plan.setup(&Identity::new("sender", "generic"), 123)
                .is_err()
        );
        let anchor = plan
            .anchor(1, 10, plan.domain().reading(100), false)
            .unwrap();
        assert_eq!(
            u32::from_be_bytes(anchor[4..8].try_into().unwrap()),
            1_u32.wrapping_sub(10)
        );
    }
    #[test]
    fn ntp_context_clock_mismatch_and_nonunicast_peers_fail_before_io() {
        let local = vec!["192.0.2.1".parse().unwrap()];
        let peers = vec!["192.0.2.2".parse().unwrap()];
        let domain = Domain::new(CLOCK, 1).unwrap();
        assert!(
            PtpPlan::new(
                GroupContext::new(123, ClockDomain::Ntp, true).unwrap(),
                domain,
                456,
                local.clone(),
                peers.clone()
            )
            .is_err()
        );
        assert!(
            PtpPlan::new(
                GroupContext::new(123, ClockDomain::Ptp(77), true).unwrap(),
                domain,
                456,
                local.clone(),
                peers
            )
            .is_err()
        );
        assert!(
            PtpPlan::new(
                GroupContext::new(123, ClockDomain::Ptp(CLOCK), true).unwrap(),
                domain,
                456,
                local,
                vec!["224.0.0.1".parse().unwrap()]
            )
            .is_err()
        );
    }
}
