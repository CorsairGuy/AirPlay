//! Decode MRP output-context requests into the coordinator's protocol-independent
//! selection model.
//! Selection precedence follows Station outputs::selection, without Station IPC,
//! model filters or its old eight-device limit.
use ::discovery::catalog::OutputCatalog;
use anyhow::Result;
use mrp::{output_context::Request, wire::Node};

/// `local_ids` includes the local device and current local group identifiers;
/// these remain local rather than creating a loopback outgoing session.
pub fn decode(body: &Node) -> Result<::coordinator::selection::Request> {
    let request = Request::parse(body)?;
    Ok(::coordinator::selection::Request {
        devices: changes(request.devices),
        groups: changes(request.clusters),
    })
}

pub fn selection(
    body: &Node,
    catalog: &OutputCatalog,
    local_ids: &[String],
    current: &[String],
) -> Result<Vec<String>> {
    ::coordinator::selection::resolve(&decode(body)?, catalog, local_ids, current)
}

fn changes(changes: mrp::output_context::Changes) -> ::coordinator::selection::Changes {
    ::coordinator::selection::Changes {
        adding: changes.adding,
        removing: changes.removing,
        setting: changes.setting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::discovery::catalog::{OutputRoute, ReceiverEndpoint};
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
}
