use anyhow::{bail,Result};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::UnixStream,sync::{watch,Mutex},time::{timeout,Duration,Instant}};
use std::sync::Arc;
use crate::model::Feed;
#[derive(Clone)]
pub struct Control{pub feed:watch::Sender<Feed>,pub pending_volume:Arc<Mutex<Option<(f32,Instant)>>>,pub physical_player:Arc<Mutex<Option<String>>>,pub outputs:crate::outputs::Outputs,volume_request:watch::Sender<Option<f32>>}
pub async fn socket_command(line:&str)->Result<String>{
 let task=async{let mut socket=UnixStream::connect("/run/airplay_control.sock").await?;socket.write_all(format!("{line}\n").as_bytes()).await?;socket.shutdown().await?;let mut result=Vec::new();socket.take(65536).read_to_end(&mut result).await?;Ok::<_,anyhow::Error>(String::from_utf8(result)?)};
 timeout(Duration::from_secs(2),task).await?
}
async fn send_volume(value:f32)->Result<()>{
 let db=if value<=0.0{-144.0}else{value*30.0-30.0};let reply=socket_command(&format!("volume {db:.4}")).await?;
 if !reply.contains("queued"){bail!("volume not accepted")}Ok(())
}
impl Control{
 // A volume slider sends a burst of SetVolume messages. The MRP connection answers at once
 // and one background task forwards only the newest value, so a busy airplay_control never
 // stalls the connection (iOS drops a card whose replies lag).
 pub fn new(feed:watch::Sender<Feed>)->Self{
  let(volume_request,mut requests)=watch::channel(None::<f32>);
  tokio::spawn(async move{while requests.changed().await.is_ok(){let value=*requests.borrow_and_update();if let Some(value)=value{if let Err(error)=send_volume(value).await{eprintln!("MRP volume {value:.3} not applied: {error:#}")}}}});
  Self{feed,pending_volume:Arc::new(Mutex::new(None)),physical_player:Arc::new(Mutex::new(None)),outputs:Default::default(),volume_request}
 }
 pub async fn volume(&self,value:f32)->Result<()>{
  if !value.is_finite()||!(0.0..=1.0).contains(&value){bail!("invalid volume")}
  *self.pending_volume.lock().await=Some((value,Instant::now()));self.feed.send_modify(|feed|feed.volume=value);self.volume_request.send_replace(Some(value));Ok(())
 }
 pub async fn action(&self,command:u64,position:Option<f64>)->Result<()>{
  if command==45{let feed=self.feed.borrow().clone();let prefix=match feed.source.as_str(){"native"=>"native","airplay"=>"airplay",_=>bail!("seek not supported by source")};let position=position.filter(|v|v.is_finite()&&*v>=0.0).ok_or_else(||anyhow::anyhow!("seek position"))?;let reply=socket_command(&format!("{prefix}-seek {position:.3}")).await?;if !reply.starts_with("ok"){bail!("seek not accepted")};return Ok(())}
  let feed=self.feed.borrow().clone();let verb=match command{1=>"play",2=>"pause",3=>if feed.playing{"pause"}else{"play"},4=>"stop",5=>"next",6=>"previous",_=>bail!("unsupported command")};
  match feed.source.as_str(){
   "airplay"=>{let verb=if verb=="stop"{"pause"}else{verb};let reply=socket_command(&format!("airplay-{verb}")).await?;if !reply.starts_with("ok"){bail!("AirPlay source rejected command")}}
   "bluetooth"=>{
    let path=self.physical_player.lock().await.clone().ok_or_else(||anyhow::anyhow!("Bluetooth player unavailable"))?;
    let connection=zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket")?.build().await?;
    let proxy=zbus::Proxy::new(&connection,"org.bluez",path.as_str(),"org.bluez.MediaPlayer1").await?;
    let method=match verb{"play"=>"Play","pause"=>"Pause","stop"=>"Stop","next"=>"Next",_=>"Previous"};
    timeout(Duration::from_secs(2),proxy.call::<_,_,()>(method,&())).await??;
   }
   _=>{let reply=socket_command(&format!("native-{verb}")).await?;if !reply.starts_with("ok"){bail!("Native player rejected command")}}
  }Ok(())
 }
}
