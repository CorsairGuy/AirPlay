//! Physical receiver operations selected from logical discovery rows.
use anyhow::{Result, bail};
use std::{collections::BTreeMap, net::SocketAddr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverEndpoint {
    pub uid: String,
    pub address: SocketAddr,
}

/// Read-only view of the current discovery snapshot. Implemented by discovery;
/// coordinator owns how user intent is resolved against it.
pub trait OutputSnapshot {
    type Route: Clone;

    fn resolve_uid(&self, uid: &str) -> Result<String>;
    fn route(&self, uid: &str) -> Result<Self::Route>;
    fn receivers(&self, selected: &[String]) -> Result<Vec<ReceiverEndpoint>>;
    fn merge_pinned(&self, routes: Vec<Self::Route>) -> Result<Self>
    where
        Self: Sized;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionPlan {
    pub selected: Vec<String>,
    pub adding: Vec<ReceiverEndpoint>,
    pub removing: Vec<ReceiverEndpoint>,
    pub retained: Vec<ReceiverEndpoint>,
}

fn key(uid: &str) -> String {
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

/// Translate logical rows to physical session changes, preserving shared
/// members and requiring an explicit replacement when an endpoint changes.
pub fn plan<C: OutputSnapshot>(
    catalog: &C,
    current: &[ReceiverEndpoint],
    selected: &[String],
) -> Result<SelectionPlan> {
    let desired = catalog.receivers(selected)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut canonical = Vec::new();
    for id in selected {
        let uid = catalog.resolve_uid(id)?;
        if ids.insert(key(&uid)) {
            canonical.push(uid);
        }
    }
    let mut old = BTreeMap::new();
    let mut addresses = std::collections::BTreeSet::new();
    if current.len() > 64 {
        bail!("too many current receivers");
    }
    for receiver in current {
        if old.insert(key(&receiver.uid), receiver).is_some() || !addresses.insert(receiver.address)
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
            .filter(|receiver| !current.iter().any(|entry| same(receiver, entry)))
            .cloned()
            .collect(),
        removing: current
            .iter()
            .filter(|receiver| !desired.iter().any(|entry| same(receiver, entry)))
            .cloned()
            .collect(),
        retained: current
            .iter()
            .filter(|receiver| desired.iter().any(|entry| same(receiver, entry)))
            .cloned()
            .collect(),
    })
}

/// Expand per-logical-output values without applying side effects.
pub fn expand_route_values<C, T>(
    catalog: &C,
    values: &[(String, T)],
) -> Result<Vec<(ReceiverEndpoint, T)>>
where
    C: OutputSnapshot,
    T: Clone + PartialEq,
{
    let ids: Vec<_> = values.iter().map(|(id, _)| id.clone()).collect();
    catalog.receivers(&ids)?;
    let mut physical = BTreeMap::new();
    for (uid, value) in values {
        for receiver in catalog.receivers(std::slice::from_ref(uid))? {
            if physical
                .insert(receiver.uid.clone(), (receiver, value.clone()))
                .is_some_and(|(_, old)| old != *value)
            {
                bail!("overlapping outputs request different values");
            }
        }
    }
    Ok(physical.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Catalog;
    impl OutputSnapshot for Catalog {
        type Route = String;

        fn resolve_uid(&self, uid: &str) -> Result<String> {
            match uid {
                "row" | "alias" => Ok("row".into()),
                _ => bail!("unknown row"),
            }
        }

        fn route(&self, uid: &str) -> Result<Self::Route> {
            self.resolve_uid(uid)
        }

        fn receivers(&self, selected: &[String]) -> Result<Vec<ReceiverEndpoint>> {
            selected
                .iter()
                .map(|id| {
                    self.resolve_uid(id)?;
                    Ok(ReceiverEndpoint {
                        uid: "speaker".into(),
                        address: ([127, 0, 0, 1], 7000).into(),
                    })
                })
                .collect()
        }

        fn merge_pinned(&self, _: Vec<Self::Route>) -> Result<Self> {
            Ok(Self)
        }
    }

    #[test]
    fn runtime_plan_keeps_shared_members_and_rejects_duplicate_physical_identity() {
        let current = [ReceiverEndpoint {
            uid: "speaker".into(),
            address: ([127, 0, 0, 1], 7000).into(),
        }];
        let selection = plan(&Catalog, &current, &["alias".into()]).unwrap();
        assert_eq!(selection.selected, vec!["row"]);
        assert_eq!(selection.retained, current);
        assert!(selection.adding.is_empty() && selection.removing.is_empty());
        assert!(plan(&Catalog, &[current[0].clone(), current[0].clone()], &[]).is_err());
    }

    #[test]
    fn route_values_expand_and_conflicting_overlaps_are_rejected() {
        let levels = expand_route_values(&Catalog, &[("row".into(), 0.5)]).unwrap();
        assert_eq!(levels[0].1, 0.5);
        assert!(
            expand_route_values(&Catalog, &[("row".into(), 0.5), ("alias".into(), 0.7)]).is_err()
        );
    }
}
