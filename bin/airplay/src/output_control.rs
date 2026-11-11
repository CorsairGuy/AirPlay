//! Bounded handoff from each MRP reader to the application's serialized output
//! worker. The worker owns discovery, pairing, relay_selection and topology.
use mrp::wire::Node;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct OutputControl(mpsc::Sender<OutputRequest>);
pub struct OutputRequest {
    pub body: Node,
    /// Reply with the actual MRP error code after applying the operation.
    /// Check is_closed()/closed() to cancel pending pairing on controller loss.
    pub reply: oneshot::Sender<u64>,
}
impl OutputControl {
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<OutputRequest>) {
        let (tx, rx) = mpsc::channel(capacity.clamp(1, 64));
        (Self(tx), rx)
    }
    pub(crate) fn submit(&self, body: Node) -> Result<oneshot::Receiver<u64>, ()> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(OutputRequest { body, reply })
            .map_err(|_| ())?;
        Ok(result)
    }
}
