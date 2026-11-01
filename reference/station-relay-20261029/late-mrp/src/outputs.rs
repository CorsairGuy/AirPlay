//! Output selection for the current local mix: Alice, AirPlay and Bluetooth.
use anyhow::{anyhow,bail,Result};
use futures_util::StreamExt;
use std::{collections::{BTreeSet,HashMap},process::Stdio,sync::Arc};
use tokio::{io::{AsyncBufReadExt,BufReader},process::{Child,Command},sync::{watch,Mutex},time::{timeout,sleep,Duration}};
use crate::{model::{self,Feed,Identity},wire::{Node,Value}};

#[derive(Clone,Debug,PartialEq,Default)]
pub struct Device{pub uid:String,pub aliases:Vec<String>,pub name:String,pub model:String,pub group:String,pub address:String,pub port:u16,pub selected:bool,pub volume:f32}
impl Device{fn matches(&self,uid:&str)->bool{model::same_uid(&self.uid,uid)||self.aliases.iter().any(|v|model::same_uid(v,uid)||v.eq_ignore_ascii_case(uid))}fn target(&self)->String{format!("{}:{}",self.address,self.port)}}
struct Relay{tap:Child,sender:Child,logger:tokio::task::JoinHandle<()>,source:String,content:Option<model::Content>}
impl Relay{
 async fn stop(mut self){
  let ids:Vec<_>=[self.tap.id(),self.sender.id()].into_iter().flatten().map(|v|v.to_string()).collect();
  if !ids.is_empty(){let _=Command::new("/bin/sh").args(["-c",&format!("kill -TERM {}",ids.join(" "))]).status().await;}
  async fn reap(child:&mut Child){
   if timeout(Duration::from_secs(3),child.wait()).await.is_err(){let _=child.start_kill();let _=timeout(Duration::from_secs(1),child.wait()).await;}
  }
  tokio::join!(reap(&mut self.tap),reap(&mut self.sender));
  self.logger.abort();
 }
 fn ended(&mut self)->bool{
  // Exit codes are safe diagnostics; never log reader command args or content URLs.
  for (name,child) in [("reader",&mut self.tap),("sender",&mut self.sender)]{
   match child.try_wait(){Ok(None)=>{},Ok(Some(status))=>{eprintln!("AirPlay relay {name} ended: {status}");return true},Err(_)=>{eprintln!("AirPlay relay {name} status unavailable");return true}}
  }false
 }
}
#[derive(Default)]struct State{devices:Vec<Device>,picked:Vec<String>,relay:Option<Relay>,cleanup:Option<tokio::task::JoinHandle<()>>}
impl State{
 fn begin_cleanup(&mut self){
  if let Some(relay)=self.relay.take(){let previous=self.cleanup.take();self.cleanup=Some(tokio::spawn(async move{if let Some(task)=previous{let _=task.await;}relay.stop().await;}));}
 }
 async fn await_cleanup(&mut self){if let Some(task)=self.cleanup.take(){let _=task.await;}}
}
#[derive(Clone)]struct VolumeSetting{address:String,value:f32,revision:u64,applied:u64}
#[derive(Clone,Default)]pub struct Outputs(Arc<Mutex<State>>,Arc<std::sync::Mutex<HashMap<String,VolumeSetting>>>);
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
fn initial_db()->f32{
 std::fs::read_to_string("/run/alice-airplay/relay-volume-db").ok().map(|v|v.trim().to_owned()).or_else(||std::env::var("MINI2_RELAY_VOLUME_DB").ok()).and_then(|s|s.parse::<f32>().ok()).filter(|v|v.is_finite()&&(-30.0..=-20.1).contains(v)).unwrap_or(-29.7)
}
fn db_from_volume(value:f32)->Result<f32>{if !value.is_finite()||!(0.0..=1.0).contains(&value){bail!("invalid output volume")};Ok(if value==0.0{-144.0}else{value*30.0-30.0})}
pub fn same_devices(a:&[Device],b:&[Device])->bool{a.len()==b.len()&&a.iter().zip(b).all(|(x,y)|{let mut x=x.clone();let mut y=y.clone();x.volume=0.0;y.volume=0.0;x==y})}
async fn write_volume(address:&str,value:f32)->Result<()>{
 let address=address.parse::<std::net::IpAddr>()?;let db=db_from_volume(value)?;
 use std::os::unix::fs::PermissionsExt;
 let base="/run/alice-airplay/output-volumes";tokio::fs::create_dir_all(base).await?;
 tokio::fs::set_permissions(base,std::fs::Permissions::from_mode(0o700)).await?;
 let path=format!("{base}/{address}.db");let tmp=format!("{path}.tmp");
 tokio::fs::write(&tmp,format!("{db:.6}")).await?;tokio::fs::set_permissions(&tmp,std::fs::Permissions::from_mode(0o600)).await?;tokio::fs::rename(tmp,path).await?;Ok(())
}
async fn launch(devices:&[Device],feed:&Feed)->Result<Relay>{
 if devices.is_empty(){bail!("empty remote output set")}
 // The volume worker is the sole writer; Outputs::start_relay gates launch on its revisions.
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
 let db=initial_db();
 let mut sender=command.args(["--volume",&format!("{db:.2}"),"--latency-ms","500"]).stdin(input).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
 let errors=sender.stderr.take().ok_or_else(||anyhow!("sender stderr"))?;
 let expected:BTreeSet<_>=devices.iter().map(Device::target).collect();let(ready,mut rx)=watch::channel(BTreeSet::<String>::new());
 let logger=tokio::spawn(async move{let mut lines=BufReader::new(errors).lines();let mut online=BTreeSet::new();while let Ok(Some(line))=lines.next_line().await{if let Some((target,_))=line.split_once(": streaming "){online.insert(target.to_string());ready.send_replace(online.clone());}eprintln!("AirPlay sender: {line}");}});
 let relay=Relay{tap,sender,logger,source:feed.source.clone(),content:feed.content.clone()};
 let connected=timeout(Duration::from_secs(5),async{loop{if *rx.borrow()==expected{return Ok::<_,anyhow::Error>(())}rx.changed().await?;}}).await;
 if !matches!(connected,Ok(Ok(()))){relay.stop().await;bail!("receiver did not confirm stream startup")};Ok(relay)
}
impl Outputs{
 fn queue_start_volumes(&self,devices:&[Device])->Result<Vec<(String,u64)>>{
  let mut volumes=self.1.lock().unwrap();let mut requested=Vec::new();
  for device in devices{
   db_from_volume(device.volume)?;device.address.parse::<std::net::IpAddr>()?;
   let entry=volumes.entry(device.uid.clone()).or_insert(VolumeSetting{address:device.address.clone(),value:device.volume,revision:1,applied:0});
   // Existing slider value wins over a possibly stale discovery/selection snapshot.
   if entry.address!=device.address{entry.address=device.address.clone();entry.revision=entry.revision.checked_add(1).ok_or_else(||anyhow!("volume revision exhausted"))?;}
   requested.push((device.uid.clone(),entry.revision));
  }Ok(requested)
 }
 async fn await_start_volumes(&self,requested:&[(String,u64)])->Result<()>{
  timeout(Duration::from_secs(2),async{
   loop{let applied={let values=self.1.lock().unwrap();requested.iter().all(|(uid,revision)|values.get(uid).is_some_and(|v|v.applied>=*revision))};
    if applied{return}sleep(Duration::from_millis(10)).await;
   }
  }).await.map_err(|_|anyhow!("output volume initialization timed out"))?;Ok(())
 }
 async fn start_relay(&self,devices:&[Device],feed:&Feed)->Result<Relay>{
  let requested=self.queue_start_volumes(devices)?;
  self.await_start_volumes(&requested).await?;
  launch(devices,feed).await
 }
 pub async fn snapshot(&self)->Vec<Device>{let state=self.0.lock().await;state.devices.iter().cloned().map(|mut d|{d.selected=state.picked.contains(&d.uid);d.volume=self.1.lock().unwrap().get(&d.uid).map(|v|v.value).unwrap_or((initial_db()+30.0)/30.0);d}).collect()}
 pub fn set_volume(&self,device:&Device,value:f32)->Result<()>{
  db_from_volume(value)?;device.address.parse::<std::net::IpAddr>()?;
  let mut volumes=self.1.lock().unwrap();let entry=volumes.entry(device.uid.clone()).or_insert(VolumeSetting{address:device.address.clone(),value,revision:0,applied:0});
  entry.address=device.address.clone();entry.value=value;entry.revision=entry.revision.checked_add(1).ok_or_else(||anyhow!("volume revision exhausted"))?;Ok(())
 }
 pub fn volume(&self,uid:&str)->Option<f32>{self.1.lock().unwrap().get(uid).map(|v|v.value)}
 pub async fn change(&self,body:&Node,identity:&Identity,feed:&Feed)->u64{
  let mut state=self.0.lock().await;
  let picked=match selection(body,identity,&state.devices,&state.picked){Ok(v)=>v,Err(error)=>{eprintln!("Output selection rejected: {error}");return 39}};
  if picked==state.picked{return 0}
  if !picked.is_empty()&&(!available(feed)||!feed.control_available){return 3}
  state.picked.clear();
  state.begin_cleanup();
  // Release the state lock and acknowledge deselection without waiting for RTSP.
  if picked.is_empty(){return 0}
  // Old sender TEARDOWN must finish before any replacement session is created.
  state.await_cleanup().await;
  if !picked.is_empty()&&feed.playing{let targets:Vec<_>=state.devices.iter().filter(|d|picked.contains(&d.uid)).map(|d|{let mut d=d.clone();d.volume=self.1.lock().unwrap().get(&d.uid).map(|v|v.value).unwrap_or((initial_db()+30.0)/30.0);d}).collect();match self.start_relay(&targets,feed).await{Ok(relay)=>state.relay=Some(relay),Err(error)=>{eprintln!("Output selection failed: {error:#}");return 100}}}
  state.picked=picked;0
 }
 pub async fn maintain(&self,feed:&Feed){
  let mut state=self.0.lock().await;
  let _=tokio::fs::create_dir_all("/run/alice-airplay").await;
  let desired=if feed.duck{b"1"}else{b"0"};
  if tokio::fs::read("/run/alice-airplay/duck").await.ok().as_deref()!=Some(desired.as_slice()){let _=tokio::fs::write("/run/alice-airplay/duck",desired).await;}
  if !feed.control_available||!feed.playing{state.begin_cleanup();return}
  // A directive and the canonical player status arrive independently. Keep
  // the chosen outputs while the next native stream is not yet identifiable.
  if state.relay.as_ref().is_some_and(|r|r.source!=feed.source){state.begin_cleanup();}
  if feed.source=="native"&&feed.content.is_none(){return}
  if state.relay.as_ref().is_some_and(|r|r.source!=feed.source||r.content!=feed.content){state.begin_cleanup();}
  if state.relay.as_mut().is_some_and(Relay::ended){state.begin_cleanup();state.picked.clear();}
  if state.relay.is_none()&&!state.picked.is_empty(){state.await_cleanup().await;let targets:Vec<_>=state.devices.iter().filter(|d|state.picked.contains(&d.uid)).map(|d|{let mut d=d.clone();d.volume=self.1.lock().unwrap().get(&d.uid).map(|v|v.value).unwrap_or((initial_db()+30.0)/30.0);d}).collect();match self.start_relay(&targets,feed).await{Ok(relay)=>state.relay=Some(relay),Err(error)=>{state.picked.clear();eprintln!("Output resume failed: {error:#}");}}}
 }
 pub fn start(&self,identity:Identity,mut feed:watch::Receiver<Feed>){
  let volumes=self.1.clone();tokio::spawn(async move{loop{
   let requests:Vec<_>={let values=volumes.lock().unwrap();values.iter().filter(|(_,v)|v.revision!=v.applied).map(|(uid,v)|(uid.clone(),v.clone())).collect()};
   let mut failed=false;
   for(uid,request)in requests{if write_volume(&request.address,request.value).await.is_ok(){let mut values=volumes.lock().unwrap();if let Some(value)=values.get_mut(&uid){value.applied=request.revision;}}else{failed=true;}}
   sleep(Duration::from_millis(if failed{500}else{50})).await;
  }});

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
  found.insert(uid.clone(),Device{uid,aliases,name,model,group:props.get("gid").cloned().unwrap_or_default(),address,port,selected:false,volume:(initial_db()+30.0)/30.0});
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

#[cfg(test)]mod live_volume_tests{
 use super::*;
 #[test]fn only_finite_normalized_volume_is_accepted(){assert!(db_from_volume(f32::NAN).is_err());assert!(db_from_volume(-0.1).is_err());assert!(db_from_volume(1.1).is_err());assert_eq!(db_from_volume(0.0).unwrap(),-144.0);assert_eq!(db_from_volume(0.25).unwrap(),-22.5);}
 #[test]fn volume_updates_do_not_look_like_topology_changes(){let a=Device{uid:"pod".into(),selected:true,volume:0.25,..Default::default()};let mut b=a.clone();b.volume=0.5;assert!(same_devices(&[a.clone()],&[b.clone()]));b.selected=false;assert!(!same_devices(&[a],&[b]));}
}

#[cfg(test)]mod pending_volume_tests{
 use super::*;
 #[test]fn rapid_slider_commands_keep_independent_latest_values(){let outputs=Outputs::default();let a=Device{uid:"a".into(),address:"10.0.0.1".into(),..Default::default()};let b=Device{uid:"b".into(),address:"10.0.0.2".into(),..Default::default()};for step in 0..=33{outputs.set_volume(&a,step as f32/100.0).unwrap();}outputs.set_volume(&b,0.2).unwrap();assert_eq!(outputs.volume("a"),Some(0.33));assert_eq!(outputs.volume("b"),Some(0.2));}
}

#[cfg(test)]mod lifecycle_tests{
 use super::*;
 fn pod()->Device{Device{uid:"pod".into(),address:"10.0.0.1".into(),volume:0.1,..Default::default()}}
 #[test]fn selection_does_not_overwrite_queued_slider(){
  let o=Outputs::default();let d=pod();o.set_volume(&d,0.77).unwrap();
  let r=o.queue_start_volumes(&[d]).unwrap();let v=o.1.lock().unwrap();
  assert_eq!(v["pod"].value,0.77);assert_eq!(r[0].1,v["pod"].revision);assert_eq!(v["pod"].applied,0);
 }
 #[test]fn new_member_is_pending_and_address_change_requeues(){
  let o=Outputs::default();let mut d=pod();let r=o.queue_start_volumes(&[d.clone()]).unwrap();
  assert_eq!(r[0].1,1);assert_eq!(o.1.lock().unwrap()["pod"].applied,0);
  d.address="10.0.0.2".into();let r=o.queue_start_volumes(&[d]).unwrap();assert_eq!(r[0].1,2);
 }
 #[tokio::test]async fn launch_barrier_accepts_newer_applied_revision(){
  let o=Outputs::default();let d=pod();let r=o.queue_start_volumes(&[d.clone()]).unwrap();
  o.set_volume(&d,0.8).unwrap();let copy=o.clone();
  tokio::spawn(async move{sleep(Duration::from_millis(40)).await;let mut v=copy.1.lock().unwrap();let e=v.get_mut("pod").unwrap();e.applied=e.revision;});
  let before=tokio::time::Instant::now();o.await_start_volumes(&r).await.unwrap();assert!(before.elapsed()>=Duration::from_millis(35));
 }
 #[tokio::test]async fn deselect_does_not_wait_for_old_teardown(){
  let o=Outputs::default();{let mut s=o.0.lock().await;s.devices=vec![pod()];s.picked=vec!["pod".into()];s.cleanup=Some(tokio::spawn(async{sleep(Duration::from_millis(200)).await;}));}
  let mut body=Node::default();body.text(7,"station");let id=Identity{device:"station".into(),group:"group".into()};
  let before=tokio::time::Instant::now();assert_eq!(o.change(&body,&id,&Feed::default()).await,0);
  assert!(before.elapsed()<Duration::from_millis(50));let mut s=o.0.lock().await;assert!(s.picked.is_empty());assert!(s.cleanup.is_some());
  s.await_cleanup().await;assert!(before.elapsed()>=Duration::from_millis(180));assert!(s.cleanup.is_none());
 }
}

#[cfg(test)]mod removal_request_regressions{
 use super::*;
 fn identity()->Identity{Identity{device:"station".into(),group:"group".into()}}
 fn devices()->Vec<Device>{vec![Device{uid:"pod".into(),..Default::default()},Device{uid:"pod2".into(),..Default::default()}]}
// Additional regression tests to paste into outputs.rs tests module.
#[test]fn removal_wins_across_normal_and_cluster_lists_in_any_wire_order(){
 for fields in [vec![(2,"pod"),(6,"pod")],vec![(6,"pod"),(2,"pod")],vec![(5,"pod"),(3,"pod")],vec![(3,"pod"),(5,"pod")]]{
  let mut body=Node::default();for(tag,uid)in fields{body.text(tag,uid);}
  assert!(selection(&body,&identity(),&devices(),&["pod".into()]).unwrap().is_empty());
 }
}
#[test]fn setting_local_plus_remote_removal_clears_selected_remote(){
 let mut body=Node::default();body.text(4,"station").text(6,"pod");
 assert!(selection(&body,&identity(),&devices(),&["pod".into()]).unwrap().is_empty());
}
#[test]fn remove_only_retains_other_selected_device(){
 let mut body=Node::default();body.int(1,1).text(6,"pod");
 assert_eq!(selection(&body,&identity(),&devices(),&["pod".into(),"pod2".into()]).unwrap(),vec!["pod2"]);
}
#[test]fn explicit_unsupported_type_rejected(){
 let mut body=Node::default();body.int(1,2).text(6,"pod");
 assert!(selection(&body,&identity(),&devices(),&["pod".into()]).is_err());
}

}
