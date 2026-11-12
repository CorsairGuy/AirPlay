//! Explicit authentication policy for output-worker preparation. No discovery,
//! retries or fallback from a failed saved pairing to transient authentication.
use crate::{output_worker::SessionPreparer, sender_store::SenderStore};
use anyhow::{Result, bail};
use sender::{Session, SessionConfig, outputs::ReceiverEndpoint};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationPolicy {
    SavedOnly,
    /// Only a receiver with no saved credentials may use transient pairing.
    AllowTransientForUnpaired,
    /// Unpaired receivers require this request's live controller PIN scope.
    InteractivePinForUnpaired,
}

pub struct StoredSessionPreparer {
    template: SessionConfig,
    store: Arc<SenderStore>,
    policy: AuthenticationPolicy,
}
impl StoredSessionPreparer {
    /// The caller explicitly chooses latency, initial-volume policy and control
    /// deadlines in template. Its endpoint/pairing are replaced for every UID;
    /// credentials for one receiver must never leak into another's connection.
    pub fn new(
        template: SessionConfig,
        store: Arc<SenderStore>,
        policy: AuthenticationPolicy,
    ) -> Self {
        Self {
            template,
            store,
            policy,
        }
    }

    fn config(&self, receiver: &ReceiverEndpoint) -> Result<SessionConfig> {
        let pairing = self.store.get(&receiver.uid)?;
        if pairing.is_none() && self.policy != AuthenticationPolicy::AllowTransientForUnpaired {
            bail!("receiver requires persistent authorization");
        }
        let mut config = self.template.clone();
        config.endpoint = receiver.address;
        config.pairing = pairing;
        Ok(config)
    }
}
impl SessionPreparer for StoredSessionPreparer {
    fn selection(&mut self, receivers: &[ReceiverEndpoint]) -> Result<()> {
        if let Some(plan) = &self.template.ptp {
            // Empty selection creates no sessions. Retain only the immutable
            // clock/group template; the next nonempty selection replaces peers.
            if !receivers.is_empty() {
                self.template.ptp =
                    Some(plan.with_receivers(receivers.iter().map(|r| r.address.ip()).collect())?);
            }
        }
        Ok(())
    }
    async fn prepare(&mut self, receiver: ReceiverEndpoint) -> Result<Session> {
        self.prepare_authorized(receiver, None).await
    }
    async fn prepare_authorized(
        &mut self,
        receiver: ReceiverEndpoint,
        authorization: Option<&crate::route_pairing::AuthorizationContext>,
    ) -> Result<Session> {
        if self.policy == AuthenticationPolicy::InteractivePinForUnpaired
            && self.store.get(&receiver.uid)?.is_none()
        {
            let scope = authorization
                .ok_or_else(|| anyhow::anyhow!("receiver requires controller PIN context"))?
                .begin()?;
            let mut route = mrp::wire::Node::default();
            route
                .text(
                    mrp::schema::field::AVOutputDeviceDescriptor::uniqueIdentifier,
                    &receiver.uid,
                )
                .text(
                    mrp::schema::field::AVOutputDeviceDescriptor::name,
                    &receiver.uid,
                );
            let credentials = scope
                .authorize_and_save(
                    crate::route_pairing::Request {
                        endpoint: receiver.address,
                        controller: self.store.controller(),
                        route,
                        answer_window: std::time::Duration::from_secs(90),
                    },
                    receiver.uid.clone(),
                    self.store.clone(),
                )
                .await?;
            if credentials.is_none() {
                bail!("receiver authorization cancelled");
            }
        }
        // Session::connect handles a single selected authentication path.
        // Dropping this future releases its connection; do not spawn/retry it.
        Session::connect(self.config(&receiver)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("airplay-session-preparer-{}", uuid::Uuid::new_v4())),
            )
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn receiver(uid: &str) -> ReceiverEndpoint {
        ReceiverEndpoint {
            uid: uid.into(),
            address: ([127, 0, 0, 1], 9).into(),
        }
    }
    fn template() -> SessionConfig {
        let mut config = SessionConfig::new(
            ([127, 0, 0, 1], 10).into(),
            sender::Identity::new("controller", "test"),
        );
        config.preserve_receiver_volume = true;
        config
    }
    #[tokio::test]
    async fn interactive_policy_requires_a_live_request_context_before_connecting() {
        let dir = Temp::new();
        let store = Arc::new(SenderStore::open(&dir.0).unwrap());
        let mut preparer = StoredSessionPreparer::new(
            template(),
            store.clone(),
            AuthenticationPolicy::InteractivePinForUnpaired,
        );
        assert!(
            preparer
                .prepare(receiver("missing"))
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("controller PIN context")
        );
        assert!(preparer.config(&receiver("missing")).is_err());
        let (outgoing, mut messages) = tokio::sync::mpsc::channel(1);
        let owner = crate::route_pairing::ControllerAuthorization::new(outgoing);
        let context = owner.context();
        drop(owner);
        assert!(
            preparer
                .prepare_authorized(receiver("missing"), Some(&context))
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("disconnected")
        );
        assert!(messages.try_recv().is_err());
        assert!(store.get("missing").unwrap().is_none());
    }
    #[tokio::test]
    async fn saved_only_rejects_missing_authorization_before_connecting() {
        let dir = Temp::new();
        let store = Arc::new(SenderStore::open(&dir.0).unwrap());
        let mut preparer =
            StoredSessionPreparer::new(template(), store, AuthenticationPolicy::SavedOnly);
        assert!(
            preparer
                .prepare(receiver("missing"))
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("persistent authorization")
        );
    }
    #[test]
    fn selected_receivers_replace_initial_ptp_peers_and_invalid_change_is_atomic() {
        let dir = Temp::new();
        let store = Arc::new(SenderStore::open(&dir.0).unwrap());
        let mut config = template();
        let local = "127.0.0.1".parse().unwrap();
        config.ptp = Some(
            sender::timing::PtpPlan::new(
                sender::group::GroupContext::new(42, sender::group::ClockDomain::Ptp(7), true)
                    .unwrap(),
                clock::domain::Domain::new(7, 1).unwrap(),
                99,
                vec![local],
                vec!["127.0.0.2".parse().unwrap()],
            )
            .unwrap(),
        );
        let mut preparer = StoredSessionPreparer::new(
            config,
            store,
            AuthenticationPolicy::AllowTransientForUnpaired,
        );
        let mut target = receiver("new");
        target.address.set_ip("127.0.0.3".parse().unwrap());
        preparer.selection(&[target.clone()]).unwrap();
        let selected = preparer.config(&target).unwrap().ptp.unwrap();
        assert_eq!(
            selected.setpeers().unwrap(),
            sender::group::peers(&[target.address.ip(), local]).unwrap()
        );
        assert_eq!(selected.domain(), clock::domain::Domain::new(7, 1).unwrap());
        let before = selected.setpeers().unwrap();
        let mut invalid = target.clone();
        invalid.address.set_ip("224.0.0.1".parse().unwrap());
        assert!(preparer.selection(&[invalid]).is_err());
        assert_eq!(
            preparer
                .config(&target)
                .unwrap()
                .ptp
                .unwrap()
                .setpeers()
                .unwrap(),
            before
        );
        preparer.selection(&[]).unwrap();
    }
    #[test]
    fn credentials_follow_uid_and_never_inherit_template_pairing() {
        let dir = Temp::new();
        let store = Arc::new(SenderStore::open(&dir.0).unwrap());
        let credentials =
            sender::PairingCredentials::new(store.controller(), b"receiver-a".to_vec(), [7; 32])
                .unwrap();
        store.save("a", &credentials).unwrap();
        let mut config = template();
        config.pairing = Some(credentials);
        let preparer = StoredSessionPreparer::new(
            config,
            store,
            AuthenticationPolicy::AllowTransientForUnpaired,
        );
        let a = preparer.config(&receiver("a")).unwrap();
        assert_eq!(a.pairing.unwrap().accessory_id(), b"receiver-a");
        let b = preparer.config(&receiver("b")).unwrap();
        assert!(b.pairing.is_none());
        assert_eq!(b.endpoint, receiver("b").address);
        assert!(b.preserve_receiver_volume);
    }
}
