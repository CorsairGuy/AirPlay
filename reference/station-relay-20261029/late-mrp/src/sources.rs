use anyhow::{bail,Result};
use serde_json::Value as Json;
use sha2::{Digest,Sha256};
use std::{collections::HashMap,sync::Arc};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpStream,sync::{watch,Mutex},time::{sleep,timeout,Duration,Instant}};
use zbus::zvariant::{OwnedObjectPath,OwnedValue};
use crate::{control::{socket_command,Control},model::{self,Cover,Feed,Identity,Route},wire::{Node,Value as WireValue}};
#[derive(Clone,Default)]struct Metadata{title:String,artist:String,album:String,position:f64,duration:Option<f64>,cover:Option<Cover>,revision:u64,content:Option<model::Content>,content_title:String,cover_url:String}
#[derive(Clone,Default)]struct Players{phone:Metadata,phone_playing:bool,phone_paused:bool,physical:Metadata,physical_playing:bool,physical_path:Option<String>,updated:Option<Instant>}
fn hash(bytes:&[u8])->String{format!("{:x}",Sha256::digest(bytes))}
fn text(object:&Json,key:&str)->String{object.get(key).and_then(Json::as_str).unwrap_or_default().to_string()}
fn finite(value:Option<f64>)->Option<f64>{value.filter(|v|v.is_finite()&&*v>=0.0)}
fn image(bytes:Vec<u8>)->Result<Cover>{
 if bytes.len()>2*1024*1024{bail!("artwork too large")}
 let(width,height,mime)=if bytes.starts_with(b"\x89PNG\r\n\x1a\n")&&bytes.len()>=24{(u32::from_be_bytes(bytes[16..20].try_into()?),u32::from_be_bytes(bytes[20..24].try_into()?),"image/png")}else if bytes.starts_with(&[255,216]){
  let mut at=2;let mut dimensions=None;while at+4<=bytes.len(){if bytes[at]!=255{break}while at<bytes.len()&&bytes[at]==255{at+=1}let marker=*bytes.get(at).ok_or_else(||anyhow::anyhow!("JPEG marker"))?;at+=1;if marker==217||marker==218{break}if marker==1||(208..=215).contains(&marker){continue}let Some(length)=bytes.get(at..at+2)else{bail!("truncated JPEG segment")};let len=u16::from_be_bytes(length.try_into()?)as usize;if len<2||at+len>bytes.len(){break}if matches!(marker,192|193|194|195|197|198|199|201|202|203|205|206|207)&&len>=7{dimensions=Some((u16::from_be_bytes(bytes[at+5..at+7].try_into()?)as u32,u16::from_be_bytes(bytes[at+3..at+5].try_into()?)as u32,"image/jpeg"));break}at+=len;}dimensions.ok_or_else(||anyhow::anyhow!("JPEG dimensions"))?
 }else{bail!("artwork format")};
 if width==0||height==0||width>8192||height>8192{bail!("artwork dimensions")}
 Ok(Cover{id:hash(&bytes),data:Arc::new(bytes),mime:mime.into(),width,height})
}
fn art_url(url:&reqwest::Url)->bool{url.scheme()=="https"&&url.username().is_empty()&&url.password().is_none()&&url.port().unwrap_or(443)==443&&url.host_str().is_some_and(|h|["yandex.net","yandex.ru","yandex.com","yastatic.net"].iter().any(|base|h==*base||h.ends_with(&format!(".{base}"))))}
fn cover_url(input:&str)->Result<reqwest::Url>{
 let input=input.trim().replace("%%","600x600");
 let absolute=if input.starts_with("//"){format!("https:{input}")}else if !input.contains("://"){format!("https://{input}")}else{input};
 let url=reqwest::Url::parse(&absolute)?;if !art_url(&url){bail!("artwork host")};Ok(url)
}
async fn fetch_cover(client:&reqwest::Client,url:&str)->Result<Cover>{let url=cover_url(url)?;let mut response=client.get(url).send().await?.error_for_status()?;let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{if bytes.len()+chunk.len()>2*1024*1024{bail!("artwork size")};bytes.extend(chunk);}image(bytes)}
fn content_stream(payload:&Json)->Option<model::Content>{
 let stream=payload.get("stream")?;let url=stream.get("url")?.as_str()?;let key=stream.get("decode_key")?.as_str()?;
 let parsed=reqwest::Url::parse(url).ok()?;let host=parsed.host_str()?;
 if parsed.scheme()!="https"||!(host.ends_with(".strm.yandex.net")||host.ends_with(".storage.yandex.net"))||key.len()!=32||!key.bytes().all(|b|b.is_ascii_hexdigit()){return None}
 Some(model::Content{url:url.into(),key:key.into(),id:stream.get("id").and_then(Json::as_str).unwrap_or(url).into()})
}
fn current_content(metadata:&Metadata)->Option<model::Content>{
 if metadata.title==metadata.content_title{metadata.content.clone()}else{None}
}
const NATIVE_CACHE:&str="/run/alice-airplay/native-directive.json";
async fn remember_content(payload:&Json){
 let Some(content)=content_stream(payload)else{return};
 let title=payload.pointer("/metadata/title").and_then(Json::as_str).unwrap_or_default();
 if title.is_empty(){return}
 let saved=serde_json::json!({"metadata":{"title":title},"stream":{"url":content.url,"decode_key":content.key,"id":content.id}});
 let operation=async{
  use std::os::unix::fs::PermissionsExt;
  tokio::fs::create_dir_all("/run/alice-airplay").await?;
  tokio::fs::set_permissions("/run/alice-airplay",std::fs::Permissions::from_mode(0o700)).await?;
  let mut file=tokio::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(format!("{NATIVE_CACHE}.tmp")).await?;
  file.write_all(&serde_json::to_vec(&saved)?).await?;drop(file);
  tokio::fs::rename(format!("{NATIVE_CACHE}.tmp"),NATIVE_CACHE).await?;
  Ok::<_,anyhow::Error>(())
 };
 if operation.await.is_err(){eprintln!("Native content cache unavailable")}
}
async fn restore_content(native:&Arc<Mutex<Metadata>>){
 let Ok(bytes)=tokio::fs::read(NATIVE_CACHE).await else{return};
 if bytes.len()>64*1024{return}
 let Ok(payload)=serde_json::from_slice::<Json>(&bytes)else{return};
 let Some(content)=content_stream(&payload)else{return};
 let title=payload.pointer("/metadata/title").and_then(Json::as_str).unwrap_or_default();
 if title.is_empty(){return}
 let mut state=native.lock().await;
 if state.content.is_none(){state.content=Some(content);state.content_title=title.into();}
}
fn json_objects(data:&[u8],depth:u8,out:&mut Vec<Json>){if depth>9{return}if data.starts_with(b"{"){if let Ok(value)=serde_json::from_slice(data){out.push(value);}return}if let Ok(node)=Node::parse(data){for(_,field)in node.0{if let WireValue::Bytes(bytes)=field{json_objects(&bytes,depth+1,out);}}}}
async fn native_worker(native:Arc<Mutex<Metadata>>,jobs:watch::Sender<Option<(u64,String)>>)->Result<()>{
 restore_content(&native).await;
 let mut connection=TcpStream::connect(("127.0.0.1",9875)).await?;
 loop{let size=connection.read_u32_le().await?as usize;if size>2*1024*1024{bail!("native frame size")};let mut bytes=vec![0;size];connection.read_exact(&mut bytes).await?;let mut objects=Vec::new();json_objects(&bytes,0,&mut objects);
  for object in objects{if let Some(directives)=object.pointer("/response/directives").and_then(Json::as_array){for directive in directives{if directive.get("name").and_then(Json::as_str)!=Some("audio_play"){continue}let Some(payload)=directive.get("payload")else{continue};let metadata=payload.get("metadata").unwrap_or(&Json::Null);let title=text(metadata,"title");if title.is_empty(){continue}remember_content(payload).await;if content_stream(payload).is_some(){eprintln!("Native content directive accepted")}let mut state=native.lock().await;let revision=state.revision.wrapping_add(1);*state=Metadata{content_title:title.clone(),title,artist:text(metadata,"subtitle"),album:text(metadata,"album"),content:content_stream(payload),cover_url:text(metadata,"art_image_url"),position:finite(payload.pointer("/stream/offset_ms").and_then(Json::as_f64).map(|v|v/1000.0)).unwrap_or(0.0),revision,..Default::default()};let url=text(metadata,"art_image_url");jobs.send_replace(if url.is_empty(){None}else{Some((revision,url))});}}}
 }
}
async fn cover_worker(native:Arc<Mutex<Metadata>>,mut jobs:watch::Receiver<Option<(u64,String)>>){
 let client=reqwest::Client::builder().timeout(Duration::from_secs(4)).redirect(reqwest::redirect::Policy::custom(|attempt|if art_url(attempt.url())&&attempt.previous().len()<4{attempt.follow()}else{attempt.stop()})).build().expect("cover client");
 loop{if jobs.changed().await.is_err(){break}loop{let job=jobs.borrow_and_update().clone();let Some((revision,url))=job else{break};tokio::select!{
  changed=jobs.changed()=>{if changed.is_err(){return}continue},
  result=fetch_cover(&client,&url)=>{match result{Ok(cover)=>{let mut current=native.lock().await;if current.revision==revision{println!("Native artwork ready: {}x{}, {} bytes",cover.width,cover.height,cover.data.len());current.cover=Some(cover);}},Err(_)=>eprintln!("Native artwork unavailable: fetch or image validation failed")}break}
 }}}
}
fn value_text(value:Option<&OwnedValue>)->String{value.and_then(|v|<&str>::try_from(v).ok()).unwrap_or_default().to_string()}
fn value_number(value:Option<&OwnedValue>)->Option<f64>{value.and_then(|v|i64::try_from(v).ok().map(|n|n as f64).or_else(||u32::try_from(v).ok().map(|n|n as f64)).or_else(||u64::try_from(v).ok().map(|n|n as f64)))}
fn value_map(value:Option<&OwnedValue>)->HashMap<String,OwnedValue>{value.and_then(|v|v.try_clone().ok()).and_then(|v|HashMap::<String,OwnedValue>::try_from(v).ok()).unwrap_or_default()}
async fn properties(connection:&zbus::Connection,destination:&str,path:&str,interface:&str)->Result<HashMap<String,OwnedValue>>{let proxy=zbus::Proxy::new(connection,destination,path,"org.freedesktop.DBus.Properties").await?;Ok(timeout(Duration::from_secs(2),proxy.call("GetAll",&(interface,))).await??)}
async fn local_cover(uri:&str)->Result<Cover>{
 let prefix="file:///tmp/shairport-sync/.cache/coverart/cover-";let tail=uri.strip_prefix(prefix).ok_or_else(||anyhow::anyhow!("cover path"))?;
 let(stem,ext)=tail.rsplit_once('.').ok_or_else(||anyhow::anyhow!("cover name"))?;if stem.len()!=32||!stem.bytes().all(|b|b.is_ascii_hexdigit())||!matches!(ext,"jpg"|"png"){bail!("cover name")}
 let path=&uri[7..];let metadata=tokio::fs::metadata(path).await?;if metadata.len()>2*1024*1024{bail!("cover size")}image(tokio::fs::read(path).await?)
}
async fn dbus_worker(players:Arc<Mutex<Players>>)->Result<()>{
 let address=tokio::fs::read_to_string("/run/dbus/airplay.addr").await?;let media=zbus::connection::Builder::address(address.trim())?.build().await?;
 let reporter=zbus::Proxy::new(&media,"org.gnome.ShairportSync","/org/gnome/ShairportSync","org.gnome.ShairportSync").await?;
 let _:std::result::Result<(),_>=reporter.call("SetFramePositionUpdateInterval",&(0.2f64,)).await;
 let system=zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket")?.build().await?;let mut cover_uri=String::new();let mut cover=None;
 loop{
  let props=properties(&media,"org.mpris.MediaPlayer2.ShairportSync","/org/mpris/MediaPlayer2","org.mpris.MediaPlayer2.Player").await?;let meta=value_map(props.get("Metadata"));let status=value_text(props.get("PlaybackStatus"));
  let uri=value_text(meta.get("mpris:artUrl"));if uri!=cover_uri{cover=if uri.is_empty(){None}else{local_cover(&uri).await.ok()};cover_uri=uri;}
  let artists=meta.get("xesam:artist").and_then(|v|v.try_clone().ok()).and_then(|v|Vec::<String>::try_from(v).ok()).unwrap_or_default().join(", ");
  let mut phone=Metadata{title:value_text(meta.get("xesam:title")),artist:artists,album:value_text(meta.get("xesam:album")),position:finite(value_number(props.get("Position")).map(|v|v/1e6)).unwrap_or(0.0),duration:finite(value_number(meta.get("mpris:length")).map(|v|v/1e6)),cover:cover.clone(),..Default::default()};
  if let(Ok(root),Ok(remote))=(properties(&media,"org.gnome.ShairportSync","/org/gnome/ShairportSync","org.gnome.ShairportSync").await,properties(&media,"org.gnome.ShairportSync","/org/gnome/ShairportSync","org.gnome.ShairportSync.RemoteControl").await){
   let frame=value_text(root.get("FramePosition"));let progress=value_text(remote.get("ProgressString"));let format=value_text(root.get("SourceFormat"));
   let counter=frame.split('/').next().and_then(|v|v.parse::<u64>().ok()).map(|v|v as u32);let markers:Vec<_>=progress.split('/').filter_map(|v|v.parse::<u64>().ok().map(|v|v as u32)).collect();let rate=format.split('/').nth(1).and_then(|v|v.parse::<u32>().ok()).filter(|v|*v>0);
   if let(Some(counter),Some(rate))=(counter,rate){if markers.len()==3{let elapsed=counter.wrapping_sub(markers[0]);let duration=markers[2].wrapping_sub(markers[0]);if elapsed<=duration&&duration>0{phone.position=elapsed as f64/rate as f64;phone.duration=Some(duration as f64/rate as f64);}}}
  }
  type Objects=HashMap<OwnedObjectPath,HashMap<String,HashMap<String,OwnedValue>>>;
  let proxy=zbus::Proxy::new(&system,"org.bluez","/","org.freedesktop.DBus.ObjectManager").await?;
  let objects:Objects=timeout(Duration::from_secs(2),proxy.call("GetManagedObjects",&())).await??;
  let(mut physical,mut physical_path,mut physical_playing)=(Metadata::default(),None,false);
  for(path,interfaces)in objects{if path.as_str().contains("/dev_02_A1_"){continue}let Some(props)=interfaces.get("org.bluez.MediaPlayer1")else{continue};let status=value_text(props.get("Status"));if !matches!(status.as_str(),"playing"|"paused"){continue}if physical_path.is_some()&&status!="playing"{continue}let track=value_map(props.get("Track"));physical=Metadata{title:value_text(track.get("Title")),artist:value_text(track.get("Artist")),album:value_text(track.get("Album")),position:finite(value_number(props.get("Position")).map(|v|v/1000.0)).unwrap_or(0.0),duration:finite(value_number(track.get("Duration")).map(|v|v/1000.0)),..Default::default()};physical_path=Some(path.to_string());physical_playing=status=="playing";if physical_playing{break}}
  *players.lock().await=Players{phone,phone_playing:status=="Playing",phone_paused:status=="Paused",physical,physical_playing,physical_path,updated:Some(Instant::now())};sleep(Duration::from_millis(500)).await;
 }
}
pub async fn identity()->Result<Identity>{
 let operation=async{let mut socket=TcpStream::connect(("127.0.0.1",7000)).await?;socket.write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n").await?;let mut bytes=Vec::new();let mut chunk=[0u8;4096];let(header,size)=loop{let n=socket.read(&mut chunk).await?;if n==0{bail!("short info")};bytes.extend_from_slice(&chunk[..n]);if bytes.len()>512*1024{bail!("info size")};if let Some(at)=bytes.windows(4).position(|v|v==b"\r\n\r\n"){let text=std::str::from_utf8(&bytes[..at])?;let size=text.lines().find_map(|line|line.split_once(':').filter(|(k,_)|k.eq_ignore_ascii_case("content-length")).and_then(|(_,v)|v.trim().parse::<usize>().ok())).unwrap_or(0);break(at+4,size)}};if size>512*1024{bail!("info size")};while bytes.len()<header+size{let n=socket.read(&mut chunk).await?;if n==0{bail!("short info body")};bytes.extend_from_slice(&chunk[..n]);}let plist=model::decode_plist(&bytes[header..header+size])?;let info=plist.as_dictionary().ok_or_else(||anyhow::anyhow!("info dictionary"))?;let device=info.get("psi").and_then(plist::Value::as_string).ok_or_else(||anyhow::anyhow!("receiver psi"))?;let group=info.get("pi").and_then(plist::Value::as_string).ok_or_else(||anyhow::anyhow!("receiver pi"))?;Ok::<_,anyhow::Error>(Identity{device:device.into(),group:group.into()})};timeout(Duration::from_secs(5),operation).await?
}
async fn route_worker(route:Arc<Mutex<Option<(Route,Instant)>>>)->Result<()>{
 let own=identity().await?;
 let address=tokio::fs::read_to_string("/run/dbus/airplay.addr").await?;
 let bus=zbus::connection::Builder::address(address.trim())?.build().await?;
 let proxy=zbus::Proxy::new(&bus,"org.freedesktop.Avahi","/","org.freedesktop.Avahi.Server").await?;
 type Resolved=(i32,i32,String,String,String,String,i32,String,u16,Vec<Vec<u8>>,u32);
 loop{
  let response:Resolved=timeout(Duration::from_secs(2),proxy.call("ResolveService",&(-1i32,-1i32,model::NAME,"_airplay._tcp","local",-1i32,0u32))).await??;
  let txt:HashMap<String,String>=response.9.into_iter().filter_map(|bytes|String::from_utf8(bytes).ok().and_then(|s|s.split_once('=').map(|(k,v)|(k.to_string(),v.to_string())))).collect();
  if !txt.get("psi").is_some_and(|s|model::same_uid(s,&own.device))||!txt.get("pi").is_some_and(|s|model::same_uid(s,&own.group)){bail!("Resolved AirPlay service identity mismatch")}
  let group=txt.get("gid").filter(|s|uuid::Uuid::parse_str(s).is_ok()).cloned().ok_or_else(||anyhow::anyhow!("AirPlay route group missing"))?;
  let flags=txt.get("flags").and_then(|s|u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s),16).ok()).unwrap_or(0);
  let current=Route{group,active:flags&0x800!=0,leader:txt.get("igl").is_some_and(|s|s=="1"),contains_leader:txt.get("gcgl").is_some_and(|s|s=="1")};
  *route.lock().await=Some((current,Instant::now()));
  sleep(Duration::from_millis(400)).await;
 }
}
/// What the card shows; `bool` is whether it is playing.
#[derive(Debug,PartialEq)]enum Pick{Local(bool),Physical(bool),Phone(bool),Previous}
/// Picks the card source from the previous one (`last`) and the current players.
/// `fresh`: D-Bus data is under 5 s old; `recent`: under 30 s (short hiccups keep the last card).
fn choose(last:&str,native_playing:bool,fresh:bool,recent:bool,p:&Players)->(&'static str,Pick){
 if native_playing{return("native",Pick::Local(true))}
 if fresh&&p.physical_playing{return("bluetooth",Pick::Physical(true))}
 if fresh&&p.phone_playing{return("airplay",Pick::Phone(true))}
 if fresh&&last=="airplay"&&p.phone_paused{return("airplay",Pick::Phone(false))}
 if fresh&&last=="bluetooth"&&p.physical_path.is_some(){return("bluetooth",Pick::Physical(false))}
 // fresh data and the previous external source is neither playing nor paused: it is gone
 if fresh||last=="native"||!recent{return("native",Pick::Local(false))}
 (match last{"airplay"=>"airplay",_=>"bluetooth"},Pick::Previous)
}
pub fn start(control:Control){
 let native=Arc::new(Mutex::new(Metadata::default()));let players=Arc::new(Mutex::new(Players::default()));let(jobs,receiver)=watch::channel(None);let status_jobs=jobs.clone();
 let worker=native.clone();tokio::spawn(cover_worker(worker,receiver));
 let worker=native.clone();tokio::spawn(async move{loop{if let Err(error)=native_worker(worker.clone(),jobs.clone()).await{eprintln!("Native metadata reconnect: {error}")}sleep(Duration::from_secs(2)).await;}});
 let worker=players.clone();tokio::spawn(async move{loop{if let Err(error)=dbus_worker(worker.clone()).await{eprintln!("D-Bus metadata reconnect: {error}")}sleep(Duration::from_secs(2)).await;}});
 let route=Arc::new(Mutex::new(None));let worker=route.clone();
 tokio::spawn(async move{loop{if let Err(error)=route_worker(worker.clone()).await{eprintln!("Route discovery reconnect: {error}")}sleep(Duration::from_secs(2)).await;}});
 tokio::spawn(async move{let mut last="native".to_string();loop{
  let status=socket_command("status").await.ok().and_then(|text|serde_json::from_str::<Json>(&text).ok()).unwrap_or(Json::Null);
  let connected=status.get("connected").and_then(Json::as_bool).unwrap_or(false);let native_playing=connected&&status.get("playing").and_then(Json::as_bool).unwrap_or(false)&&status.get("playerType").and_then(Json::as_str)!=Some("bluetooth");
  // The current state includes coverURI even when playback started before this MRP client.
  let canonical=text(&status,"title");
  if status.get("playerType").and_then(Json::as_str)!=Some("bluetooth")&&!canonical.is_empty(){
   let uri=status.pointer("/player/extra/coverURI").and_then(Json::as_str).unwrap_or_default();
   let mut current=native.lock().await;
   let changed_title=current.title!=canonical;
   if changed_title{current.title=canonical.clone();current.artist=status.pointer("/player/subtitle").and_then(Json::as_str).unwrap_or_default().into();current.cover=None;}
   if !uri.is_empty()&&(current.cover_url!=uri||changed_title){current.cover_url=uri.into();current.revision=current.revision.wrapping_add(1);current.cover=None;status_jobs.send_replace(Some((current.revision,uri.to_string())));}
  }
  let local=native.lock().await.clone();let remote=players.lock().await.clone();let fresh=remote.updated.is_some_and(|at|at.elapsed()<Duration::from_secs(5));
  let recent=remote.updated.is_some_and(|at|at.elapsed()<Duration::from_secs(30));
  let(next,pick)=choose(&last,native_playing,fresh,recent,&remote);
  last=next.into();
  let(mut metadata,playing)=match pick{Pick::Local(on)=>(local,on),Pick::Physical(on)=>(remote.physical.clone(),on),Pick::Phone(on)=>(remote.phone.clone(),on),Pick::Previous=>{let old=control.feed.borrow().clone();(Metadata{title:old.title,artist:old.artist,album:old.album,position:old.position,duration:old.duration,cover:old.cover,..Default::default()},false)}};
  *control.physical_player.lock().await=remote.physical_path;
  let mut position=metadata.position;if last=="native"{position=finite(status.get("progress").and_then(Json::as_f64)).unwrap_or(position)}
  let actual=finite(status.get("volume").and_then(Json::as_f64)).unwrap_or(control.feed.borrow().volume as f64).clamp(0.0,1.0)as f32;
  let mut pending=control.pending_volume.lock().await;let volume=if let Some((desired,at))=*pending{if(actual-desired).abs()<0.01||at.elapsed()>Duration::from_secs(2){*pending=None;actual}else{desired}}else{actual};drop(pending);
  // a Bluetooth player in Alice is our virtual AirPlay device or a real phone; both have their own branch
  let alice_bluetooth=status.get("playerType").and_then(Json::as_str)==Some("bluetooth");
  if last=="native"&&!alice_bluetooth&&!canonical.is_empty()&&canonical!=metadata.title{metadata.title=canonical;metadata.artist.clear();metadata.album.clear();metadata.cover=None;}
  let content=if last=="native"{current_content(&metadata)}else{None};
  let title=metadata.title;
  let duration=metadata.duration.or_else(||if last=="native"{finite(status.get("duration").and_then(Json::as_f64)).filter(|v|*v>0.0)}else{None});
  let content_id=hash(format!("{last}\0{title}\0{}\0{}",metadata.artist,metadata.album).as_bytes());
  let route=route.lock().await.as_ref().filter(|(_,at)|at.elapsed()<Duration::from_secs(5)).map(|(r,_)|r.clone());
  let outputs=control.outputs.snapshot().await;
  let duck=status.get("alice").and_then(Json::as_str).is_some_and(|s|s!="IDLE");
  let feed=Feed{source:last.clone(),title,artist:metadata.artist,album:metadata.album,content_id,position,duration,playing,volume,control_available:connected,cover:metadata.cover,route,output_control:control.outputs.enabled(),outputs,content,duck};
  control.feed.send_if_modified(|old|if *old!=feed{*old=feed;true}else{false});sleep(Duration::from_millis(150)).await;
 }});
}

#[cfg(test)]mod artwork_tests{
 use super::*;
 #[test]fn truncated_jpeg_is_rejected_without_panicking(){
  for count in 2..40{let mut bytes=vec![255,216];bytes.extend(vec![255;count]);bytes.push(192);assert!(image(bytes).is_err());}
 }
 #[test]fn native_yandex_cover_template_is_normalized(){
  assert_eq!(cover_url("avatars.yandex.net/get-music-content/example/%%").unwrap().as_str(),"https://avatars.yandex.net/get-music-content/example/600x600");
  assert_eq!(cover_url("//avatars.mds.yandex.net/example/%%").unwrap().scheme(),"https");
  assert!(cover_url("avatars.yandex.net.evil.invalid/example/%%").is_err());
  assert!(cover_url("http://127.0.0.1/example").is_err());
  assert!(cover_url("https://user:password@avatars.yandex.net/example").is_err());
 }
 #[test]fn invalid_png_dimensions_are_rejected(){
  let mut bytes=vec![0;24];bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");assert!(image(bytes).is_err());
 }
}

#[cfg(test)]mod choose_tests{
 use super::*;
 fn players()->Players{Players::default()}
 #[test]fn native_wins_when_playing(){let mut p=players();p.phone_playing=true;assert_eq!(choose("airplay",true,true,true,&p),("native",Pick::Local(true)));}
 #[test]fn airplay_playing(){let mut p=players();p.phone_playing=true;assert_eq!(choose("native",false,true,true,&p),("airplay",Pick::Phone(true)));}
 #[test]fn airplay_paused_keeps_card(){let mut p=players();p.phone_paused=true;assert_eq!(choose("airplay",false,true,true,&p),("airplay",Pick::Phone(false)));}
 #[test]fn airplay_stopped_returns_to_alice(){assert_eq!(choose("airplay",false,true,true,&players()),("native",Pick::Local(false)));}
 #[test]fn bluetooth_gone_returns_to_alice(){assert_eq!(choose("bluetooth",false,true,true,&players()),("native",Pick::Local(false)));}
 #[test]fn bluetooth_paused_keeps_card(){let mut p=players();p.physical_path=Some("/org/bluez/hci0/dev_X/player0".into());assert_eq!(choose("bluetooth",false,true,true,&p),("bluetooth",Pick::Physical(false)));}
 #[test]fn short_dbus_hiccup_keeps_last_card(){assert_eq!(choose("airplay",false,false,true,&players()),("airplay",Pick::Previous));}
 #[test]fn long_dbus_outage_returns_to_alice(){assert_eq!(choose("airplay",false,false,false,&players()),("native",Pick::Local(false)));}
 #[test]fn physical_beats_phone(){let mut p=players();p.phone_playing=true;p.physical_playing=true;assert_eq!(choose("airplay",false,true,true,&p).1,Pick::Physical(true));}
}

#[cfg(test)]mod native_content_tests{
 use super::*;
 fn sample()->model::Content{model::Content{url:"https://test.strm.yandex.net/audio".into(),key:"0123456789abcdef0123456789abcdef".into(),id:"new".into()}}
 #[test]fn late_status_does_not_discard_next_track_stream(){
  let mut metadata=Metadata{title:"New".into(),content_title:"New".into(),content:Some(sample()),..Default::default()};
  metadata.title="Old".into();assert!(current_content(&metadata).is_none());
  metadata.title="New".into();assert_eq!(current_content(&metadata).unwrap().id,"new");
 }
 #[test]fn stale_cached_stream_never_plays_for_another_title(){
  let metadata=Metadata{title:"Other".into(),content_title:"New".into(),content:Some(sample()),..Default::default()};
  assert!(current_content(&metadata).is_none());
 }
}
