//! Pure projection of running logical selections and owner-verified descriptors.
//!
//! A worker Snapshot proves only that required session tasks are running. A
//! catalog proves only discovery identity. Neither proves native group adoption,
//! supplies observed volume, or authorizes inventing group/cluster descriptors.
//! The host must obtain those independently before calling this function and
//! publishing its result through Card::set_outputs. This module starts no task.
use anyhow::{Context, Result, bail};
use mrp::{schema::field::AVOutputDeviceDescriptor as fields, wire::Node};
use sender::outputs::OutputCatalog;
use std::collections::{BTreeMap, BTreeSet};

use crate::output_worker::Snapshot;

fn key(uid: &str) -> String {
    uuid::Uuid::parse_str(uid)
        .ok()
        .filter(|_| uid.len() == 36)
        .map(|uid| uid.to_string())
        .unwrap_or_else(|| uid.to_owned())
}

/// Intersect a confirmed logical selection with descriptors independently
/// verified by the output owner. The owner must pin the catalog consistently
/// with the worker's snapshot; stale/missing identities reject the projection.
///
/// Input descriptors identify canonical logical catalog rows, not aliases or
/// arbitrary physical endpoints. Aliases are accepted only in confirmed selection.
/// A logical group needs its own observed row;
/// its member descriptors must not be flattened into invented group membership.
///
/// Preserve original fields, including group/cluster IDs and optional volume.
/// Missing volume remains missing. Unknown/unselected valid descriptors are
/// retained by the caller, not published here. Duplicate row identities reject
/// the entire batch. Stable UID sorting prevents input order changing topology.
/// No card mutation occurs on failure; caller should retain its last valid view.
pub fn project(
    catalog: &OutputCatalog,
    confirmed: &Snapshot,
    verified: &[Node],
) -> Result<Vec<Node>> {
    if confirmed.selected.len() > 64 || verified.len() > 64 {
        bail!("too many card output rows");
    }
    // Validate physical identity conflicts even though descriptors stay logical.
    catalog.receivers(&confirmed.selected)?;
    let mut selected = BTreeSet::new();
    for id in &confirmed.selected {
        let route = catalog.resolve(id)?;
        if !selected.insert(key(&route.uid)) {
            bail!("duplicate confirmed output row");
        }
    }
    let mut descriptors = BTreeMap::new();
    for descriptor in verified {
        descriptor.validate("AVOutputDeviceDescriptor")?;
        let uid = descriptor
            .string(fields::uniqueIdentifier)
            .context("verified descriptor has no output UID")?;
        let route = catalog.resolve(uid)?;
        if uid != route.uid {
            bail!("verified descriptor UID is not the canonical catalog identity");
        }
        if descriptor.get(fields::volume).is_some() {
            let value = descriptor.real(fields::volume).unwrap_or(f64::NAN);
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                bail!("invalid verified output volume");
            }
        }
        if descriptors.insert(key(&route.uid), descriptor).is_some() {
            bail!("duplicate verified output row");
        }
    }
    selected
        .into_iter()
        .map(|uid| {
            descriptors
                .get(&uid)
                .map(|descriptor| (**descriptor).clone())
                .context("confirmed output has no verified descriptor")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sender::outputs::{OutputRoute, ReceiverEndpoint};

    fn catalog() -> OutputCatalog {
        OutputCatalog::new(
            (0..2)
                .map(|n| OutputRoute {
                    uid: format!("row{n}"),
                    name: format!("speaker{n}"),
                    model: "arbitrary".into(),
                    aliases: vec![format!("alias{n}")],
                    receivers: vec![ReceiverEndpoint {
                        uid: format!("physical{n}"),
                        address: ([127, 0, 0, 1], 7000 + n).into(),
                    }],
                })
                .collect(),
        )
        .unwrap()
    }
    fn descriptor(uid: &str, volume: Option<f32>) -> Node {
        let mut value = Node::default();
        value
            .text(fields::uniqueIdentifier, uid)
            .text(fields::groupID, "reported-group")
            .text(fields::clusterID, "reported-cluster");
        if let Some(volume) = volume {
            value.float(fields::volume, volume);
        }
        value
    }
    fn snapshot(ids: &[&str]) -> Snapshot {
        Snapshot {
            selected: ids.iter().map(|id| (*id).into()).collect(),
            last_error: None,
        }
    }

    #[test]
    fn stable_projection_preserves_reported_fields_without_fabricating_volume() {
        let catalog = catalog();
        let a = descriptor("row0", None);
        let b = descriptor("row1", Some(0.3));
        let confirmed = snapshot(&["row1", "alias0"]);
        let projected = project(&catalog, &confirmed, &[b.clone(), a.clone()]).unwrap();
        assert_eq!(projected, vec![a.clone(), b.clone()]);
        assert_eq!(
            projected,
            project(&catalog, &snapshot(&["row0", "row1"]), &[a, b]).unwrap()
        );
        assert!(projected[0].get(fields::volume).is_none());
        assert_eq!(projected[1].string(fields::groupID), Some("reported-group"));
        assert_eq!(
            projected[1].string(fields::clusterID),
            Some("reported-cluster")
        );
    }

    #[test]
    fn missing_duplicate_unknown_and_invalid_volume_reject_the_batch() {
        let catalog = catalog();
        let selected = snapshot(&["row0"]);
        assert!(project(&catalog, &selected, &[]).is_err());
        assert!(project(&catalog, &snapshot(&["row0", "alias0"]), &[]).is_err());
        assert!(
            project(
                &catalog,
                &selected,
                &[descriptor("row0", None), descriptor("alias0", None)]
            )
            .is_err()
        );
        assert!(project(&catalog, &selected, &[descriptor("missing", None)]).is_err());
        assert!(project(&catalog, &selected, &[descriptor("row0", Some(f32::NAN))]).is_err());
        assert!(project(&catalog, &snapshot(&["missing"]), &[]).is_err());
    }

    #[test]
    fn descriptor_alias_spelling_cannot_churn_the_same_logical_row() {
        let catalog = catalog();
        let confirmed = snapshot(&["alias0"]);
        assert!(project(&catalog, &confirmed, &[descriptor("alias0", None)]).is_err());
        assert_eq!(
            project(&catalog, &confirmed, &[descriptor("row0", None)]).unwrap(),
            project(&catalog, &snapshot(&["row0"]), &[descriptor("row0", None)]).unwrap()
        );
    }

    #[test]
    fn deselection_withdraws_rows_and_volume_only_changes_preserve_identity() {
        let catalog = catalog();
        let a = descriptor("row0", Some(0.2));
        let b = descriptor("row0", Some(0.4));
        assert!(
            project(&catalog, &snapshot(&[]), &[a.clone()])
                .unwrap()
                .is_empty()
        );
        let mut old = project(&catalog, &snapshot(&["row0"]), &[a])
            .unwrap()
            .remove(0);
        let mut new = project(&catalog, &snapshot(&["row0"]), &[b])
            .unwrap()
            .remove(0);
        old.remove(fields::volume);
        new.remove(fields::volume);
        assert_eq!(old, new);
    }
}
