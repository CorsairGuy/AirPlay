//! Card output requests -> logical catalog -> prepared sender session changes.
//! Selection precedence follows Station outputs::selection, without Station IPC,
//! model filters or its old eight-device limit. The owner serializes requests
//! and publishes confirmed topology after inspecting the returned runtime result.
use anyhow::Result;
use mrp::{output_context::Request, wire::Node};
use sender::{
    Session,
    outputs::OutputCatalog,
    pool::{SelectionResult, SessionPool},
};
use std::collections::BTreeSet;

fn same_uid(a: &str, b: &str) -> bool {
    if a.len() == 36 && b.len() == 36 {
        if let (Ok(a), Ok(b)) = (uuid::Uuid::parse_str(a), uuid::Uuid::parse_str(b)) {
            return a == b;
        }
    }
    a == b
}

/// `local_ids` includes the local device and current local group identifiers;
/// these remain local rather than creating a loopback outgoing session.
/// Resolve both identifier namespaces through discovery's explicit aliases,
/// never by assuming a cluster ID is a receiver address.
pub fn selection(
    body: &Node,
    catalog: &OutputCatalog,
    local_ids: &[String],
    current: &[String],
) -> Result<Vec<String>> {
    let request = Request::parse(body)?;
    let canonical = |ids: &[String]| -> Result<Vec<String>> {
        ids.iter()
            .filter(|id| !local_ids.iter().any(|local| same_uid(local, id)))
            .map(|id| Ok(catalog.resolve(id)?.uid.clone()))
            .collect()
    };
    // Validate every referenced remote UID before changing anything, including
    // remove requests and add entries later overridden by remove.
    let adding = canonical(&request.devices.adding)?;
    let cluster_adding = canonical(&request.clusters.adding)?;
    let removing = canonical(&request.devices.removing)?;
    let cluster_removing = canonical(&request.clusters.removing)?;
    let setting = canonical(&request.devices.setting)?;
    let cluster_setting = canonical(&request.clusters.setting)?;
    // Presence matters: setting only the local device explicitly clears remote
    // outputs. An absent repeated setting field is not an empty reset request.
    let replacing = !request.devices.setting.is_empty() || !request.clusters.setting.is_empty();
    let mut picked: BTreeSet<String> = if replacing {
        setting.into_iter().chain(cluster_setting).collect()
    } else {
        canonical(current)?.into_iter().collect()
    };
    picked.extend(adding.into_iter().chain(cluster_adding));
    for id in removing.into_iter().chain(cluster_removing) {
        picked.remove(&id);
    }
    let desired: Vec<_> = picked.into_iter().collect();
    catalog.receivers(&desired)?; // Enforce physical identity/capacity invariants.
    Ok(desired)
}

/// Caller authenticates additions first (using route_pairing/SenderStore as
/// required). Dispatch this on a selection worker so bounded session teardown
/// does not stall the MRP reader. This helper does not perform PIN pairing.
/// The owner must not acknowledge a requested list as audible/adopted topology.
pub async fn apply_request(
    pool: &mut SessionPool,
    body: &Node,
    catalog: &OutputCatalog,
    local_ids: &[String],
    current: &[String],
    prepared: Vec<(String, Session)>,
) -> Result<SelectionResult> {
    let desired = selection(body, catalog, local_ids, current)?;
    pool.apply_selection(catalog, &desired, prepared).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sender::outputs::{OutputRoute, ReceiverEndpoint};
    fn catalog() -> OutputCatalog {
        OutputCatalog::new(
            (0..2)
                .map(|i| OutputRoute {
                    uid: format!("row{i}"),
                    name: format!("speaker{i}"),
                    model: "".into(),
                    aliases: vec![format!("cluster{i}")],
                    receivers: vec![ReceiverEndpoint {
                        uid: format!("physical{i}"),
                        address: ([127, 0, 0, 1], 7000 + i).into(),
                    }],
                })
                .collect(),
        )
        .unwrap()
    }
    #[test]
    fn cluster_aliases_and_device_rows_share_station_precedence() {
        let catalog = catalog();
        let mut body = Node::default();
        body.text(5, "cluster1").text(2, "row0").text(3, "row0");
        assert_eq!(
            selection(&body, &catalog, &[], &["row0".into()]).unwrap(),
            vec!["row1"]
        );
        let mut body = Node::default();
        body.text(7, "cluster0").text(2, "row1");
        assert_eq!(
            selection(&body, &catalog, &[], &[]).unwrap(),
            vec!["row0", "row1"]
        );
    }
    #[test]
    fn explicit_local_only_setting_clears_remotes_but_empty_request_keeps_them() {
        let catalog = catalog();
        let current = vec!["row0".into()];
        let local = vec!["local".into()];
        assert_eq!(
            selection(&Node::default(), &catalog, &local, &current).unwrap(),
            current
        );
        assert!(
            selection(Node::default().text(4, "local"), &catalog, &local, &current)
                .unwrap()
                .is_empty()
        );
        assert!(
            selection(
                Node::default().text(3, "missing"),
                &catalog,
                &local,
                &current
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn invalid_request_or_missing_prepared_session_does_not_modify_pool() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let mut pool = SessionPool::new(audio);
        let catalog = catalog();
        assert!(
            apply_request(
                &mut pool,
                Node::default().text(5, "cluster0"),
                &catalog,
                &[],
                &[],
                vec![]
            )
            .await
            .is_err()
        );
        assert!(pool.is_empty());
        let result = apply_request(
            &mut pool,
            Node::default().text(4, "local"),
            &catalog,
            &["local".into()],
            &[],
            vec![],
        )
        .await
        .unwrap();
        assert!(result.requested.is_empty() && result.started.is_empty());
    }
}
