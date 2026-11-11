//! Logical output selection, porting Station's group-row/member distinction.
//! Discovery supplies the catalog. No model allowlist, network or group-adoption
//! claims: resolving a route is not evidence that any receiver joined it.
use anyhow::{Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverEndpoint {
    pub uid: String,
    pub address: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    pub uid: String,
    pub name: String,
    pub model: String,
    /// Discovery must assign a group alias only to its logical route, not all followers.
    pub aliases: Vec<String>,
    pub receivers: Vec<ReceiverEndpoint>,
}

pub struct OutputCatalog {
    routes: Vec<OutputRoute>,
}

fn key(uid: &str) -> String {
    // UUIDs are case-insensitive; arbitrary opaque IDs are not.
    let bytes = uid.as_bytes();
    let uuid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    if uuid {
        uid.to_ascii_lowercase()
    } else {
        uid.to_owned()
    }
}
fn valid_uid(uid: &str) -> bool {
    !uid.is_empty() && uid.len() <= 256 && !uid.chars().any(char::is_control)
}

impl OutputCatalog {
    pub fn new(routes: Vec<OutputRoute>) -> Result<Self> {
        if routes.len() > 64 {
            bail!("too many logical outputs");
        }
        let mut ids = BTreeSet::new();
        for route in &routes {
            if !valid_uid(&route.uid)
                || !ids.insert(key(&route.uid))
                || route.aliases.len() > 64
                || route.aliases.iter().any(|id| !valid_uid(id))
                || route.name.is_empty()
                || route.name.len() > 256
                || route.model.len() > 256
                || route.receivers.is_empty()
                || route.receivers.len() > 64
            {
                bail!("invalid logical output catalog");
            }
            for receiver in &route.receivers {
                if !valid_uid(&receiver.uid)
                    || receiver.address.port() == 0
                    || receiver.address.ip().is_unspecified()
                    || receiver.address.ip().is_multicast()
                {
                    bail!("invalid output receiver");
                }
            }
        }
        Ok(Self { routes })
    }
    pub fn routes(&self) -> &[OutputRoute] {
        &self.routes
    }
    pub fn receiver(&self, uid: &str) -> Result<&ReceiverEndpoint> {
        if !valid_uid(uid) {
            bail!("invalid receiver identifier");
        }
        let uid = key(uid);
        let mut matches = self
            .routes
            .iter()
            .flat_map(|route| &route.receivers)
            .filter(|receiver| key(&receiver.uid) == uid);
        let receiver = matches
            .next()
            .ok_or_else(|| anyhow::anyhow!("unknown receiver identifier"))?;
        if matches.any(|other| other.address != receiver.address) {
            bail!("conflicting receiver addresses");
        }
        Ok(receiver)
    }
    pub fn resolve(&self, uid: &str) -> Result<&OutputRoute> {
        if !valid_uid(uid) {
            bail!("invalid output identifier");
        }
        let uid = key(uid);
        let mut matches = self
            .routes
            .iter()
            .filter(|r| key(&r.uid) == uid || r.aliases.iter().any(|alias| key(alias) == uid));
        let route = matches
            .next()
            .ok_or_else(|| anyhow::anyhow!("unknown output identifier"))?;
        if matches.next().is_some() {
            bail!("ambiguous output identifier");
        }
        Ok(route)
    }
    /// Expand logical rows to unique physical receivers in stable first-selected
    /// order. Shared members do not start duplicate sessions.
    pub fn receivers(&self, selected: &[String]) -> Result<Vec<ReceiverEndpoint>> {
        if selected.len() > 64 {
            bail!("too many selected outputs");
        }
        let mut identities = BTreeMap::new();
        let mut addresses = BTreeMap::new();
        let mut result = Vec::new();
        for uid in selected {
            for receiver in &self.resolve(uid)?.receivers {
                let id = key(&receiver.uid);
                if identities
                    .get(&id)
                    .is_some_and(|address| *address != receiver.address)
                    || addresses
                        .get(&receiver.address)
                        .is_some_and(|old| old != &id)
                {
                    bail!("conflicting output receiver identity");
                }
                if identities.insert(id.clone(), receiver.address).is_none() {
                    addresses.insert(receiver.address, id);
                    result.push(receiver.clone());
                    if result.len() > 64 {
                        bail!("too many physical receivers");
                    }
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(uid: &str, port: u16) -> ReceiverEndpoint {
        ReceiverEndpoint {
            uid: uid.into(),
            address: ([127, 0, 0, 1], port).into(),
        }
    }
    fn route(uid: &str, peers: Vec<ReceiverEndpoint>) -> OutputRoute {
        OutputRoute {
            uid: uid.into(),
            name: uid.into(),
            model: "UnrestrictedAudioReceiver".into(),
            aliases: vec![],
            receivers: peers,
        }
    }
    #[test]
    fn theater_selection_expands_members_and_deduplicates_overlapping_rows() {
        let a = peer("speaker-a", 7001);
        let b = peer("speaker-b", 7002);
        let mut theater = route("theater", vec![a.clone(), b.clone()]);
        theater.aliases.push("cluster-alias".into());
        let catalog =
            OutputCatalog::new(vec![theater, route("standalone-a", vec![a.clone()])]).unwrap();
        assert_eq!(
            catalog
                .receivers(&["cluster-alias".into(), "standalone-a".into()])
                .unwrap(),
            vec![a, b]
        );
        assert!(catalog.receivers(&["unknown".into()]).is_err());
    }
    #[test]
    fn ambiguous_aliases_and_conflicting_physical_identities_are_not_guessed() {
        let mut a = route("a", vec![peer("one", 7001)]);
        a.aliases.push("group".into());
        let mut b = route("b", vec![peer("two", 7002)]);
        b.aliases.push("group".into());
        assert!(
            OutputCatalog::new(vec![a, b])
                .unwrap()
                .resolve("group")
                .is_err()
        );
        let catalog = OutputCatalog::new(vec![
            route("a", vec![peer("one", 7001)]),
            route("b", vec![peer("one", 7002)]),
        ])
        .unwrap();
        assert!(catalog.receivers(&["a".into(), "b".into()]).is_err());
        assert!(catalog.receiver("one").is_err());
        let mut a = route("a", vec![peer("one", 7001)]);
        a.receivers.push(peer("two", 7001));
        assert!(
            OutputCatalog::new(vec![a])
                .unwrap()
                .receivers(&["a".into()])
                .is_err()
        );
    }
    #[test]
    fn uuid_case_is_normalized_but_opaque_ids_and_invalid_routes_are_not() {
        let id = "ABCDEF01-2345-6789-ABCD-EF0123456789";
        let catalog = OutputCatalog::new(vec![
            route(id, vec![peer("one", 7001)]),
            route("Opaque", vec![peer("two", 7002)]),
        ])
        .unwrap();
        assert!(catalog.resolve(&id.to_ascii_lowercase()).is_ok());
        assert!(catalog.resolve("opaque").is_err());
        assert!(OutputCatalog::new(vec![route("empty", vec![])]).is_err());
        assert!(OutputCatalog::new(vec![route("bad", vec![peer("one", 0)])]).is_err());
    }
}
