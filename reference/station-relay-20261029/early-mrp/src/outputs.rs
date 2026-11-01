//! Output selection for the current local mix: Alice, AirPlay and Bluetooth.
use anyhow::{anyhow,bail,Result};
use futures_util::StreamExt;
use std::{collections::{BTreeSet,HashMap},process::Stdio,sync::Arc};
use tokio::{io::{AsyncBufReadExt,BufReader},process::{Child,Command},sync::{watch,Mutex},time::{timeout,sleep,Duration}};
use crate::{model::{self,Feed,Identity},wire::{Node,Value}};

#[derive(Clone,Debug,PartialEq,Default)]
pub struct Device{pub uid:String,pub aliases:Vec<String>,pub name:String,pub model:String,pub group:String,pub address:String,pub port:u16,pub selected:bool}
impl Device{fn matches(&self,uid:&str)->bool{model::same_uid(&self.uid,uid)||self.aliases.iter().any(|v|model::same_uid(v,uid)||v.eq_ignore_ascii_case(uid))}fn target(&self)->String{format!("{}:{}",self.address,self.port)}}
struct Relay{tap:Child,sender:Child,logger:tokio::task::JoinHandle<()>,source:String,content:Option<model::Content>}
impl Relay{
 async fn stop(mut self){
  let ids:Vec<_>=[self.tap.id(),self.sender.id()].into_iter().flatten().map(|v|v.to_string()).collect();
  if !ids.is_empty(){let _=Command::new("/bin/sh").args(["-c",&format!("kill -TERM {}",ids.join(" "))]).status().await;}
  if timeout(Duration::from_secs(3),self.tap.wait()).await.is_err(){let _=self.tap.start_kill();let _=self.tap.wait().await;}
  if timeout(Duration::from_secs(3),self.sender.wait()).await.is_err(){let _=self.sender.start_kill();let _=self.sender.wait().await;}
  self.logger.abort();
 }
 fn ended(&mut self)->bool{!matches!(self.tap.try_wait(),Ok(None))||!matches!(self.sender.try_wait(),Ok(None))}
}
#[derive(Default)]struct State{devices:Vec<Device>,picked:Vec<String>,relay:Option<Relay>}
#[derive(Clone,Default)]pub struct Outputs(Arc<Mutex<State>>);
fn backend()->bool{["source-reader","content-reader.armhf","airplay_sender"].iter().all(|name|std::path::Path::new("/data/local/opt/airplay-sender").join(name).exists())}
pub fn available(feed:&Feed)->bool{feed.output_control}

/// Only the six repeated UID fields carry selection. Never interpret arbitrary strings as UIDs.
pub fn selection(body:&Node,identity:&Identity,devices:&[Device],current:&[String])->Result<Vec<String>>{
 if body.number(1).is_some_and(|v|v!=1){bail!("unsupported output context type")}
 let mut fields:HashMap<u32,Vec<String>>=HashMap::new();
 for(tag,value)in &body.0{if !(2..=7).contains(tag){continue}let Value::Bytes(raw)=value else{bail!("UID wire type")};let uid=std::str::from_utf8(raw)?;if uid.is_empty()||uid.len()>256{bail!("UID size")}
  let canonical=if model::same_uid(uid,&identity.device)||model::same_uid(uid,&identity.group){None}else{Some(devices.iter().find(|d|d.matches(uid)).ok_or_else(||anyhow!("unknown output UID"))?.uid.clone())};
  if let Some(uid)=canonical{fields.entry(*tag).or_default().push(uid);}else{fields.entry(*tag).or_default();}
 }
 let setting=body.0.iter().any(|(t,_)|*t==4||*t==7);
 let mut picked:BTreeSet<String>=if setting{fields.get(&4).into_iter().chain(fields.get(&7)).flatten().cloned().collect()}else{current.iter().cloned().collect()};
 for tag in [2,5]{for uid in fields.get(&tag).into_iter().flatten(){picked.insert(uid.clone());}}
 for tag in [3,6]{for uid in fields.get(&tag).into_iter().flatten(){picked.remove(uid);}}
 if picked.len()>8{bail!("too many output devices")};Ok(picked.into_iter().collect())
}
async fn launch(devices:&[Device],feed:&Feed)->Result<Relay>{
 if devices.is_empty(){bail!("empty remote output set")}
 let mut reader=if feed.source=="native"{
  let content=feed.content.as_ref().ok_or_else(||anyhow!("waiting for native content directive"))?;
  let base="/run/alice-airplay/content";tokio::fs::create_dir_all(base).await?;
  use std::os::unix::fs::PermissionsExt;
  tokio::fs::set_permissions(base,std::fs::Permissions::from_mode(0o700)).await?;
  let client=reqwest::Client::builder().timeout(Duration::from_secs(12)).redirect(reqwest::redirect::Policy::none()).build()?;
  let mut response=client.get(&content.url).send().await.map_err(|_|anyhow!("native content fetch failed"))?.error_for_status().map_err(|_|anyhow!("native content HTTP error"))?;
  let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|_|anyhow!("native content transfer failed"))?{if bytes.len()+chunk.len()>16*1024*1024{bail!("native content size limit")};bytes.extend(chunk);}
  tokio::fs::write(format!("{base}/native.enc"),bytes).await?;tokio::fs::write(format!("{base}/native.key"),&content.key).await?;
  tokio::fs::set_permissions(format!("{base}/native.key"),std::fs::Permissions::from_mode(0o600)).await?;
  let mut command=Command::new("/data/local/opt/airplay-armhf-control/lib/ld-linux-armhf.so.3");
  command.args(["--library-path","/data/local/opt/airplay-armhf-control/lib","/data/local/opt/airplay-sender/content-reader.armhf",&format!("{base}/native.enc"),&format!("{base}/native.key"),&format!("{:.3}",feed.position+0.5),"/run/alice-airplay/duck"]);command
 }else{
  let key=match feed.source.as_str(){"airplay"=>0x415031,"bluetooth"=>0x425431,_=>bail!("unsupported playback source")};
  let mut command=Command::new("/data/local/opt/airplay-sender/source-reader");command.args([key.to_string(),"/run/alice-airplay/duck".into()]);command
 };
 let mut tap=reader.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn()?;
 let input:Stdio=tap.stdout.take().ok_or_else(||anyhow!("tap stdout"))?.try_into()?;
 let mut command=Command::new("/data/local/opt/airplay-sender/airplay_sender");
 for device in devices{command.args(["--to",&device.target()]);}
 // Start every newly selected receiver at the minimum nonzero level.
 let db=std::env::var("MINI2_RELAY_VOLUME_DB").ok().and_then(|s|s.parse::<f32>().ok()).filter(|v|v.is_finite()&&(-30.0..=-25.5).contains(v)).unwrap_or(-29.7);
 let mut sender=command.args(["--volume",&format!("{db:.2}"),"--latency-ms","500"]).stdin(input).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
 let errors=sender.stderr.take().ok_or_else(||anyhow!("sender stderr"))?;
 let expected:BTreeSet<_>=devices.iter().map(Device::target).collect();let(ready,mut rx)=watch::channel(BTreeSet::<String>::new());
 let logger=tokio::spawn(async move{let mut lines=BufReader::new(errors).lines();let mut online=BTreeSet::new();while let Ok(Some(line))=lines.next_line().await{if let Some((target,_))=line.split_once(": streaming "){online.insert(target.to_string());ready.send_replace(online.clone());}eprintln!("AirPlay sender: {line}");}});
 let relay=Relay{tap,sender,logger,source:feed.source.clone(),content:feed.content.clone()};
 let connected=timeout(Duration::from_secs(5),async{loop{if *rx.borrow()==expected{return Ok::<_,anyhow::Error>(())}rx.changed().await?;}}).await;
 if !matches!(connected,Ok(Ok(()))){relay.stop().await;bail!("receiver did not confirm stream startup")};Ok(relay)
}
impl Outputs{
 pub async fn snapshot(&self)->Vec<Device>{let state=self.0.lock().await;state.devices.iter().cloned().map(|mut d|{d.selected=state.picked.contains(&d.uid);d}).collect()}
 pub async fn change(&self,body:&Node,identity:&Identity,feed:&Feed)->u64{
  let mut state=self.0.lock().await;
  let picked=match selection(body,identity,&state.devices,&state.picked){Ok(v)=>v,Err(error)=>{eprintln!("Output selection rejected: {error}");return 39}};
  if picked==state.picked{return 0}
  if !picked.is_empty()&&(!available(feed)||!feed.control_available){return 3}
  if let Some(relay)=state.relay.take(){relay.stop().await;}
  state.picked.clear();
  if !picked.is_empty()&&feed.playing{let targets:Vec<_>=state.devices.iter().filter(|d|picked.contains(&d.uid)).cloned().collect();match launch(&targets,feed).await{Ok(relay)=>state.relay=Some(relay),Err(error)=>{eprintln!("Output selection failed: {error:#}");return 100}}}
  state.picked=picked;0
 }
 pub async fn maintain(&self,feed:&Feed){
  let mut state=self.0.lock().await;
  let _=tokio::fs::create_dir_all("/run/alice-airplay").await;
  let desired=if feed.duck{b"1"}else{b"0"};
  if tokio::fs::read("/run/alice-airplay/duck").await.ok().as_deref()!=Some(desired.as_slice()){let _=tokio::fs::write("/run/alice-airplay/duck",desired).await;}
  if !feed.control_available||!feed.playing{if let Some(relay)=state.relay.take(){relay.stop().await;}return}
  if state.relay.as_ref().is_some_and(|r|r.source!=feed.source||r.content!=feed.content){if let Some(relay)=state.relay.take(){relay.stop().await;}}
  if state.relay.as_mut().is_some_and(Relay::ended){if let Some(relay)=state.relay.take(){relay.stop().await;}state.picked.clear();}
  if state.relay.is_none()&&!state.picked.is_empty(){let targets:Vec<_>=state.devices.iter().filter(|d|state.picked.contains(&d.uid)).cloned().collect();match launch(&targets,feed).await{Ok(relay)=>state.relay=Some(relay),Err(error)=>{state.picked.clear();eprintln!("Output resume failed: {error:#}");}}}
 }
 pub fn start(&self,identity:Identity,mut feed:watch::Receiver<Feed>){
  let discovery=self.clone();tokio::spawn(async move{loop{match discover(&identity).await{Ok(devices)=>{let mut state=discovery.0.lock().await;let mut merged=devices;for d in &state.devices{if state.picked.contains(&d.uid)&&!merged.iter().any(|x|x.uid==d.uid){merged.push(d.clone());}}merged.sort_by(|a,b|a.uid.cmp(&b.uid));state.devices=merged;},Err(error)=>eprintln!("AirPlay output discovery: {error:#}")};sleep(Duration::from_secs(30)).await;}});
  let controller=self.clone();tokio::spawn(async move{loop{let current=feed.borrow_and_update().clone();controller.maintain(&current).await;tokio::select!{_=sleep(Duration::from_millis(500))=>{},v=feed.changed()=>{if v.is_err(){break}}}}});
 }
 pub fn enabled(&self)->bool{backend()}
}

pub async fn discover(identity:&Identity)->Result<Vec<Device>>{
 let address=tokio::fs::read_to_string("/run/dbus/airplay.addr").await?;
 let connection=zbus::connection::Builder::address(address.trim())?.build().await?;
 let rule=zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface("org.freedesktop.Avahi.ServiceBrowser")?.build();
 let mut signals=zbus::MessageStream::for_match_rule(rule,&connection,Some(128)).await?;
 let server=zbus::Proxy::new(&connection,"org.freedesktop.Avahi","/","org.freedesktop.Avahi.Server").await?;
 let path:zbus::zvariant::OwnedObjectPath=server.call("ServiceBrowserNew",&(-1i32,0i32,"_airplay._tcp","local",0u32)).await?;
 let deadline=tokio::time::Instant::now()+Duration::from_secs(4);let mut found=HashMap::new();
 loop{let next=tokio::time::timeout_at(deadline,signals.next()).await;let Ok(Some(Ok(message)))=next else{break};if message.header().path().map(|p|p.as_str())!=Some(path.as_str()){continue}
  let member=message.header().member().map(|v|v.as_str().to_owned()).unwrap_or_default();if member=="AllForNow"{break}if member!="ItemNew"{continue}
  let(interface,protocol,name,typ,domain,_flags):(i32,i32,String,String,String,u32)=message.body().deserialize()?;
  let resolved=timeout(Duration::from_secs(1),server.call::<_,_,(i32,i32,String,String,String,String,i32,String,u16,Vec<Vec<u8>>,u32)>("ResolveService",&(interface,protocol,&name,&typ,&domain,0i32,0u32))).await;
  let Ok(Ok((_,_,_,_,_,_,_,address,port,txt,_)))=resolved else{continue};
  let props:HashMap<_,_>=txt.iter().filter_map(|v|std::str::from_utf8(v).ok()?.split_once('=')).map(|(k,v)|(k.to_string(),v.to_string())).collect();
  let model=props.get("model").cloned().unwrap_or_default();if !model.starts_with("AudioAccessory"){continue}
  let aliases:Vec<_>=["pi","psi","deviceid"].iter().filter_map(|key|props.get(*key).cloned()).collect();
  let Some(uid)=props.get("psi").or_else(||props.get("deviceid")).cloned()else{continue};
  if aliases.iter().any(|uid|model::same_uid(uid,&identity.device)){continue}
  if address.parse::<std::net::Ipv4Addr>().is_err()||port==0{continue}
  found.insert(uid.clone(),Device{uid,aliases,name,model,group:props.get("gid").cloned().unwrap_or_default(),address,port,selected:false});
 }
 let browser=zbus::Proxy::new(&connection,"org.freedesktop.Avahi",path,"org.freedesktop.Avahi.ServiceBrowser").await?;let _:Result<(),_>=browser.call("Free",&()).await;
 Ok(found.into_values().collect())
}

#[cfg(test)]mod tests{
 use super::*;
 fn identity()->Identity{Identity{device:"station".into(),group:"station-group".into()}}
 fn devices()->Vec<Device>{vec![Device{uid:"pod".into(),aliases:vec!["AA:BB:CC:DD:EE:FF".into()],..Default::default()},Device{uid:"pod2".into(),..Default::default()}]}
 #[test]fn add_remove_and_alias(){let mut b=Node::default();b.text(5,"aa:bb:cc:dd:ee:ff");assert_eq!(selection(&b,&identity(),&devices(),&[]).unwrap(),vec!["pod"]);b.text(6,"pod");assert!(selection(&b,&identity(),&devices(),&[]).unwrap().is_empty());}
 #[test]fn setting_self_stops_remote_without_removing_local(){let mut b=Node::default();b.text(7,"station");assert!(selection(&b,&identity(),&devices(),&["pod".into()]).unwrap().is_empty());}
 #[test]fn unknown_output_is_rejected_before_side_effects(){let mut b=Node::default();b.text(2,"unknown");assert!(selection(&b,&identity(),&devices(),&["pod".into()]).is_err());}
 #[test]fn two_outputs_and_repeated_lists_are_deduplicated(){let mut b=Node::default();b.text(2,"pod").text(5,"pod").text(5,"pod2");assert_eq!(selection(&b,&identity(),&devices(),&[]).unwrap(),vec!["pod","pod2"]);}
 #[test]fn wrong_wire_type_is_rejected(){let mut b=Node::default();b.int(2,3);assert!(selection(&b,&identity(),&devices(),&[]).is_err());}
 #[test]fn output_control_is_available_for_every_working_source(){for source in ["native","airplay","bluetooth"]{let feed=Feed{source:source.into(),control_available:true,output_control:true,..Default::default()};assert!(available(&feed));}}
 #[test]fn advertised_descriptors_match_the_reference_schema(){let identity=identity();let feed=Feed{control_available:true,output_control:true,outputs:devices(),..Default::default()};for message in model::topology(&identity,&feed){message.validate("ProtocolMessage").unwrap();}model::device_info(&identity,&feed,None).validate("ProtocolMessage").unwrap();}
 #[test]fn only_selected_devices_are_advertised_as_group_members(){
  let identity=identity();let mut feed=Feed{control_available:true,output_control:true,outputs:devices(),..Default::default()};
  let count=|feed:&Feed|model::topology(&identity,feed)[0].nested(crate::schema::extension(model::kind("UPDATE_OUTPUT_DEVICE_MESSAGE"))).unwrap().0.iter().filter(|(t,_)|*t==3).count();
  assert_eq!(count(&feed),1);feed.outputs[0].selected=true;assert_eq!(count(&feed),2);model::device_info(&identity,&feed,None).validate("ProtocolMessage").unwrap();
 }
 #[test]fn a_current_track_artwork_update_is_valid_without_a_track_change(){
  let identity=identity();let feed=Feed{content_id:"already-playing".into(),cover:Some(model::Cover{data:Arc::new(vec![1,2,3]),id:"cover-digest".into(),mime:"image/jpeg".into(),width:600,height:600}),..Default::default()};
  let message=model::artwork_update(&identity,&feed).unwrap();message.validate("ProtocolMessage").unwrap();assert_eq!(message.number(model::f("ProtocolMessage","type")),Some(model::kind("UPDATE_CONTENT_ITEM_ARTWORK_MESSAGE")as u64));
 }
}
