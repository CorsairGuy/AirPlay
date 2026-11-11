//! Concrete noninteractive output-worker preparation. No PIN prompt, discovery,
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
        if pairing.is_none() && self.policy == AuthenticationPolicy::SavedOnly {
            bail!("receiver requires persistent authorization");
        }
        let mut config = self.template.clone();
        config.endpoint = receiver.address;
        config.pairing = pairing;
        Ok(config)
    }
}
impl SessionPreparer for StoredSessionPreparer {
    async fn prepare(&mut self, receiver: ReceiverEndpoint) -> Result<Session> {
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
