//! Compatibility facade for the former application-level relay selection API.
pub use ::remote::output_selection::{decode, selection};

use ::coordinator::runtime::Runtime;
use ::discovery::catalog::OutputCatalog;
use ::media::tx::{Session, pool::SessionPool};
use anyhow::Result;
use mrp::wire::Node;

/// Compatibility wrapper. New code should decode under `remote` and let the
/// output worker apply the resulting selection.
pub async fn apply_request(
    pool: &mut SessionPool,
    body: &Node,
    catalog: &OutputCatalog,
    local_ids: &[String],
    current: &[String],
    prepared: Vec<(String, Session)>,
) -> Result<crate::coordinator::output_worker::AppliedSelection> {
    let desired = selection(body, catalog, local_ids, current)?;
    let mut runtime = Runtime::default();
    let plan =
        crate::coordinator::output_worker::plan_selection(pool, &runtime, catalog, &desired)?;
    crate::coordinator::output_worker::apply_selection(&mut runtime, pool, plan, prepared).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> OutputCatalog {
        OutputCatalog::new(vec![::discovery::catalog::OutputRoute {
            uid: "row".into(),
            name: "speaker".into(),
            model: String::new(),
            aliases: vec!["group".into()],
            receivers: vec![::discovery::catalog::ReceiverEndpoint {
                uid: "physical".into(),
                address: ([127, 0, 0, 1], 7000).into(),
            }],
        }])
        .unwrap()
    }

    #[tokio::test]
    async fn invalid_request_or_missing_prepared_session_does_not_modify_pool() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let mut pool = SessionPool::new(audio);
        let catalog = catalog();
        let invalid = Node::default().text(5, "group").clone();
        assert!(
            apply_request(&mut pool, &invalid, &catalog, &[], &[], vec![],)
                .await
                .is_err()
        );
        assert!(pool.is_empty());
        let local = Node::default().text(4, "local").clone();
        let result = apply_request(&mut pool, &local, &catalog, &["local".into()], &[], vec![])
            .await
            .unwrap();
        assert!(result.requested.is_empty() && result.started.is_empty());
    }
}
