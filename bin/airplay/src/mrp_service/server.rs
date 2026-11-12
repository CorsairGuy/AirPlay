use crate::mrp_service::{
    control::Control,
    model::{self, Feed, Identity, f, kind},
    schema,
    wire::{self, Node},
};
use anyhow::{Result, bail};
use plist::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, watch},
};
use uuid::Uuid;
enum Output {
    Ack(u64),
    Messages(Vec<Node>),
}
fn framed(kind: &[u8; 4], seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + payload.len());
    out.extend(((32 + payload.len()) as u32).to_be_bytes());
    out.extend(kind);
    out.extend([0u8; 8]);
    out.extend(if kind == b"sync" {
        b"cmnd"
    } else {
        b"\0\0\0\0"
    });
    out.extend(seq.to_be_bytes());
    out.extend([0u8; 4]);
    out.extend(payload);
    out
}
async fn writer(mut write: impl AsyncWrite + Unpin, mut rx: mpsc::Receiver<Output>) -> Result<()> {
    let mut seq = u64::from_be_bytes(Uuid::new_v4().as_bytes()[..8].try_into()?);
    while let Some(output) = rx.recv().await {
        let bytes = match output {
            Output::Ack(n) => framed(b"rply", n, &[]),
            Output::Messages(messages) => {
                let data = wire::encode_packet(&messages);
                let payload = model::binary(&model::dict(vec![(
                    "params",
                    model::dict(vec![("data", Value::Data(data))]),
                )]));
                let bytes = framed(b"sync", seq, &payload);
                seq = seq.wrapping_add(1);
                bytes
            }
        };
        tokio::time::timeout(tokio::time::Duration::from_secs(5), write.write_all(&bytes))
            .await??;
    }
    Ok(())
}
fn reply(identifier: Option<&str>, error: u64) -> Node {
    let mut msg = model::envelope("UNKNOWN_MESSAGE", Node::default(), identifier);
    msg.0
        .retain(|(tag, _)| *tag != f("ProtocolMessage", "errorCode"));
    msg.int(f("ProtocolMessage", "errorCode"), error);
    msg
}
fn command_reply(identifier: Option<&str>, error: u64) -> Node {
    let mut body = Node::default();
    body.int(f("SendCommandResultMessage", "sendError"), error)
        .int(
            f("SendCommandResultMessage", "handlerReturnStatus"),
            schema::enum_value(
                "HandlerReturnStatus",
                if error == 0 {
                    "Success"
                } else {
                    "CommandFailed"
                },
            ),
        );
    model::envelope("SEND_COMMAND_RESULT_MESSAGE", body, identifier)
}

/// Queue output selection without awaiting pairing/teardown on the MRP reader.
#[cfg(test)]
fn dispatch_output(
    message: &Node,
    control: &Control,
    tx: &mpsc::Sender<Output>,
    pending: &mut tokio::task::JoinSet<()>,
) -> Result<bool> {
    dispatch_output_authorized(message, control, tx, pending, None)
}
fn dispatch_output_authorized(
    message: &Node,
    control: &Control,
    tx: &mpsc::Sender<Output>,
    pending: &mut tokio::task::JoinSet<()>,
    authorization: Option<crate::route_pairing::AuthorizationContext>,
) -> Result<bool> {
    if message.number(f("ProtocolMessage", "type"))
        != Some(kind("MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE") as u64)
    {
        return Ok(false);
    }
    let Some(backend) = &control.outputs else {
        return Ok(false);
    };
    while pending.try_join_next().is_some() {}
    let id = message
        .string(f("ProtocolMessage", "identifier"))
        .map(str::to_owned);
    let parsed = (|| -> Result<_> {
        let body = message.nested(schema::extension(kind(
            "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
        )))?;
        let body = mrp::wire::Node::parse(&body.encode())?;
        mrp::output_context::Request::parse(&body)?;
        Ok(body)
    })();
    let result = if pending.len() >= 32 {
        Err(100)
    } else {
        match parsed {
            Ok(body) => backend
                .submit_authorized(body, authorization)
                .map_err(|_| 100),
            Err(_) => Err(18),
        }
    };
    let tx = tx.clone();
    // At most32 pending operations per connection; a rejected request replies
    // immediately without spawning another potentially blocked writer task.
    if let Err(error) = result {
        // If even this bounded queue is full, the existing writer/reader
        // connection will close rather than acknowledging an unapplied change.
        tx.try_send(Output::Messages(vec![reply(id.as_deref(), error)]))
            .map_err(|_| anyhow::anyhow!("output response queue full or closed"))?;
        return Ok(true);
    }
    let result = result.unwrap();
    pending.spawn(async move {
        let error = match tokio::time::timeout(tokio::time::Duration::from_secs(120), result).await
        {
            Ok(Ok(code)) => code,
            _ => 100,
        };
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            tx.send(Output::Messages(vec![reply(id.as_deref(), error)])),
        )
        .await;
    });
    Ok(true)
}
fn dispatch_group_volume(
    message: &Node,
    identity: &Identity,
    feed: &Feed,
    control: &Control,
    tx: &mpsc::Sender<Output>,
    pending: &mut tokio::task::JoinSet<()>,
) -> Result<bool> {
    if message.number(f("ProtocolMessage", "type")) != Some(kind("SET_VOLUME_MESSAGE") as u64)
        || control.outputs.is_none()
        || feed.outputs.is_empty()
    {
        return Ok(false);
    }
    let body = message.nested(schema::extension(kind("SET_VOLUME_MESSAGE")))?;
    let uid = body.string(f("SetVolumeMessage", "outputDeviceUID"));
    if body.get(f("SetVolumeMessage", "outputDeviceUID")).is_some() && uid.is_none() {
        return Ok(false);
    }
    if let Some(uid) = uid {
        if model::same_uid(uid, &identity.device)
            || (!model::same_uid(uid, &identity.group)
                && !model::same_uid(uid, model::active_group(identity, feed)))
        {
            return Ok(false);
        }
    }
    let value = body
        .real(f("SetVolumeMessage", "volume"))
        .unwrap_or(f64::NAN);
    let id = message
        .string(f("ProtocolMessage", "identifier"))
        .map(str::to_owned);
    while pending.try_join_next().is_some() {}
    let error = if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        Some(18)
    } else if pending.len() >= 32 {
        Some(100)
    } else {
        None
    };
    if let Some(error) = error {
        tx.try_send(Output::Messages(vec![reply(id.as_deref(), error)]))
            .map_err(|_| anyhow::anyhow!("group volume response queue full"))?;
        return Ok(true);
    }
    let change = control.group_volume(identity.device.clone(), value as f32);
    let tx = tx.clone();
    pending.spawn(async move {
        let code = if change.await.is_ok() { 0 } else { 2 };
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            tx.send(Output::Messages(vec![reply(id.as_deref(), code)])),
        )
        .await;
    });
    Ok(true)
}

fn dispatch_remote_volume(
    message: &Node,
    identity: &Identity,
    feed: &Feed,
    control: &Control,
    tx: &mpsc::Sender<Output>,
    pending: &mut tokio::task::JoinSet<()>,
) -> Result<bool> {
    if message.number(f("ProtocolMessage", "type")) != Some(kind("SET_VOLUME_MESSAGE") as u64) {
        return Ok(false);
    }
    let Some(backend) = &control.outputs else {
        return Ok(false);
    };
    let body = message.nested(schema::extension(kind("SET_VOLUME_MESSAGE")))?;
    let Some(uid) = body.string(f("SetVolumeMessage", "outputDeviceUID")) else {
        return Ok(false);
    };
    if model::same_uid(uid, &identity.device)
        || model::same_uid(uid, &identity.group)
        || model::same_uid(uid, model::active_group(identity, feed))
    {
        return Ok(false);
    }
    while pending.try_join_next().is_some() {}
    let id = message
        .string(f("ProtocolMessage", "identifier"))
        .map(str::to_owned);
    let value = body
        .real(f("SetVolumeMessage", "volume"))
        .unwrap_or(f64::NAN);
    let result = if !value.is_finite()
        || !(0.0..=1.0).contains(&value)
        || uid.is_empty()
        || uid.len() > 256
        || uid.chars().any(char::is_control)
    {
        Err(18)
    } else if pending.len() >= 32 {
        Err(100)
    } else {
        backend
            .submit_volume_after(
                uid.into(),
                value as f32,
                Box::new({
                    let uid = uid.to_owned();
                    let updates = control.feed.clone();
                    move |code| {
                        if code == 0 {
                            updates.send_if_modified(|feed| {
                                let Some(output) = feed.outputs.iter_mut().find(|o| {
                                    o.descriptor
                                        .string(f("AVOutputDeviceDescriptor", "uniqueIdentifier"))
                                        .is_some_and(|id| model::same_uid(id, &uid))
                                }) else {
                                    return false;
                                };
                                let next = Some(value as f32);
                                if output.volume == next {
                                    false
                                } else {
                                    output.volume = next;
                                    true
                                }
                            });
                        }
                    }
                }),
            )
            .map_err(|_| 100)
    };
    let result = match result {
        Ok(result) => result,
        Err(code) => {
            tx.try_send(Output::Messages(vec![reply(id.as_deref(), code)]))
                .map_err(|_| anyhow::anyhow!("volume response queue full or closed"))?;
            return Ok(true);
        }
    };
    let tx = tx.clone();
    pending.spawn(async move {
        let code = match tokio::time::timeout(tokio::time::Duration::from_secs(10), result).await {
            Ok(Ok(code)) => code,
            _ => 100,
        };
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            tx.send(Output::Messages(vec![reply(id.as_deref(), code)])),
        )
        .await;
    });
    Ok(true)
}

async fn handle(
    message: Node,
    identity: &Identity,
    feed: &Feed,
    control: &Control,
    subscribed: &mut bool,
) -> Result<Vec<Node>> {
    let typ = message.number(f("ProtocolMessage", "type")).unwrap_or(0) as u32;
    let id = message.string(f("ProtocolMessage", "identifier"));
    let body = message.nested(schema::extension(typ))?;
    if std::env::var_os("AIRPLAY_MRP_TRACE").is_some() {
        println!(
            "MRP_TRACE type={typ} request={} source={} control={}",
            id.is_some(),
            feed.source,
            feed.control_available
        );
    }
    if typ == kind("DEVICE_INFO_MESSAGE") {
        return Ok(vec![model::device_info(identity, feed, id)]);
    }
    if typ == kind("CLIENT_UPDATES_CONFIG_MESSAGE") {
        *subscribed = true;
        let mut out = model::topology(identity, feed);
        out.extend(model::announcement(identity, feed));
        if id.is_some() {
            out.push(reply(id, 0));
        }
        return Ok(out);
    }
    if typ == kind("PLAYBACK_QUEUE_REQUEST_MESSAGE") {
        return Ok(vec![model::state(identity, feed, Some(&body), id)]);
    }
    if typ == kind("GET_VOLUME_MESSAGE") {
        if body.get(f("GetVolumeMessage", "outputDeviceUID")).is_some()
            && !body
                .string(f("GetVolumeMessage", "outputDeviceUID"))
                .is_some_and(|id| {
                    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
                })
        {
            return Ok(vec![reply(id, 18)]);
        }
        if let Some(uid) = body.string(f("GetVolumeMessage", "outputDeviceUID")) {
            if !model::same_uid(uid, &identity.device)
                && !model::same_uid(uid, &identity.group)
                && !model::same_uid(uid, model::active_group(identity, feed))
            {
                let value = feed
                    .outputs
                    .iter()
                    .find(|o| {
                        o.descriptor
                            .string(f("AVOutputDeviceDescriptor", "uniqueIdentifier"))
                            .is_some_and(|id| model::same_uid(id, uid))
                    })
                    .and_then(|o| o.volume);
                let Some(value) = value else {
                    return Ok(vec![reply(id, 39)]);
                };
                let mut result = Node::default();
                result.float(f("GetVolumeResultMessage", "volume"), value);
                return Ok(vec![model::envelope(
                    "GET_VOLUME_RESULT_MESSAGE",
                    result,
                    id,
                )]);
            }
        }
        let mut body = Node::default();
        let individual = message
            .nested(schema::extension(typ))?
            .string(f("GetVolumeMessage", "outputDeviceUID"))
            .is_some_and(|uid| model::same_uid(uid, &identity.device));
        let level = if control.outputs.is_some() && !feed.outputs.is_empty() && !individual {
            control.group_level(feed)
        } else {
            feed.volume
        };
        body.float(f("GetVolumeResultMessage", "volume"), level);
        return Ok(vec![model::envelope("GET_VOLUME_RESULT_MESSAGE", body, id)]);
    }
    if typ == kind("SET_VOLUME_MESSAGE") {
        if body.get(f("SetVolumeMessage", "outputDeviceUID")).is_some()
            && !body
                .string(f("SetVolumeMessage", "outputDeviceUID"))
                .is_some_and(|id| {
                    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
                })
        {
            return Ok(vec![reply(id, 18)]);
        }
        if let Some(uid) = body.string(f("SetVolumeMessage", "outputDeviceUID")) {
            if !model::same_uid(uid, &identity.device)
                && !model::same_uid(uid, &identity.group)
                && !model::same_uid(uid, model::active_group(identity, feed))
            {
                return Ok(vec![reply(id, 39)]);
            }
        }
        let value = body
            .real(f("SetVolumeMessage", "volume"))
            .unwrap_or(f64::NAN) as f32;
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Ok(vec![reply(id, 18)]);
        }
        let accepted = control.volume(value).await.is_ok();
        return Ok(vec![
            reply(id, if accepted { 0 } else { 2 }),
            model::volume(identity, &control.feed.borrow().clone()),
        ]);
    }
    if typ == kind("SEND_COMMAND_MESSAGE") {
        let command = body.number(f("SendCommandMessage", "command")).unwrap_or(0);
        let options = body.nested(f("SendCommandMessage", "options"))?;
        let position = options.real(f("CommandOptions", "playbackPosition"));
        let error = if !(1..=6).contains(&command) && command != 45 {
            schema::enum_value("SendError", "NotSupported")
        } else if command == 45 && !position.is_some_and(|v| v.is_finite() && v >= 0.0) {
            schema::enum_value("SendError", "InvalidOptions")
        } else if control.action(command, position).await.is_ok() {
            0
        } else {
            schema::enum_value("SendError", "ConnectionFailed")
        };
        return Ok(vec![command_reply(id, error)]);
    }
    if typ == kind("GENERIC_MESSAGE") {
        return Ok(vec![model::envelope(
            "GENERIC_MESSAGE",
            Node::default(),
            id,
        )]);
    }
    if typ == kind("MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE") {
        let body = mrp::wire::Node::parse(&body.encode())?;
        let request = match mrp::output_context::Request::parse(&body) {
            Ok(request) => request,
            Err(_) => return Ok(vec![reply(id, 18)]),
        };
        // Until a remote output backend is attached, acknowledge only local
        // identifiers. Unknown fields are not identifiers; malformed IDs must
        // never be silently dropped into a successful empty request.
        let own = request.identifiers().all(|uid| {
            model::same_uid(uid, &identity.device)
                || model::same_uid(uid, &identity.group)
                || model::same_uid(uid, model::active_group(identity, feed))
        });
        return Ok(vec![reply(id, if own { 0 } else { 33 })]);
    }
    if matches!(typ, 37 | 38) {
        return Ok(Vec::new());
    }
    Ok(if id.is_some() {
        vec![reply(id, 6)]
    } else {
        Vec::new()
    })
}
pub async fn connection(
    socket: impl AsyncRead + AsyncWrite + Send + 'static,
    identity: Identity,
    mut updates: watch::Receiver<Feed>,
    control: Control,
) -> Result<()> {
    let (mut read, write) = tokio::io::split(socket);
    let (tx, rx) = mpsc::channel(64);
    let writer = tokio::spawn(writer(write, rx));
    let (pin_out, mut pin_messages) = mpsc::channel(4);
    let authorization = crate::route_pairing::ControllerAuthorization::new(pin_out);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 16384];
    let mut subscribed = false;
    let mut previous = updates.borrow().clone();
    let opened = std::time::Instant::now();
    let mut last = 0u64;
    let mut pending = tokio::task::JoinSet::new();
    println!("{} MRP connection opened", crate::mrp_service::stamp::now());
    let result:Result<()>=async{loop{tokio::select!{
  Some(message)=pin_messages.recv()=>{tx.send(Output::Messages(vec![Node::parse(&message.encode())?])).await?;},
  count=read.read(&mut chunk)=>{let count=count?;if count==0{break}buffer.extend_from_slice(&chunk[..count]);while buffer.len()>=32{
   let size=u32::from_be_bytes(buffer[..4].try_into()?)as usize;if !(32..=4*1024*1024).contains(&size){bail!("invalid data frame size")};if buffer.len()<size{break}
   let rest=buffer.split_off(size);let frame=std::mem::replace(&mut buffer,rest);let seq=u64::from_be_bytes(frame[20..28].try_into()?);
   if &frame[4..8]==b"sync"{tx.send(Output::Ack(seq)).await?}
   if size>32{let value=match model::decode_plist(&frame[32..]){Ok(v)=>v,Err(error)=>{crate::mrp_log!("MRP frame skipped, bad plist: {error:#}");continue}};if let Some(data)=value.as_dictionary().and_then(|v|v.get("params")).and_then(Value::as_dictionary).and_then(|v|v.get("data")).and_then(Value::as_data){let messages=match wire::decode_packet(data){Ok(m)=>m,Err(error)=>{crate::mrp_log!("MRP frame skipped, bad protobuf: {error:#}");continue}};for message in messages{if message.number(f("ProtocolMessage","type"))==Some(kind("PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE") as u64){authorization.response(mrp::wire::Node::parse(&message.encode())?)?;continue;}if dispatch_output_authorized(&message,&control,&tx,&mut pending,Some(authorization.context()))?{continue;}let current=updates.borrow().clone();if dispatch_group_volume(&message,&identity,&current,&control,&tx,&mut pending)?{continue;}if dispatch_remote_volume(&message,&identity,&current,&control,&tx,&mut pending)?{continue;}let typ=message.number(f("ProtocolMessage","type")).unwrap_or(0);let id=message.string(f("ProtocolMessage","identifier")).map(str::to_owned);let out=match handle(message,&identity,&current,&control,&mut subscribed).await{Ok(out)=>out,Err(error)=>{crate::mrp_log!("MRP message type={typ} failed: {error:#}");if id.is_some(){vec![reply(id.as_deref(),6)]}else{Vec::new()}}};last=typ;if !out.is_empty(){tx.send(Output::Messages(out)).await?}}}}
  }},
  changed=updates.changed()=>{if changed.is_err(){break}let current=updates.borrow_and_update().clone();if subscribed{
   let mut output=Vec::new();if current.source!=previous.source{output.extend(model::announcement(&identity,&current));}else{output.extend(model::state_change(&identity,&previous,&current));}
   if current.route!=previous.route{output.push(model::device_info_update(&identity,&current));}
   output.extend(model::topology_updates(&identity,&previous,&current));
   if !output.is_empty(){tx.send(Output::Messages(output)).await?;previous=current;}
  }}
 }}Ok(())}.await;
    drop(authorization);
    drop(tx);
    writer.abort();
    let reason = match &result {
        Ok(()) => "closed by peer".to_string(),
        Err(error) => format!("{error:#}"),
    };
    eprintln!(
        "{} MRP connection closed after {}s (last message type={last}): {reason}",
        crate::mrp_service::stamp::now(),
        opened.elapsed().as_secs()
    );
    result
}
pub async fn run(
    port: u16,
    identity: Identity,
    updates: watch::Receiver<Feed>,
    control: Control,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("Native MRP ready on loopback:{port}");
    loop {
        let (socket, _) = listener.accept().await?;
        let identity = identity.clone();
        let updates = updates.clone();
        let control = control.clone();
        tokio::spawn(async move {
            let _ = connection(socket, identity, updates, control).await;
        });
    }
}
#[cfg(test)]
#[path = "authorization_tests.rs"]
mod authorization_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headers_are_fixed_size() {
        assert_eq!(framed(b"rply", 17, &[]).len(), 32);
        assert_eq!(&framed(b"sync", 17, b"123")[20..28], &17u64.to_be_bytes());
    }
    #[tokio::test]
    async fn group_slider_scales_local_and_remote_and_recovers_after_zero() {
        use crate::mrp_service::model::RemoteOutput;
        let identity = Identity {
            device: "local".into(),
            group: "group".into(),
        };
        let remote = crate::remote::Remote::new();
        let mut commands = remote.attach(1);
        let initial = Feed {
            volume: 0.1,
            outputs: vec![RemoteOutput {
                descriptor: Node::default()
                    .text(f("AVOutputDeviceDescriptor", "uniqueIdentifier"), "pod")
                    .clone(),
                volume: Some(0.3),
            }],
            ..Default::default()
        };
        let (feed, _) = watch::channel(initial);
        let (backend, mut requests) = crate::output_control::OutputControl::channel(4);
        let control = Control::new(feed.clone(), remote).with_outputs(backend);
        let (tx, mut replies) = mpsc::channel(4);
        let mut pending = tokio::task::JoinSet::new();
        for target in [0.2_f32, 0.0, 0.2] {
            let body = Node::default()
                .float(f("SetVolumeMessage", "volume"), target)
                .clone();
            let message = model::envelope("SET_VOLUME_MESSAGE", body, Some("group-slider"));
            let before = feed.borrow().clone();
            assert!(
                dispatch_group_volume(&message, &identity, &before, &control, &tx, &mut pending)
                    .unwrap()
            );
            let request = requests.recv().await.unwrap();
            let backend = crate::output_control::testing::Backend::default();
            crate::output_control::testing::execute(request, &backend);
            let recorded = backend.0.lock().unwrap();
            assert_eq!(recorded[0].len(), 1);
            assert_eq!(recorded[0][0].0, "pod");
            assert!(
                (recorded[0][0].1.as_db() - sender::Volume::normalized(target).unwrap().as_db())
                    .abs()
                    < 1e-6
            );
            drop(recorded);
            pending.join_next().await.unwrap().unwrap();
            let crate::remote::Command::Volume(local) = commands.recv().await.unwrap() else {
                panic!("wrong local command")
            };
            assert!((local - target / 3.0).abs() < 1e-6);
            let after = feed.borrow().clone();
            assert!((after.volume - target / 3.0).abs() < 1e-6);
            assert_eq!(after.outputs[0].volume, Some(target));
            assert!(
                model::topology_updates(&identity, &before, &after)
                    .iter()
                    .all(|n| n.number(1) != Some(65))
            );
            let Output::Messages(messages) = replies.recv().await.unwrap() else {
                panic!("wrong output")
            };
            assert_eq!(
                messages[0].number(f("ProtocolMessage", "errorCode")),
                Some(0)
            );
        }
        let current = feed.borrow().clone();
        assert!((control.group_level(&current) - 0.2).abs() < 1e-6);
    }
    #[tokio::test]
    async fn remote_volume_routes_without_changing_local_or_other_output() {
        use crate::mrp_service::model::RemoteOutput;
        let identity = Identity {
            device: "local".into(),
            group: "local-group".into(),
        };
        let remote = |uid: &str| RemoteOutput {
            descriptor: Node::default()
                .text(f("AVOutputDeviceDescriptor", "uniqueIdentifier"), uid)
                .clone(),
            volume: Some(0.2),
        };
        let before = Feed {
            outputs: vec![remote("a"), remote("b")],
            ..Default::default()
        };
        let (feed, _) = watch::channel(before.clone());
        let (backend, mut requests) = crate::output_control::OutputControl::channel(4);
        let control =
            Control::new(feed.clone(), crate::remote::Remote::new()).with_outputs(backend);
        let (tx, mut replies) = mpsc::channel(4);
        let mut pending = tokio::task::JoinSet::new();
        let mut body = Node::default();
        body.text(f("SetVolumeMessage", "outputDeviceUID"), "a")
            .float(f("SetVolumeMessage", "volume"), 0.3);
        let message = model::envelope("SET_VOLUME_MESSAGE", body, Some("volume-id"));
        assert!(
            dispatch_remote_volume(&message, &identity, &before, &control, &tx, &mut pending)
                .unwrap()
        );
        let request = requests.recv().await.unwrap();
        assert_eq!(request.volume, Some(("a".into(), 0.3)));
        request.finish(0);
        pending.join_next().await.unwrap().unwrap();
        let current = feed.borrow().clone();
        assert_eq!(current.volume, before.volume);
        assert_eq!(current.outputs[0].volume, Some(0.3));
        assert_eq!(current.outputs[1].volume, Some(0.2));
        let changes = model::topology_updates(&identity, &before, &current);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].number(1),
            Some(kind("VOLUME_DID_CHANGE_MESSAGE") as u64)
        );
        let Output::Messages(messages) = replies.recv().await.unwrap() else {
            panic!("wrong output")
        };
        assert_eq!(
            messages[0].string(f("ProtocolMessage", "identifier")),
            Some("volume-id")
        );
        let body = Node::default()
            .text(f("GetVolumeMessage", "outputDeviceUID"), "a")
            .clone();
        let replies = handle(
            model::envelope("GET_VOLUME_MESSAGE", body, Some("get")),
            &identity,
            &current,
            &control,
            &mut false,
        )
        .await
        .unwrap();
        assert!(
            (replies[0]
                .nested(schema::extension(kind("GET_VOLUME_RESULT_MESSAGE")))
                .unwrap()
                .real(f("GetVolumeResultMessage", "volume"))
                .unwrap()
                - 0.3)
                .abs()
                < 1e-6
        );
        for name in ["GET_VOLUME_MESSAGE", "SET_VOLUME_MESSAGE"] {
            let field = if name == "GET_VOLUME_MESSAGE" {
                f("GetVolumeMessage", "outputDeviceUID")
            } else {
                f("SetVolumeMessage", "outputDeviceUID")
            };
            let mut body = Node(vec![(field, wire::Value::Bytes(vec![255]))]);
            if name == "SET_VOLUME_MESSAGE" {
                body.float(f("SetVolumeMessage", "volume"), 0.8);
            }
            let result = handle(
                model::envelope(name, body, Some("bad")),
                &identity,
                &current,
                &control,
                &mut false,
            )
            .await
            .unwrap();
            assert_eq!(
                result[0].number(f("ProtocolMessage", "errorCode")),
                Some(18)
            );
            assert_eq!(*feed.borrow(), current);
        }
    }

    #[tokio::test]
    async fn output_request_carries_only_its_connection_authorization_context() {
        let (feed, _) = watch::channel(Feed::default());
        let (backend, mut requests) = crate::output_control::OutputControl::channel(2);
        let control = Control::new(feed, crate::remote::Remote::new()).with_outputs(backend);
        let (tx, _replies) = mpsc::channel(4);
        let (out, _pin_messages) = mpsc::channel(4);
        let owner = crate::route_pairing::ControllerAuthorization::new(out);
        let mut pending = tokio::task::JoinSet::new();
        let message = model::envelope(
            "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
            Node::default().text(5, "group").clone(),
            Some("selection"),
        );
        assert!(
            dispatch_output_authorized(
                &message,
                &control,
                &tx,
                &mut pending,
                Some(owner.context())
            )
            .unwrap()
        );
        let request = requests.recv().await.unwrap();
        let context = request.authorization.as_ref().unwrap();
        let scope = context.begin().unwrap();
        assert!(owner.begin().is_err());
        drop(scope);
        drop(owner);
        assert!(context.begin().is_err());
        request.finish(100);
        pending.join_next().await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn queued_output_reply_preserves_request_id_and_disconnect_cancels_wait() {
        let (feed, _) = watch::channel(Feed::default());
        let (backend, mut requests) = crate::output_control::OutputControl::channel(2);
        let control = Control::new(feed, crate::remote::Remote::new()).with_outputs(backend);
        let (tx, mut replies) = mpsc::channel(4);
        let mut pending = tokio::task::JoinSet::new();
        let message = model::envelope(
            "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
            Node::default().text(5, "group").clone(),
            Some("selection-id"),
        );
        assert!(dispatch_output(&message, &control, &tx, &mut pending).unwrap());
        let request = requests.recv().await.unwrap();
        assert_eq!(request.body.string(5), Some("group"));
        assert!(
            replies.try_recv().is_err(),
            "do not acknowledge before backend result"
        );
        request.reply.send(39).unwrap();
        let Output::Messages(messages) = replies.recv().await.unwrap() else {
            panic!("wrong output")
        };
        assert_eq!(
            messages[0].string(f("ProtocolMessage", "identifier")),
            Some("selection-id")
        );
        assert_eq!(
            messages[0].number(f("ProtocolMessage", "errorCode")),
            Some(39)
        );
        assert!(dispatch_output(&message, &control, &tx, &mut pending).unwrap());
        let mut request = requests.recv().await.unwrap();
        pending.shutdown().await;
        request.reply.closed().await;
    }

    #[tokio::test]
    async fn full_output_worker_queue_fails_without_blocking_reader() {
        let (feed, _) = watch::channel(Feed::default());
        let (backend, _requests) = crate::output_control::OutputControl::channel(1);
        let control = Control::new(feed, crate::remote::Remote::new()).with_outputs(backend);
        let (tx, mut replies) = mpsc::channel(4);
        let mut pending = tokio::task::JoinSet::new();
        let message = model::envelope(
            "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
            Node::default().text(2, "peer").clone(),
            Some("id"),
        );
        assert!(dispatch_output(&message, &control, &tx, &mut pending).unwrap());
        assert!(dispatch_output(&message, &control, &tx, &mut pending).unwrap());
        let Output::Messages(messages) = replies.recv().await.unwrap() else {
            panic!("wrong output")
        };
        assert_eq!(
            messages[0].number(f("ProtocolMessage", "errorCode")),
            Some(100)
        );
        pending.shutdown().await;
    }

    #[tokio::test]
    async fn output_requests_validate_ids_before_local_only_acknowledgement() {
        let identity = Identity {
            device: "ABCDEF01-2345-6789-ABCD-EF0123456789".into(),
            group: "group".into(),
        };
        let feed = Feed::default();
        let (updates, _) = watch::channel(feed.clone());
        let control = Control::new(updates, crate::remote::Remote::new());
        for (body, expected) in [
            (
                Node::default()
                    .text(2, &identity.device.to_ascii_lowercase())
                    .clone(),
                0,
            ),
            (Node::default().text(5, "foreign-cluster").clone(), 33),
            (Node(vec![(2, wire::Value::Bytes(vec![255]))]), 18),
            (Node::default().int(2, 7).clone(), 18),
            // A protobuf extension unknown to this implementation is not a UID.
            (Node::default().text(100, "future-extension").clone(), 0),
        ] {
            let message = model::envelope(
                "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
                body,
                Some("request"),
            );
            let replies = handle(message, &identity, &feed, &control, &mut false)
                .await
                .unwrap();
            assert_eq!(
                replies[0].number(f("ProtocolMessage", "errorCode")),
                Some(expected)
            );
        }
    }
}
