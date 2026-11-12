//! Bridge between the controller's MRP PIN sheet and sender pairing. No audio.
//! The connection owner supplies PRIVATE channels for this request and keeps
//! reading MRP while this future waits. Never share its reply channel across
//! controllers or overlapping requests. Dropping the future cancels pairing.
use anyhow::{Context, Result, bail};
use mrp::{
    authorization::{self, Status},
    wire::Node,
};
use sender::{PairingCredentials, PinPairing};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::{mpsc, watch};

pub struct Request {
    pub endpoint: SocketAddr,
    pub controller: pair::Identity,
    pub route: Node,
    pub answer_window: Duration,
}

fn cancelled(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

/// One MRP connection owns this router. Each selection gets a fresh private
/// reply queue; concurrent PIN sheets on the same connection are rejected.
/// The connection reader must route only authorization responses here and keep
/// forwarding outgoing messages while the selection worker awaits pairing.
pub struct ControllerAuthorization {
    outgoing: mpsc::Sender<Node>,
    active: std::sync::Arc<std::sync::Mutex<Option<(uuid::Uuid, mpsc::Sender<Node>)>>>,
    stop: watch::Sender<bool>,
}
pub struct AuthorizationScope {
    token: uuid::Uuid,
    active: std::sync::Arc<std::sync::Mutex<Option<(uuid::Uuid, mpsc::Sender<Node>)>>>,
    outgoing: mpsc::Sender<Node>,
    incoming: Option<mpsc::Receiver<Node>>,
    stop: watch::Receiver<bool>,
}
/// Cloneable request context does not own the connection lifetime. The worker
/// opens a fresh scope only when a selected receiver actually needs a PIN.
#[derive(Clone)]
pub struct AuthorizationContext {
    outgoing: mpsc::Sender<Node>,
    active: std::sync::Arc<std::sync::Mutex<Option<(uuid::Uuid, mpsc::Sender<Node>)>>>,
    stop: watch::Receiver<bool>,
}
impl ControllerAuthorization {
    pub fn new(outgoing: mpsc::Sender<Node>) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            outgoing,
            active: Default::default(),
            stop,
        }
    }
    pub fn begin(&self) -> Result<AuthorizationScope> {
        self.context().begin()
    }
    pub fn context(&self) -> AuthorizationContext {
        AuthorizationContext {
            outgoing: self.outgoing.clone(),
            active: self.active.clone(),
            stop: self.stop.subscribe(),
        }
    }
}
impl AuthorizationContext {
    pub fn begin(&self) -> Result<AuthorizationScope> {
        if cancelled(&self.stop) {
            bail!("authorization controller disconnected");
        }
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            bail!("controller already has a pending authorization");
        }
        let token = uuid::Uuid::new_v4();
        let (sender, incoming) = mpsc::channel(4);
        *active = Some((token, sender));
        Ok(AuthorizationScope {
            token,
            active: self.active.clone(),
            outgoing: self.outgoing.clone(),
            incoming: Some(incoming),
            stop: self.stop.clone(),
        })
    }
}
impl ControllerAuthorization {
    /// No PIN content is logged or retained outside the bounded request queue.
    /// A response without an active sheet is ignored; overflow is an error for
    /// the connection owner, never silent delivery to a different controller.
    pub fn response(&self, message: Node) -> Result<()> {
        if message.number(mrp::schema::field::ProtocolMessage::r#type)
            != Some(mrp::schema::kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE as u64)
        {
            bail!("not a route authorization response");
        }
        if let Some((_, sender)) = self.active.lock().unwrap().as_ref() {
            sender
                .try_send(message)
                .map_err(|_| anyhow::anyhow!("authorization response queue unavailable"))?;
        }
        Ok(())
    }
}
impl Drop for ControllerAuthorization {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        self.active.lock().unwrap().take();
    }
}
impl AuthorizationScope {
    pub async fn authorize_and_save(
        mut self,
        request: Request,
        receiver_uid: String,
        store: std::sync::Arc<crate::sender_store::SenderStore>,
    ) -> Result<Option<PairingCredentials>> {
        authorize_and_save(
            request,
            receiver_uid,
            store,
            self.outgoing.clone(),
            self.incoming.take().unwrap(),
            self.stop.clone(),
        )
        .await
    }
}
impl Drop for AuthorizationScope {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap();
        if active
            .as_ref()
            .is_some_and(|(token, _)| *token == self.token)
        {
            active.take();
        }
    }
}

/// Use the application store's controller identity, reusing existing credentials
/// or saving successful PIN pairing off the async reader thread. The UID must be
/// the physical receiver's stable UID, not a UI group alias or network address.
pub async fn authorize_and_save(
    mut request: Request,
    receiver_uid: String,
    store: std::sync::Arc<crate::sender_store::SenderStore>,
    outgoing: mpsc::Sender<Node>,
    incoming: mpsc::Receiver<Node>,
    stop: watch::Receiver<bool>,
) -> Result<Option<PairingCredentials>> {
    if cancelled(&stop) {
        return Ok(None);
    }
    if let Some(credentials) = store.get(&receiver_uid)? {
        return Ok(Some(credentials));
    }
    request.controller = store.controller();
    authorize_inner(
        request,
        outgoing,
        incoming,
        stop,
        Some((receiver_uid, store)),
    )
    .await
}

pub async fn authorize(
    request: Request,
    outgoing: mpsc::Sender<Node>,
    incoming: mpsc::Receiver<Node>,
    stop: watch::Receiver<bool>,
) -> Result<Option<PairingCredentials>> {
    authorize_inner(request, outgoing, incoming, stop, None).await
}

async fn authorize_inner(
    request: Request,
    outgoing: mpsc::Sender<Node>,
    mut incoming: mpsc::Receiver<Node>,
    mut stop: watch::Receiver<bool>,
    persistence: Option<(String, std::sync::Arc<crate::sender_store::SenderStore>)>,
) -> Result<Option<PairingCredentials>> {
    if cancelled(&stop) {
        return Ok(None);
    }
    if request.answer_window.is_zero() || request.answer_window > Duration::from_secs(120) {
        bail!("route authorization window must be1..120 seconds");
    }
    request.route.validate("AVOutputDeviceDescriptor")?;
    let identifier = uuid::Uuid::new_v4().to_string().to_uppercase();
    let exchange = async {
        let pending =
            PinPairing::begin(request.endpoint, request.controller, Duration::from_secs(8)).await?;
        outgoing
            .send(authorization::prompt(&request.route, &identifier)?)
            .await
            .map_err(|_| anyhow::anyhow!("controller disconnected before PIN prompt"))?;
        loop {
            let Some(message) = incoming.recv().await else {
                return Ok(None);
            };
            let response = authorization::response(&message)?;
            if response.identifier().is_some_and(|id| id != identifier) {
                continue;
            }
            // Identifier-less answers are allowed only on this request-owned
            // channel, matching the Station's existing controller path.
            let Some(pin) = response.into_pin() else {
                return Ok(None);
            };
            return pending.finish(&pin).await.map(Some);
        }
    };
    let started = tokio::time::Instant::now();
    let mut result = tokio::select! {
        biased;
        _ = async {
            loop {
                if *stop.borrow_and_update() || stop.changed().await.is_err() { break; }
            }
        } => Ok(None),
        result = tokio::time::timeout(request.answer_window, exchange) =>
            result.context("route authorization deadline exceeded").and_then(|r| r),
    };
    if let (Ok(Some(credentials)), Some((uid, store))) = (&result, persistence) {
        let saved = credentials.clone();
        let remaining = request.answer_window.saturating_sub(started.elapsed());
        let write = tokio::task::spawn_blocking(move || store.save(&uid, &saved));
        if let Err(error) = async {
            tokio::time::timeout(remaining, write)
                .await
                .context("pairing persistence deadline exceeded")?
                .context("pairing persistence task failed")?
        }
        .await
        {
            result = Err(error);
        }
    }
    let status = match &result {
        Ok(Some(_)) => Status::Ok,
        Err(error) if error.downcast_ref::<pair::Error>().is_some() => Status::AuthenticationFailed,
        _ => Status::UnknownError,
    };
    // A full or dead controller channel must not hold pairing resources alive.
    let _ = tokio::time::timeout(
        Duration::from_millis(250),
        outgoing.send(authorization::status(&request.route, status)?),
    )
    .await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrp::{
        messages::envelope,
        schema::{field as f, kind},
    };
    use pair::{
        PairingStore,
        hap::{PairSetup, SetupOutcome},
    };

    #[tokio::test]
    async fn controller_scopes_are_private_bounded_and_cancelled_with_the_connection() {
        let (out_a, _messages_a) = mpsc::channel(4);
        let (out_b, _messages_b) = mpsc::channel(4);
        let a = ControllerAuthorization::new(out_a);
        let b = ControllerAuthorization::new(out_b);
        let mut first = a.begin().unwrap();
        let mut other = b.begin().unwrap();
        assert!(a.begin().is_err());
        let reply = |id| {
            envelope(
                kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE,
                &Node::default(),
                Some(id),
            )
        };
        a.response(reply("a")).unwrap();
        b.response(reply("b")).unwrap();
        assert_eq!(
            first
                .incoming
                .as_mut()
                .unwrap()
                .recv()
                .await
                .unwrap()
                .string(2),
            Some("a")
        );
        assert_eq!(
            other
                .incoming
                .as_mut()
                .unwrap()
                .recv()
                .await
                .unwrap()
                .string(2),
            Some("b")
        );
        for _ in 0..4 {
            a.response(reply("queued")).unwrap();
        }
        assert!(a.response(reply("overflow")).is_err());
        assert!(a.response(Node::default()).is_err());
        drop(first);
        a.response(reply("no-active-request")).unwrap();
        let mut fresh = a.begin().unwrap();
        assert!(fresh.incoming.as_mut().unwrap().try_recv().is_err());
        drop(a);
        assert!(cancelled(&fresh.stop));
        assert!(fresh.incoming.as_mut().unwrap().recv().await.is_none());
        assert!(!cancelled(&other.stop));
        b.response(reply("still-b")).unwrap();
        assert_eq!(
            other
                .incoming
                .as_mut()
                .unwrap()
                .recv()
                .await
                .unwrap()
                .string(2),
            Some("still-b")
        );
    }

    #[tokio::test]
    async fn cancelled_owner_never_starts_pairing_or_opens_a_pin_sheet() {
        for closed in [false, true] {
            let directory = std::env::temp_dir()
                .join(format!("airplay-cancelled-owner-{}", uuid::Uuid::new_v4()));
            let store =
                std::sync::Arc::new(crate::sender_store::SenderStore::open(&directory).unwrap());
            for persistent in [false, true] {
                let (owner, stop) = watch::channel(!closed);
                if closed {
                    drop(owner);
                }
                let (outgoing, mut messages) = mpsc::channel(1);
                let (_answers, incoming) = mpsc::channel(1);
                // Invalid configuration must never be examined after owner loss;
                // no socket or PIN prompt may be started on this path.
                let request = Request {
                    endpoint: ([127, 0, 0, 1], 0).into(),
                    controller: store.controller(),
                    route: Node::default(),
                    answer_window: Duration::ZERO,
                };
                let result = if persistent {
                    authorize_and_save(
                        request,
                        "receiver".into(),
                        store.clone(),
                        outgoing,
                        incoming,
                        stop,
                    )
                    .await
                } else {
                    authorize(request, outgoing, incoming, stop).await
                };
                assert!(result.unwrap().is_none());
                assert!(messages.recv().await.is_none());
                assert!(store.get("receiver").unwrap().is_none());
            }
            drop(store);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[tokio::test]
    async fn pin_sheet_drives_real_setup_without_starting_media() {
        // Success, empty sheet, controller disconnect and explicit stop.
        for mode in 0..4 {
            let cancelled = mode != 0;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let receiver = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = Vec::new();
                let store = pair::MemoryStore::default();
                let mut setup =
                    PairSetup::new(pair::Identity::from_seed("receiver", &[3; 32]), "4821");
                loop {
                    // Existing public RTSP codec, with buffered socket reads.
                    let request = loop {
                        match rtsp::decode_request(&mut buffer).unwrap() {
                            Some(request) => {
                                break Some(request);
                            }
                            None => {
                                use tokio::io::AsyncReadExt;
                                let mut bytes = [0; 4096];
                                let n = socket.read(&mut bytes).await.unwrap();
                                if n == 0 {
                                    break None;
                                }
                                buffer.extend_from_slice(&bytes[..n]);
                            }
                        }
                    };
                    let Some(request) = request else {
                        return store.len();
                    };
                    let mut response = rtsp::Response::to(&request, 200);
                    let mut done = false;
                    if request.uri != "/pair-pin-start" {
                        assert_eq!(request.uri, "/pair-setup");
                        response.body = match setup.handle(&request.body, &store).unwrap() {
                            SetupOutcome::Continue(body) => body,
                            SetupOutcome::Paired { reply, .. } => {
                                done = true;
                                reply
                            }
                            _ => panic!("unexpected setup result"),
                        };
                    }
                    let mut bytes = Vec::new();
                    response.encode(&mut bytes);
                    use tokio::io::AsyncWriteExt;
                    socket.write_all(&bytes).await.unwrap();
                    if done {
                        return store.len();
                    }
                }
            });
            let mut route = Node::default();
            route.text(f::AVOutputDeviceDescriptor::name, "Receiver");
            let directory =
                std::env::temp_dir().join(format!("airplay-route-store-{}", uuid::Uuid::new_v4()));
            let store =
                std::sync::Arc::new(crate::sender_store::SenderStore::open(&directory).unwrap());
            let request = Request {
                endpoint,
                controller: pair::Identity::from_seed("controller", &[4; 32]),
                route,
                answer_window: Duration::from_secs(10),
            };
            let (out, mut messages) = mpsc::channel(4);
            let (answers, input) = mpsc::channel(4);
            let (stop, stopped) = watch::channel(false);
            let task = tokio::spawn(authorize_and_save(
                request,
                "physical-receiver".into(),
                store.clone(),
                out,
                input,
                stopped,
            ));
            let prompt = messages.recv().await.unwrap();
            assert_eq!(prompt.number(1), Some(59));
            let mut body = Node::default();
            if !cancelled {
                body.text(1, "4821");
            }
            let reply = envelope(
                kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE,
                &body,
                prompt.string(f::ProtocolMessage::identifier),
            );
            match mode {
                2 => drop(answers),
                3 => {
                    stop.send_replace(true);
                }
                _ => {
                    if !cancelled {
                        let mut stale = Node::default();
                        stale.text(1, "0000");
                        answers
                            .send(envelope(
                                kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE,
                                &stale,
                                Some("other-request"),
                            ))
                            .await
                            .unwrap();
                    }
                    answers.send(reply).await.unwrap();
                }
            }
            let result = task.await.unwrap().unwrap();
            assert_eq!(result.is_none(), cancelled);
            let status = messages.recv().await.unwrap();
            assert_eq!(status.number(1), Some(61));
            assert_eq!(receiver.await.unwrap(), usize::from(!cancelled));
            assert_eq!(
                store.get("physical-receiver").unwrap().is_some(),
                !cancelled
            );
            drop(store);
            assert_eq!(
                crate::sender_store::SenderStore::open(&directory)
                    .unwrap()
                    .get("physical-receiver")
                    .unwrap()
                    .is_some(),
                !cancelled
            );
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
