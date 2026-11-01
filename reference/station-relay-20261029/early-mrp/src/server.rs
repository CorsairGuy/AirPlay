use anyhow::{bail,Result};
use plist::Value;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::{TcpListener,TcpStream},sync::{mpsc,watch}};
use uuid::Uuid;
use crate::{control::Control,model::{self,f,kind,Feed,Identity},schema,wire::{self,Node}};
enum Output{Ack(u64),Messages(Vec<Node>)}
// UTC wall clock for log lines; the station has no timestamps in the RAM log otherwise.
fn stamp()->String{let s=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs()).unwrap_or(0)%86400;format!("{:02}:{:02}:{:02}",s/3600,s/60%60,s%60)}
fn framed(kind:&[u8;4],seq:u64,payload:&[u8])->Vec<u8>{let mut out=Vec::with_capacity(32+payload.len());out.extend(((32+payload.len())as u32).to_be_bytes());out.extend(kind);out.extend([0u8;8]);out.extend(if kind==b"sync"{b"cmnd"}else{b"\0\0\0\0"});out.extend(seq.to_be_bytes());out.extend([0u8;4]);out.extend(payload);out}
async fn writer(mut write:tokio::net::tcp::OwnedWriteHalf,mut rx:mpsc::Receiver<Output>)->Result<()>{let mut seq=u64::from_be_bytes(Uuid::new_v4().as_bytes()[..8].try_into()?);while let Some(output)=rx.recv().await{let bytes=match output{Output::Ack(n)=>framed(b"rply",n,&[]),Output::Messages(messages)=>{let data=wire::encode_packet(&messages);let payload=model::binary(&model::dict(vec![("params",model::dict(vec![("data",Value::Data(data))]))]));let bytes=framed(b"sync",seq,&payload);seq=seq.wrapping_add(1);bytes}};tokio::time::timeout(tokio::time::Duration::from_secs(5),write.write_all(&bytes)).await??;}Ok(())}
fn reply(identifier:Option<&str>,error:u64)->Node{let mut msg=model::envelope("UNKNOWN_MESSAGE",Node::default(),identifier);msg.0.retain(|(tag,_)|*tag!=f("ProtocolMessage","errorCode"));msg.int(f("ProtocolMessage","errorCode"),error);msg}
fn command_reply(identifier:Option<&str>,error:u64)->Node{let mut body=Node::default();body.int(f("SendCommandResultMessage","sendError"),error).int(f("SendCommandResultMessage","handlerReturnStatus"),schema::enum_value("HandlerReturnStatus",if error==0{"Success"}else{"CommandFailed"}));model::envelope("SEND_COMMAND_RESULT_MESSAGE",body,identifier)}
async fn handle(message:Node,identity:&Identity,feed:&Feed,control:&Control,subscribed:&mut bool)->Result<Vec<Node>>{
 let typ=message.number(f("ProtocolMessage","type")).unwrap_or(0)as u32;let id=message.string(f("ProtocolMessage","identifier"));let body=message.nested(schema::extension(typ))?;
 if std::env::var_os("MINI2_MRP_TRACE").is_some(){println!("MRP_TRACE type={typ} request={} source={} control={}",id.is_some(),feed.source,feed.control_available);}
 if typ==kind("DEVICE_INFO_MESSAGE"){return Ok(vec![model::device_info(identity,feed,id)])}
 if typ==kind("CLIENT_UPDATES_CONFIG_MESSAGE"){*subscribed=true;let mut out=model::topology(identity,feed);out.extend(model::announcement(identity,feed));if let Some(artwork)=model::artwork_update(identity,feed){out.push(artwork);}if id.is_some(){out.push(reply(id,0));}return Ok(out)}
 if typ==kind("PLAYBACK_QUEUE_REQUEST_MESSAGE"){return Ok(vec![model::state(identity,feed,Some(&body),id)])}
 if typ==kind("GET_VOLUME_MESSAGE"){let mut body=Node::default();body.float(f("GetVolumeResultMessage","volume"),feed.volume);return Ok(vec![model::envelope("GET_VOLUME_RESULT_MESSAGE",body,id)])}
 if typ==kind("SET_VOLUME_MESSAGE"){
  if let Some(uid)=body.string(f("SetVolumeMessage","outputDeviceUID")){if !model::same_uid(uid,&identity.device)&&!model::same_uid(uid,&identity.group)&&!model::same_uid(uid,model::active_group(identity,feed)){return Ok(vec![reply(id,39)])}}
  let value=body.real(f("SetVolumeMessage","volume")).unwrap_or(f64::NAN)as f32;
  if !value.is_finite()||!(0.0..=1.0).contains(&value){return Ok(vec![reply(id,18)])}
  let accepted=control.volume(value).await.is_ok();return Ok(vec![reply(id,if accepted{0}else{2}),model::volume(identity,&control.feed.borrow().clone())])
 }
 if typ==kind("SEND_COMMAND_MESSAGE"){let command=body.number(f("SendCommandMessage","command")).unwrap_or(0);let options=body.nested(f("SendCommandMessage","options"))?;let position=options.real(f("CommandOptions","playbackPosition"));let error=if !(1..=6).contains(&command)&&command!=45{schema::enum_value("SendError","NotSupported")}else if command==45&&!position.is_some_and(|v|v.is_finite()&&v>=0.0){schema::enum_value("SendError","InvalidOptions")}else if control.action(command,position).await.is_ok(){0}else{schema::enum_value("SendError","ConnectionFailed")};return Ok(vec![command_reply(id,error)])}
 if typ==kind("GENERIC_MESSAGE"){return Ok(vec![reply(id,0)])}
 // The reference control proxy consumes this group hint without an MRP reply.
 // It is not a request to change the selected output devices.
 if typ==kind("CONFIGURE_CONNECTION_MESSAGE"){return Ok(Vec::new())}
 if typ==kind("MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE"){
  let error=control.outputs.change(&body,identity,feed).await;
  let mut current=feed.clone();current.outputs=control.outputs.snapshot().await;
  let mut out=vec![reply(id,error)];out.extend(model::topology(identity,&current));return Ok(out)
 }
 if typ==kind("SET_DISCOVERY_MODE_MESSAGE"){let mut out=model::topology(identity,feed);if id.is_some(){out.push(reply(id,0));}return Ok(out)}
 if matches!(typ,37|38){return Ok(Vec::new())}
 Ok(if id.is_some(){vec![reply(id,6)]}else{Vec::new()})
}
async fn connection(socket:TcpStream,identity:Identity,mut updates:watch::Receiver<Feed>,control:Control)->Result<()>{
 let(mut read,write)=socket.into_split();let(tx,rx)=mpsc::channel(64);let writer=tokio::spawn(writer(write,rx));let mut buffer=Vec::new();let mut chunk=[0u8;16384];let mut subscribed=false;let mut previous=updates.borrow().clone();let opened=std::time::Instant::now();let mut last=0u64;println!("{} MRP connection opened",stamp());
 let result:Result<()>=async{loop{tokio::select!{
  count=read.read(&mut chunk)=>{let count=count?;if count==0{break}buffer.extend_from_slice(&chunk[..count]);while buffer.len()>=32{
   let size=u32::from_be_bytes(buffer[..4].try_into()?)as usize;if !(32..=4*1024*1024).contains(&size){bail!("invalid data frame size")};if buffer.len()<size{break}
   let rest=buffer.split_off(size);let frame=std::mem::replace(&mut buffer,rest);let seq=u64::from_be_bytes(frame[20..28].try_into()?);
   if &frame[4..8]==b"sync"{tx.send(Output::Ack(seq)).await?}
   if size>32{let value=model::decode_plist(&frame[32..])?;if let Some(data)=value.as_dictionary().and_then(|v|v.get("params")).and_then(Value::as_dictionary).and_then(|v|v.get("data")).and_then(Value::as_data){for message in wire::decode_packet(data)?{let current=updates.borrow().clone();let typ=message.number(f("ProtocolMessage","type")).unwrap_or(0);let id=message.string(f("ProtocolMessage","identifier")).map(str::to_owned);let out=match handle(message,&identity,&current,&control,&mut subscribed).await{Ok(out)=>out,Err(error)=>{eprintln!("{} MRP message type={typ} failed: {error:#}",stamp());if id.is_some(){vec![reply(id.as_deref(),6)]}else{Vec::new()}}};last=typ;if !out.is_empty(){tx.send(Output::Messages(out)).await?}}}}
  }},
  changed=updates.changed()=>{if changed.is_err(){break}let current=updates.borrow_and_update().clone();if subscribed{
   let mut output=Vec::new();if current.source!=previous.source{output.extend(model::announcement(&identity,&current));}else if current.title!=previous.title||current.artist!=previous.artist||current.album!=previous.album||current.cover!=previous.cover||current.duration!=previous.duration||current.control_available!=previous.control_available||current.playing!=previous.playing||(current.position-previous.position).abs()>=1.0{output.push(model::state(&identity,&current,None,None));}
   if current.cover!=previous.cover{if let Some(artwork)=model::artwork_update(&identity,&current){output.push(artwork);}}
   if current.route!=previous.route||current.source!=previous.source||current.output_control!=previous.output_control||current.outputs!=previous.outputs{output.push(model::device_info_update(&identity,&current));}
   if current.control_available!=previous.control_available||current.route!=previous.route||current.outputs!=previous.outputs||current.source!=previous.source||current.output_control!=previous.output_control{output.extend(model::topology(&identity,&current));}else if(current.volume-previous.volume).abs()>0.004{output.push(model::volume(&identity,&current));}
   if !output.is_empty(){tx.send(Output::Messages(output)).await?;previous=current;}
  }}
 }}Ok(())}.await;
 drop(tx);writer.abort();let reason=match &result{Ok(())=>"closed by peer".to_string(),Err(error)=>format!("{error:#}")};eprintln!("{} MRP connection closed after {}s (last message type={last}): {reason}",stamp(),opened.elapsed().as_secs());result
}
pub async fn run(port:u16,identity:Identity,updates:watch::Receiver<Feed>,control:Control)->Result<()>{let listener=TcpListener::bind(("127.0.0.1",port)).await?;println!("Native MRP ready on loopback:{port}");loop{let(socket,_)=listener.accept().await?;let identity=identity.clone();let updates=updates.clone();let control=control.clone();tokio::spawn(async move{let _=connection(socket,identity,updates,control).await;});}}
#[cfg(test)]mod tests{use super::*;#[test]fn headers_are_fixed_size(){assert_eq!(framed(b"rply",17,&[]).len(),32);assert_eq!(&framed(b"sync",17,b"123")[20..28],&17u64.to_be_bytes());}}
