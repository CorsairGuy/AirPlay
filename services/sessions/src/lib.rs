//! Session preparation and caller-owned authentication policy.
use anyhow::Result;
use coordinator::outputs::ReceiverEndpoint;
use media::tx::{Session, pool::SessionPool};
use std::future::Future;

pub mod buffered_sender;
pub mod output_card;
pub mod output_router;
pub mod output_worker;
pub mod pcm_sender;
pub mod photo;
pub mod preparer;
pub mod ptp_group;
pub mod send_control;
pub mod serve;
pub mod session;
pub mod transport;

/// Application supplies authentication/configuration. Dropping this future
/// must release pending pairing resources; do not detach network work here.
pub trait SessionPreparer: Send {
    /// Connection-private context; default noninteractive preparers ignore it.
    /// Implementations must never retain one controller's context for another
    /// request. Opening a scope is lazy, only for receivers needing a PIN.
    fn prepare_authorized(
        &mut self,
        receiver: ReceiverEndpoint,
        _authorization: Option<&identity::route_pairing::AuthorizationContext>,
    ) -> impl Future<Output = Result<Session>> + Send {
        self.prepare(receiver)
    }
    /// Bounded reconciliation after pool changes and periodic task-state
    /// observation. Failure terminates the owner and awaits pool cleanup.
    fn active(&mut self, _pool: &SessionPool) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
    /// Release external interests after pool teardown, even if reconciliation
    /// or preparation failed. Must be bounded and safe after a cancelled call.
    fn finish(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
    /// Validated complete physical selection, before preparing any additions.
    /// Pure configuration/preflight only: no I/O or mutation of live sessions.
    fn selection(&mut self, _receivers: &[ReceiverEndpoint]) -> Result<()> {
        Ok(())
    }
    fn prepare(
        &mut self,
        receiver: ReceiverEndpoint,
    ) -> impl Future<Output = Result<Session>> + Send;
}
