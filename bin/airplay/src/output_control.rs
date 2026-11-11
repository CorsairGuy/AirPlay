//! Bounded handoff from each MRP reader to the application's serialized output
//! worker. The worker owns discovery, pairing, relay_selection and topology.
use mrp::wire::Node;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct OutputControl(mpsc::Sender<OutputRequest>);
pub struct OutputRequest {
    pub body: Node,
    pub volume: Option<(String, f32)>,
    pub batch: Option<Vec<(String, f32)>>,
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
            .try_send(OutputRequest {
                body,
                volume: None,
                batch: None,
                reply,
            })
            .map_err(|_| ())?;
        Ok(result)
    }
    pub(crate) fn submit_volume(
        &self,
        uid: String,
        value: f32,
    ) -> Result<oneshot::Receiver<u64>, ()> {
        if uid.is_empty()
            || uid.len() > 256
            || uid.chars().any(char::is_control)
            || !value.is_finite()
            || !(0.0..=1.0).contains(&value)
        {
            return Err(());
        }
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(OutputRequest {
                body: Node::default(),
                volume: Some((uid, value)),
                batch: None,
                reply,
            })
            .map_err(|_| ())?;
        Ok(result)
    }
    pub(crate) fn submit_batch(
        &self,
        levels: Vec<(String, f32)>,
    ) -> Result<oneshot::Receiver<u64>, ()> {
        if levels.is_empty()
            || levels.len() > 64
            || levels.iter().any(|(uid, value)| {
                uid.is_empty()
                    || uid.len() > 256
                    || uid.chars().any(char::is_control)
                    || !value.is_finite()
                    || !(0.0..=1.0).contains(value)
            })
        {
            return Err(());
        }
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(OutputRequest {
                body: Node::default(),
                volume: None,
                batch: Some(levels),
                reply,
            })
            .map_err(|_| ())?;
        Ok(result)
    }
}
