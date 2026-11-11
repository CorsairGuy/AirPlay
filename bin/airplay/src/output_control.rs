//! Bounded handoff from each MRP reader to the application's serialized output
//! worker. The worker owns discovery, pairing, relay_selection and topology.
use mrp::wire::Node;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct OutputControl(mpsc::Sender<OutputRequest>);
pub(crate) trait VolumeBackend {
    fn set_routes(
        &self,
        catalog: &sender::outputs::OutputCatalog,
        levels: &[(String, sender::Volume)],
    ) -> anyhow::Result<()>;
}
impl VolumeBackend for sender::pool::SessionPool {
    fn set_routes(
        &self,
        catalog: &sender::outputs::OutputCatalog,
        levels: &[(String, sender::Volume)],
    ) -> anyhow::Result<()> {
        self.set_route_volumes(catalog, levels)
    }
}
pub(crate) type VolumeOperation =
    Box<dyn FnOnce(&dyn VolumeBackend, &sender::outputs::OutputCatalog) -> u64 + Send>;
pub struct OutputRequest {
    pub body: Node,
    pub volume: Option<(String, f32)>,
    pub batch: Option<Vec<(String, f32)>>,
    pub(crate) operation: Option<VolumeOperation>,
    after: Option<Box<dyn FnOnce(u64) + Send>>,
    /// Reply with the actual MRP error code after applying the operation.
    /// Check is_closed()/closed() to cancel pending pairing on controller loss.
    pub reply: oneshot::Sender<u64>,
}
impl OutputRequest {
    /// Run the accepted-state callback before acknowledging, so the next queued
    /// operation sees fresh levels even if its controller's writer is delayed.
    pub fn finish(mut self, code: u64) {
        if let Some(after) = self.after.take() {
            after(code);
        }
        let _ = self.reply.send(code);
    }
}
#[cfg(test)]
pub(crate) mod testing {
    #[derive(Default)]
    pub struct Backend(pub std::sync::Mutex<Vec<Vec<(String, sender::Volume)>>>);
    impl super::VolumeBackend for Backend {
        fn set_routes(
            &self,
            _: &sender::outputs::OutputCatalog,
            levels: &[(String, sender::Volume)],
        ) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(levels.to_vec());
            Ok(())
        }
    }
    pub fn execute(mut request: super::OutputRequest, backend: &Backend) {
        let operation = request.operation.take().unwrap();
        let code = operation(
            backend,
            &sender::outputs::OutputCatalog::new(vec![]).unwrap(),
        );
        request.finish(code);
    }
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
                operation: None,
                after: None,
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
        self.enqueue_volume(uid, value, None)
    }
    pub(crate) fn submit_volume_after(
        &self,
        uid: String,
        value: f32,
        after: Box<dyn FnOnce(u64) + Send>,
    ) -> Result<oneshot::Receiver<u64>, ()> {
        self.enqueue_volume(uid, value, Some(after))
    }
    fn enqueue_volume(
        &self,
        uid: String,
        value: f32,
        after: Option<Box<dyn FnOnce(u64) + Send>>,
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
                operation: None,
                after,
                reply,
            })
            .map_err(|_| ())?;
        Ok(result)
    }
    pub(crate) fn submit_operation(
        &self,
        operation: VolumeOperation,
    ) -> Result<oneshot::Receiver<u64>, ()> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(OutputRequest {
                body: Node::default(),
                volume: None,
                batch: None,
                operation: Some(operation),
                after: None,
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
                operation: None,
                after: None,
                reply,
            })
            .map_err(|_| ())?;
        Ok(result)
    }
}
