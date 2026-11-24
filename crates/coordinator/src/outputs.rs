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

/// A side-effect-free replacement plan. Authenticate additions before applying
/// it; only commit the UI selection after the runtime confirms the changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionPlan {
    pub selected: Vec<String>,
    pub adding: Vec<ReceiverEndpoint>,
    pub removing: Vec<ReceiverEndpoint>,
    pub retained: Vec<ReceiverEndpoint>,
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
    /// Refresh discovery while keeping selected rows pinned to the endpoints
    /// already in use. A transient advertisement loss/address change must not
    /// silently move or tear down an active receiver. Deselection releases pins.
    pub fn preserving_selected(&self, previous: &Self, selected: &[String]) -> Result<Self> {
        let mut pinned = Vec::new();
        let mut seen = BTreeSet::new();
        for id in selected {
            let route = previous.resolve(id)?;
            if seen.insert(key(&route.uid)) {
                pinned.push(route.clone());
            }
        }
        let mut routes: Vec<_> = self
            .routes
            .iter()
            .filter(|r| !seen.contains(&key(&r.uid)))
            .cloned()
            .collect();
        routes.extend(pinned);
        Self::new(routes)
    }
    /// Compare actual runtime endpoints with a complete desired logical selection.
    /// Removing one overlapping row keeps receivers required by another row.
    /// A changed address for the same UID requires an explicit session replacement.
    pub fn plan(&self, current: &[ReceiverEndpoint], selected: &[String]) -> Result<SelectionPlan> {
        let desired = self.receivers(selected)?;
        let mut selected_ids = BTreeSet::new();
        let mut canonical = Vec::new();
        for id in selected {
            let route = self.resolve(id)?;
            if selected_ids.insert(key(&route.uid)) {
                canonical.push(route.uid.clone());
            }
        }
        let mut old = BTreeMap::new();
        let mut addresses = BTreeSet::new();
        if current.len() > 64 {
            bail!("too many current receivers");
        }
        for receiver in current {
            if !valid_uid(&receiver.uid)
                || old.insert(key(&receiver.uid), receiver).is_some()
                || !addresses.insert(receiver.address)
            {
                bail!("ambiguous current receiver identity");
            }
        }
        let same = |a: &ReceiverEndpoint, b: &ReceiverEndpoint| {
            key(&a.uid) == key(&b.uid) && a.address == b.address
        };
        Ok(SelectionPlan {
            selected: canonical,
            adding: desired
                .iter()
                .filter(|r| !current.iter().any(|c| same(r, c)))
                .cloned()
                .collect(),
            removing: current
                .iter()
                .filter(|r| !desired.iter().any(|d| same(r, d)))
                .cloned()
                .collect(),
            // Preserve runtime UID spelling for existing handles.
            retained: current
                .iter()
                .filter(|r| desired.iter().any(|d| same(r, d)))
                .cloned()
                .collect(),
        })
    }
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
    fn discovery_refresh_pins_selected_endpoints_and_releases_on_deselection() {
        let old = OutputCatalog::new(vec![route("active", vec![peer("a", 7001)])]).unwrap();
        let fresh = OutputCatalog::new(vec![
            route("active", vec![peer("a", 7002)]),
            route("new", vec![peer("b", 7003)]),
        ])
        .unwrap();
        let merged = fresh.preserving_selected(&old, &["active".into()]).unwrap();
        assert_eq!(
            merged.resolve("active").unwrap().receivers[0]
                .address
                .port(),
            7001
        );
        assert!(merged.resolve("new").is_ok());
        let missing = OutputCatalog::new(vec![])
            .unwrap()
            .preserving_selected(&old, &["active".into()])
            .unwrap();
        assert!(missing.resolve("active").is_ok());
        let released = fresh.preserving_selected(&merged, &[]).unwrap();
        assert_eq!(
            released.resolve("active").unwrap().receivers[0]
                .address
                .port(),
            7002
        );
    }
    #[test]
    fn selection_delta_retains_shared_members_and_canonicalizes_aliases() {
        let a = peer("a", 7001);
        let b = peer("b", 7002);
        let mut theater = route("theater", vec![a.clone(), b.clone()]);
        theater.aliases.push("alias".into());
        let catalog = OutputCatalog::new(vec![theater, route("single", vec![a.clone()])]).unwrap();
        let plan = catalog
            .plan(&[a.clone(), b.clone()], &["single".into()])
            .unwrap();
        assert_eq!(plan.retained, vec![a.clone()]);
        assert_eq!(plan.removing, vec![b]);
        assert!(plan.adding.is_empty());
        let plan = catalog
            .plan(&[a], &["alias".into(), "theater".into()])
            .unwrap();
        assert_eq!(plan.selected, vec!["theater"]);
        assert_eq!(plan.adding.len(), 1);
        assert!(plan.removing.is_empty());
    }
    #[test]
    fn changed_address_requires_replacement_and_empty_selection_removes_all() {
        let old = peer("a", 7001);
        let new = peer("a", 7002);
        let catalog = OutputCatalog::new(vec![route("single", vec![new.clone()])]).unwrap();
        let plan = catalog
            .plan(std::slice::from_ref(&old), &["single".into()])
            .unwrap();
        assert_eq!(plan.adding, vec![new]);
        assert_eq!(plan.removing, vec![old.clone()]);
        assert!(plan.retained.is_empty());
        assert_eq!(
            catalog
                .plan(std::slice::from_ref(&old), &[])
                .unwrap()
                .removing,
            vec![old.clone()]
        );
        assert!(catalog.plan(&[old.clone(), old], &[]).is_err());
        assert!(catalog.plan(&[], &["missing".into()]).is_err());
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
