//! Mini 2 AirPlay sender: streams 44.1 kHz S16LE stereo PCM from stdin to AirPlay 2 receivers
//! (HomePod and similar) using transient pairing and realtime (type 96) streams.
//!
//! Usage: airplay_sender --to 10.10.30.121 [--to host[:port]] [--volume -20] [--latency-ms 1500]
use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::{aead::{Aead, KeyInit, Payload}, ChaCha20Poly1305, Key, Nonce};
use num_bigint::BigUint;
use rand::RngCore;
use sha2::{Digest, Sha512};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream, UdpSocket}, sync::{broadcast, watch}, time::{sleep, timeout}};

const RATE: u64 = 44100;
const FRAMES_PER_PACKET: usize = 352;
const PACKET_BYTES: usize = FRAMES_PER_PACKET * 4;
const BACKLOG: usize = 512;
const N_HEX: &str = concat!(
 "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B",
 "139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485",
 "B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1F",
 "E649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23",
 "DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32",
 "905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF69558",
 "17183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33A85521",
 "ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7ABF5AE8CDB0933D7",
 "1E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864D87602733EC86A64521F2B1817",
 "7B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB3143DB5BFCE0FD108E4B82",
 "D120A93AD2CAFFFFFFFFFFFFFFFF");

// ---------------------------------------------------------------- crypto helpers
fn sha512(parts:&[&[u8]])->Vec<u8>{let mut h=Sha512::new();for p in parts{h.update(p);}h.finalize().to_vec()}
fn hkdf(salt:&str,info:&str,secret:&[u8])->[u8;32]{let mut out=[0u8;32];hkdf::Hkdf::<Sha512>::new(Some(salt.as_bytes()),secret).expand(info.as_bytes(),&mut out).expect("32-byte HKDF output");out}
fn pad(n:&BigUint,len:usize)->Vec<u8>{let b=n.to_bytes_be();let mut v=vec![0u8;len.saturating_sub(b.len())];v.extend(b);v}
fn nonce(counter:u64)->[u8;12]{let mut n=[0u8;12];n[4..].copy_from_slice(&counter.to_le_bytes());n}

/// SRP-6a client exactly as pair_ap (owntone) does it for HomeKit transient pairing.
struct Srp{n:BigUint,g:BigUint,a:BigUint,pub_a:BigUint}
impl Srp{
 fn new()->Srp{
  let n=BigUint::parse_bytes(N_HEX.as_bytes(),16).expect("prime");let g=BigUint::from(5u32);
  let mut a=[0u8;32];rand::thread_rng().fill_bytes(&mut a);let a=BigUint::from_bytes_be(&a);
  let pub_a=g.modpow(&a,&n);Srp{n,g,a,pub_a}
 }
 /// Returns (session key K, client proof M1).
 fn process(&self,salt:&[u8],pub_b:&[u8],pin:&str)->Result<(Vec<u8>,Vec<u8>)>{
  let (n,g)=(&self.n,&self.g);let len=384;
  let b=BigUint::from_bytes_be(pub_b);let s=BigUint::from_bytes_be(salt);
  if (&b%n)==BigUint::from(0u32){bail!("invalid SRP B")}
  let k=BigUint::from_bytes_be(&sha512(&[&pad(n,len),&pad(g,len)]));
  let u=BigUint::from_bytes_be(&sha512(&[&pad(&self.pub_a,len),&pad(&b,len)]));
  let inner=sha512(&[format!("Pair-Setup:{pin}").as_bytes()]);
  let x=BigUint::from_bytes_be(&sha512(&[&s.to_bytes_be(),&inner]));
  let v=g.modpow(&x,n);let kv=(k*v)%n;let base=((&b%n)+n-kv)%n;
  let secret=base.modpow(&(&self.a+u*x),n);
  let key=sha512(&[&secret.to_bytes_be()]);
  let hn=sha512(&[&n.to_bytes_be()]);let hg=sha512(&[&g.to_bytes_be()]);
  let xor:Vec<u8>=hn.iter().zip(hg.iter()).map(|(a,b)|a^b).collect();
  let hi=sha512(&[b"Pair-Setup"]);
  let m1=sha512(&[&xor,&hi,&s.to_bytes_be(),&self.pub_a.to_bytes_be(),&b.to_bytes_be(),&key]);
  Ok((key,m1))
 }
}

fn tlv_encode(items:&[(u8,&[u8])])->Vec<u8>{
 let mut out=Vec::new();
 for(t,v)in items{if v.is_empty(){out.extend([*t,0]);continue}for chunk in v.chunks(255){out.push(*t);out.push(chunk.len() as u8);out.extend(chunk);}}
 out
}
fn tlv_decode(data:&[u8])->HashMap<u8,Vec<u8>>{
 let mut map:HashMap<u8,Vec<u8>>=HashMap::new();let mut i=0;
 while i+2<=data.len(){let(t,l)=(data[i],data[i+1] as usize);let end=(i+2+l).min(data.len());map.entry(t).or_default().extend(&data[i+2..end]);i=end;}
 map
}

// ---------------------------------------------------------------- HAP-framed connection
struct Cipher{out:ChaCha20Poly1305,inp:ChaCha20Poly1305,out_n:u64,in_n:u64}
impl Cipher{fn new(out:&[u8;32],inp:&[u8;32])->Cipher{Cipher{out:ChaCha20Poly1305::new(Key::from_slice(out)),inp:ChaCha20Poly1305::new(Key::from_slice(inp)),out_n:0,in_n:0}}}

struct Response{status:u16,headers:HashMap<String,String>,body:Vec<u8>}
impl Response{fn header(&self,name:&str)->Option<&str>{self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)}}

struct Conn{stream:TcpStream,cipher:Option<Cipher>,raw:Vec<u8>,plain:Vec<u8>,idle_ok:bool}
impl Conn{
 async fn send(&mut self,data:&[u8])->Result<()>{
  match&mut self.cipher{
   None=>self.stream.write_all(data).await?,
   Some(c)=>{let mut out=Vec::new();for frame in data.chunks(1024){let len=(frame.len() as u16).to_le_bytes();
    let sealed=c.out.encrypt(Nonce::from_slice(&nonce(c.out_n)),Payload{msg:frame,aad:&len}).map_err(|_|anyhow!("encrypt"))?;c.out_n+=1;out.extend(len);out.extend(sealed);}
    self.stream.write_all(&out).await?}
  }
  Ok(())
 }
 /// Moves decrypted bytes into `plain`; returns false on EOF.
 async fn fill(&mut self)->Result<bool>{
  let mut buf=[0u8;4096];let n=self.stream.read(&mut buf).await?;if n==0{return Ok(false)}
  match&mut self.cipher{
   None=>self.plain.extend(&buf[..n]),
   Some(c)=>{self.raw.extend(&buf[..n]);
    while self.raw.len()>=2{let len=u16::from_le_bytes([self.raw[0],self.raw[1]]) as usize;if self.raw.len()<2+len+16{break}
     let aad=[self.raw[0],self.raw[1]];let block:Vec<u8>=self.raw[2..2+len+16].to_vec();
     let open=c.inp.decrypt(Nonce::from_slice(&nonce(c.in_n)),Payload{msg:&block,aad:&aad}).map_err(|_|anyhow!("decrypt"))?;c.in_n+=1;
     self.plain.extend(open);self.raw.drain(..2+len+16);}
   }
  }
  Ok(true)
 }
 async fn read_message(&mut self)->Result<(String,HashMap<String,String>,Vec<u8>)>{
  loop{
   if let Some(end)=find(&self.plain,b"\r\n\r\n"){
    let head=String::from_utf8_lossy(&self.plain[..end]).to_string();let mut lines=head.split("\r\n");let first=lines.next().unwrap_or_default().to_string();
    let headers:HashMap<String,String>=lines.filter_map(|l|l.split_once(':').map(|(k,v)|(k.trim().to_ascii_lowercase(),v.trim().to_string()))).collect();
    let len:usize=headers.get("content-length").and_then(|v|v.parse().ok()).unwrap_or(0);
    if self.plain.len()>=end+4+len{let body=self.plain[end+4..end+4+len].to_vec();self.plain.drain(..end+4+len);return Ok((first,headers,body))}
   }
   // the event channel idles for long stretches; only request/response traffic gets a deadline
   let more=if self.idle_ok{self.fill().await?}else{timeout(Duration::from_secs(8),self.fill()).await.context("receiver response timeout")??};
   if !more{bail!("connection closed")}
  }
 }
 async fn read_response(&mut self)->Result<Response>{
  let(first,headers,body)=self.read_message().await?;
  let status=first.split_whitespace().nth(1).and_then(|s|s.parse().ok()).ok_or_else(||anyhow!("bad status line {first}"))?;
  Ok(Response{status,headers,body})
 }
}
fn find(h:&[u8],n:&[u8])->Option<usize>{h.windows(n.len()).position(|w|w==n)}

// ---------------------------------------------------------------- RTSP session
struct Rtsp{conn:Conn,cseq:u32,uri:String,dacp:String,active_remote:u32}
impl Rtsp{
 async fn http_post(&mut self,path:&str,body:&[u8])->Result<Response>{
  let head=format!("POST {path} HTTP/1.1\r\nUser-Agent: AirPlay/320.20\r\nConnection: keep-alive\r\nX-Apple-HKP: 4\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",body.len());
  let mut msg=head.into_bytes();msg.extend(body);self.conn.send(&msg).await?;self.conn.read_response().await
 }
 async fn request(&mut self,method:&str,uri:Option<&str>,content_type:Option<&str>,body:&[u8],extra:&[(&str,String)])->Result<Response>{
  let uri=uri.map(str::to_string).unwrap_or_else(||self.uri.clone());let cseq=self.cseq;self.cseq+=1;
  let mut head=format!("{method} {uri} RTSP/1.0\r\nCSeq: {cseq}\r\nUser-Agent: AirPlay/550.10\r\nDACP-ID: {}\r\nActive-Remote: {}\r\nClient-Instance: {}\r\n",self.dacp,self.active_remote,self.dacp);
  for(k,v)in extra{head.push_str(&format!("{k}: {v}\r\n"));}
  if let Some(ct)=content_type{head.push_str(&format!("Content-Type: {ct}\r\n"));}
  head.push_str(&format!("Content-Length: {}\r\n\r\n",body.len()));
  let mut msg=head.into_bytes();msg.extend(body);self.conn.send(&msg).await?;
  let resp=self.conn.read_response().await?;
  if resp.status!=200{bail!("{method} {uri} -> {}",resp.status)}
  Ok(resp)
 }
}
fn bplist(v:plist::Value)->Result<Vec<u8>>{let mut out=Vec::new();v.to_writer_binary(&mut out)?;Ok(out)}
fn dict(items:Vec<(&str,plist::Value)>)->plist::Value{plist::Value::Dictionary(items.into_iter().map(|(k,v)|(k.to_string(),v)).collect())}

// ---------------------------------------------------------------- timing
fn ntp_now()->u64{let d=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();((d.as_secs()+0x83AA7E80)<<32)|(((d.subsec_nanos() as u64)<<32)/1_000_000_000)}
fn ntp2ts(ntp:u64)->u64{(((ntp>>16) as u128*RATE as u128)>>16) as u64}
fn ts2ntp(ts:u64)->u64{((((ts as u128)<<16)/RATE as u128)<<16) as u64}

async fn timing_server(sock:Arc<UdpSocket>){
 let mut buf=[0u8;128];
 loop{
  let Ok((n,from))=sock.recv_from(&mut buf).await else{return};if n<32{continue}
  let now=ntp_now();let mut resp=[0u8;32];resp[0]=buf[0];resp[1]=0xD3;resp[2..4].copy_from_slice(&7u16.to_be_bytes());
  resp[8..16].copy_from_slice(&buf[24..32]);resp[16..24].copy_from_slice(&now.to_be_bytes());resp[24..32].copy_from_slice(&now.to_be_bytes());
  let _=sock.send_to(&resp,from).await;
 }
}

// ---------------------------------------------------------------- one receiver
struct Target{addr:SocketAddr,volume:f32,latency:u64}

async fn event_channel(addr:SocketAddr,out:[u8;32],inp:[u8;32]){
 let Ok(stream)=TcpStream::connect(addr).await else{eprintln!("{addr}: event channel connect failed");return};
 let mut conn=Conn{stream,cipher:Some(Cipher::new(&out,&inp)),raw:Vec::new(),plain:Vec::new(),idle_ok:true};
 while let Ok((_,headers,_))=conn.read_message().await{
  let cseq=headers.get("cseq").cloned().unwrap_or_else(||"0".into());
  let reply=format!("RTSP/1.0 200 OK\r\nContent-Length: 0\r\nAudio-Latency: 0\r\nCSeq: {cseq}\r\n\r\n");
  if conn.send(reply.as_bytes()).await.is_err(){break}
 }
}

async fn stream_to(target:&Target,audio:&mut broadcast::Receiver<Arc<Vec<u8>>>,stop:&mut watch::Receiver<bool>)->Result<()>{
 let host=target.addr;
 let stream=timeout(Duration::from_secs(5),TcpStream::connect(host)).await.context("connect timeout")??;stream.set_nodelay(true)?;
 let local_ip=stream.local_addr()?.ip();
 // ThreadRng is not Send, so draw everything random before the first await below
 let(session_id,dacp,active_remote,uuid_tail,first_seq)={let mut rng=rand::thread_rng();(rng.next_u32(),format!("{:016X}",rng.next_u64()),rng.next_u32(),rng.next_u64()&0xFFFF_FFFF_FFFF,rng.next_u32() as u16)};
 let mut rtsp=Rtsp{conn:Conn{stream,cipher:None,raw:Vec::new(),plain:Vec::new(),idle_ok:false},cseq:0,uri:format!("rtsp://{local_ip}/{session_id}"),dacp,active_remote};

 // transient pair-setup (M1..M4) with the well-known PIN 3939
 let srp=Srp::new();
 rtsp.http_post("/pair-pin-start",&[]).await?;
 let m2=rtsp.http_post("/pair-setup",&tlv_encode(&[(0,&[0]),(6,&[1]),(19,&[0x10])])).await?;
 let m2=tlv_decode(&m2.body);if let Some(e)=m2.get(&7){bail!("pair-setup M2 error {e:?}")}
 let(salt,pub_b)=(m2.get(&2).ok_or_else(||anyhow!("no salt"))?,m2.get(&3).ok_or_else(||anyhow!("no B"))?);
 let(key,proof)=srp.process(salt,pub_b,"3939")?;
 let m4=rtsp.http_post("/pair-setup",&tlv_encode(&[(6,&[3]),(3,&srp.pub_a.to_bytes_be()),(4,&proof)])).await?;
 let m4=tlv_decode(&m4.body);if let Some(e)=m4.get(&7){bail!("pair-setup M4 error {e:?} (wrong PIN or access restricted)")}
 rtsp.conn.cipher=Some(Cipher::new(&hkdf("Control-Salt","Control-Write-Encryption-Key",&key),&hkdf("Control-Salt","Control-Read-Encryption-Key",&key)));

 let timing=Arc::new(UdpSocket::bind((local_ip,0)).await?);let control=Arc::new(UdpSocket::bind((local_ip,0)).await?);
 let timing_task=tokio::spawn(timing_server(timing.clone()));
 let setup=dict(vec![
  ("deviceID",plist::Value::String("AA:BB:CC:DD:EE:01".into())),("sessionUUID",plist::Value::String(format!("{:08X}-0000-4000-8000-{:012X}",session_id,uuid_tail))),
  ("timingPort",plist::Value::Integer((timing.local_addr()?.port() as u64).into())),("timingProtocol",plist::Value::String("NTP".into())),
  ("isMultiSelectAirPlay",plist::Value::Boolean(true)),("groupContainsGroupLeader",plist::Value::Boolean(false)),
  ("macAddress",plist::Value::String("AA:BB:CC:DD:EE:01".into())),("model",plist::Value::String("AudioAccessory5,1".into())),
  ("name",plist::Value::String("Станция Мини".into())),("osBuildVersion",plist::Value::String("20F66".into())),("osName",plist::Value::String("iPhone OS".into())),
  ("osVersion",plist::Value::String("16.5".into())),("senderSupportsRelay",plist::Value::Boolean(false)),("sourceVersion",plist::Value::String("690.7.1".into())),
  ("statsCollectionEnabled",plist::Value::Boolean(false))]);
 let resp=rtsp.request("SETUP",None,Some("application/x-apple-binary-plist"),&bplist(setup)?,&[]).await?;
 let info=plist::Value::from_reader(std::io::Cursor::new(&resp.body)).ok();
 let event_port=info.as_ref().and_then(|v|v.as_dictionary()).and_then(|d|d.get("eventPort")).and_then(|v|v.as_unsigned_integer()).unwrap_or(0) as u16;
 let event_task=(event_port!=0).then(||tokio::spawn(event_channel(SocketAddr::new(host.ip(),event_port),hkdf("Events-Salt","Events-Read-Encryption-Key",&key),hkdf("Events-Salt","Events-Write-Encryption-Key",&key))));
 sleep(Duration::from_millis(100)).await;

 let shk=hkdf("Events-Salt","Events-Write-Encryption-Key",&key);
 let stream_setup=dict(vec![("streams",plist::Value::Array(vec![dict(vec![
  ("audioFormat",plist::Value::Integer(0x800u64.into())),("audioMode",plist::Value::String("default".into())),
  ("controlPort",plist::Value::Integer((control.local_addr()?.port() as u64).into())),("ct",plist::Value::Integer(1u64.into())),
  ("isMedia",plist::Value::Boolean(true)),("latencyMax",plist::Value::Integer(88200u64.into())),("latencyMin",plist::Value::Integer(11025u64.into())),
  ("shk",plist::Value::Data(shk.to_vec())),("spf",plist::Value::Integer((FRAMES_PER_PACKET as u64).into())),("sr",plist::Value::Integer(RATE.into())),
  ("type",plist::Value::Integer(0x60u64.into())),("supportsDynamicStreamID",plist::Value::Boolean(false)),("streamConnectionID",plist::Value::Integer((session_id as u64).into()))])]))]);
 let resp=rtsp.request("SETUP",None,Some("application/x-apple-binary-plist"),&bplist(stream_setup)?,&[]).await?;
 let info=plist::Value::from_reader(std::io::Cursor::new(&resp.body))?;
 let s0=info.as_dictionary().and_then(|d|d.get("streams")).and_then(|v|v.as_array()).and_then(|a|a.first()).and_then(|v|v.as_dictionary()).ok_or_else(||anyhow!("no streams in SETUP reply"))?;
 let data_port=s0.get("dataPort").and_then(|v|v.as_unsigned_integer()).ok_or_else(||anyhow!("no dataPort"))? as u16;
 let control_port=s0.get("controlPort").and_then(|v|v.as_unsigned_integer()).unwrap_or(0) as u16;

 let start_ts=ntp2ts(ntp_now());let mut head_ts=start_ts;let latency=target.latency;
 let mut seq:u16=first_seq;
 let rtptime=|head:u64|->u32{(head-start_ts+latency) as u32};
 rtsp.request("RECORD",None,None,&[],&[]).await?;
 rtsp.request("FLUSH",None,None,&[],&[("Range","npt=0-".into()),("Session",session_id.to_string()),("RTP-Info",format!("seq={seq};rtptime={}",rtptime(head_ts)))]).await?;
 rtsp.request("SET_PARAMETER",None,Some("text/parameters"),format!("volume: {:.6}\r\n",target.volume).as_bytes(),&[]).await?;
 eprintln!("{host}: streaming (data {data_port}, control {control_port}, latency {} ms)",latency*1000/RATE);

 let audio_sock=UdpSocket::bind((local_ip,0)).await?;audio_sock.connect((host.ip(),data_port)).await?;
 let audio_cipher=ChaCha20Poly1305::new(Key::from_slice(&shk));let mut audio_n:u64=0;
 let mut backlog:Vec<Option<(u16,Vec<u8>)>>=vec![None;BACKLOG];
 let control_dest=SocketAddr::new(host.ip(),control_port);
 let mut first=true;let mut last_sync=Instant::now()-Duration::from_secs(2);let mut last_feedback=Instant::now();
 let mut current_volume=target.volume;
 let volume_path=format!("/run/alice-airplay/output-volumes/{}.db",host.ip());
 let mut volume_tick=tokio::time::interval(Duration::from_millis(100));
 let mut ctl_buf=[0u8;64];
 loop{
  tokio::select!{
   chunk=audio.recv()=>{
    let chunk=match chunk{Ok(c)=>c,Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break};
    if last_sync.elapsed()>=Duration::from_secs(1)&&control_port!=0{
     let now=ts2ntp(head_ts);let mut p=[0u8;20];p[0]=if first{0x90}else{0x80};p[1]=0xD4;p[2..4].copy_from_slice(&7u16.to_be_bytes());
     p[4..8].copy_from_slice(&(rtptime(head_ts).wrapping_sub(latency as u32)).to_be_bytes());p[8..16].copy_from_slice(&now.to_be_bytes());p[16..20].copy_from_slice(&rtptime(head_ts).to_be_bytes());
     let _=control.send_to(&p,control_dest).await;last_sync=Instant::now();
    }
    let mut header=[0u8;12];header[0]=0x80;header[1]=if first{0xE0}else{0x60};header[2..4].copy_from_slice(&seq.to_be_bytes());
    header[4..8].copy_from_slice(&rtptime(head_ts).to_be_bytes());header[8..12].copy_from_slice(&session_id.to_be_bytes());
    let n=nonce(audio_n);audio_n+=1;
    let sealed=audio_cipher.encrypt(Nonce::from_slice(&n),Payload{msg:&chunk,aad:&header[4..12]}).map_err(|_|anyhow!("audio encrypt"))?;
    let mut packet=header.to_vec();packet.extend(sealed);packet.extend(&n[4..]);
    // a stray ICMP "port unreachable" only fails the next send on a connected UDP socket; keep going
    if let Err(e)=audio_sock.send(&packet).await{if e.kind()!=std::io::ErrorKind::ConnectionRefused{return Err(e).context("audio send")}}
    backlog[seq as usize%BACKLOG]=Some((seq,packet));
    seq=seq.wrapping_add(1);head_ts+=FRAMES_PER_PACKET as u64;first=false;
    if last_feedback.elapsed()>=Duration::from_secs(2){rtsp.request("POST",Some("/feedback"),None,&[],&[]).await.context("feedback")?;last_feedback=Instant::now();}
   }
   _=volume_tick.tick()=>{
    if let Some(db)=std::fs::read_to_string(&volume_path).ok().and_then(|v|v.trim().parse::<f32>().ok()).filter(|v|v.is_finite()&&(*v== -144.0||(-30.0..=0.0).contains(v))){
     if (db-current_volume).abs()>0.01{
      match rtsp.request("SET_PARAMETER",None,Some("text/parameters"),format!("volume: {db:.6}\r\n").as_bytes(),&[]).await{
       Ok(_)=>{current_volume=db;eprintln!("{host}: volume {db:.2} dB");},
       Err(_)=>{eprintln!("{host}: volume update failed");}
      }
     }
    }
   }
   got=control.recv_from(&mut ctl_buf)=>{
    let Ok((n,from))=got else{continue};
    if n>=8&&ctl_buf[1]&0x7F==0x55{
     let lost=u16::from_be_bytes([ctl_buf[4],ctl_buf[5]]);let count=u16::from_be_bytes([ctl_buf[6],ctl_buf[7]]);
     for i in 0..count{let want=lost.wrapping_add(i);if let Some((s,p))=&backlog[want as usize%BACKLOG]{if *s==want{let mut r=vec![0x80,0xD6];r.extend(&p[2..4]);r.extend(p);let _=control.send_to(&r,from).await;}}}
    }
   }
   _=stop.changed()=>break,
  }
 }
 let _=rtsp.request("TEARDOWN",None,None,&[],&[]).await;
 timing_task.abort();if let Some(t)=event_task{t.abort();}
 eprintln!("{host}: stopped");
 Ok(())
}

async fn run_target(target:Target,tx:broadcast::Sender<Arc<Vec<u8>>>,mut stop:watch::Receiver<bool>){
 while !*stop.borrow(){
  let mut audio=tx.subscribe();
  if let Err(e)=stream_to(&target,&mut audio,&mut stop).await{eprintln!("{}: {e:#}",target.addr);}
  if *stop.borrow(){break}
  tokio::select!{_=sleep(Duration::from_secs(3))=>{},_=stop.changed()=>{}}
 }
}

struct Options{targets:Vec<Target>,capture:Option<String>,local:Option<String>}

fn parse_args()->Result<Options>{
 let args:Vec<String>=std::env::args().skip(1).collect();let(mut hosts,mut volume,mut latency_ms)=(Vec::new(),-20.0f32,1500u64);
 let(mut capture,mut local)=(None,None);
 let mut i=0;while i<args.len(){
  let value=args.get(i+1).cloned().ok_or_else(||anyhow!("missing value for {}",args[i]))?;
  match args[i].as_str(){"--to"=>hosts.push(value),"--volume"=>volume=value.parse()?,"--latency-ms"=>latency_ms=value.parse()?,
   "--capture"=>capture=Some(value),"--local"=>local=Some(value),other=>bail!("unknown option {other}")}
  i+=2;
 }
 if hosts.is_empty()&&local.is_none(){bail!("usage: airplay_sender --to HOST[:PORT]... [--volume dB] [--latency-ms N] [--capture ALSA_PCM [--local ALSA_PCM]]  (without --capture: s16le-44100-stereo on stdin)")}
 if local.is_some()&&capture.is_none(){bail!("--local needs --capture")}
 let targets=hosts.into_iter().map(|h|{let addr:SocketAddr=if h.contains(':'){h.parse()?}else{format!("{h}:7000").parse()?};Ok(Target{addr,volume,latency:latency_ms*RATE/1000})}).collect::<Result<_>>()?;
 Ok(Options{targets,capture,local})
}

/// 48 kHz → 44.1 kHz linear resampler for interleaved S16LE stereo.
struct Resampler{pos:f64,prev:[f32;2],out:Vec<u8>}
impl Resampler{
 const STEP:f64=48000.0/44100.0;
 fn push(&mut self,input:&[u8]){
  let frames:Vec<[f32;2]>=input.chunks_exact(4).map(|f|[i16::from_le_bytes([f[0],f[1]]) as f32,i16::from_le_bytes([f[2],f[3]]) as f32]).collect();
  // pos is measured from `prev` (index -1) so interpolation can cross chunk borders
  while self.pos<frames.len() as f64{
   let i=self.pos.floor() as isize;let t=(self.pos-i as f64) as f32;
   let a=if i==0{self.prev}else{frames[(i-1) as usize]};let b=frames[i as usize];
   for c in 0..2{let v=(a[c]+(b[c]-a[c])*t).round().clamp(-32768.0,32767.0) as i16;self.out.extend(v.to_le_bytes());}
   self.pos+=Self::STEP;
  }
  self.pos-=frames.len() as f64;if let Some(last)=frames.last(){self.prev=*last;}
 }
}

/// Starts an ALSA command-line tool with piped stdin/stdout; callers restart it when it exits.
fn spawn_alsa(program:&str,args:&[&str])->std::io::Result<tokio::process::Child>{
 tokio::process::Command::new(program).args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true).spawn()
}

/// Reads 48 kHz from the ALSA loopback, plays it locally and feeds 44.1 kHz packets to the senders.
async fn loop_reader(capture:String,local:Option<String>,tx:broadcast::Sender<Arc<Vec<u8>>>){
 let fmt=["-q","-t","raw","-f","S16_LE","-r","48000","-c","2"];
 let mut player:Option<tokio::process::Child>=None;let mut rs=Resampler{pos:1.0,prev:[0.0;2],out:Vec::new()};
 loop{
  let mut args=vec!["-D",capture.as_str()];args.extend(fmt);
  let mut rec=match spawn_alsa("arecord",&args){Ok(c)=>c,Err(e)=>{eprintln!("arecord: {e}");sleep(Duration::from_millis(500)).await;continue}};
  let mut out=rec.stdout.take().expect("piped stdout");let mut buf=vec![0u8;1920];
  loop{
   let n=match out.read(&mut buf).await{Ok(0)|Err(_)=>break,Ok(n)=>n};
   if let Some(dev)=&local{
    if player.is_none(){let mut a=vec!["-D",dev.as_str()];a.extend(fmt);player=spawn_alsa("aplay",&a).map_err(|e|eprintln!("aplay: {e}")).ok();}
    let failed=match player.as_mut().and_then(|p|p.stdin.as_mut()){Some(stdin)=>stdin.write_all(&buf[..n]).await.is_err(),None=>true};
    if failed{player=None;}
   }
   rs.push(&buf[..n&!3]);
   while rs.out.len()>=PACKET_BYTES{let packet:Vec<u8>=rs.out.drain(..PACKET_BYTES).collect();let _=tx.send(Arc::new(packet));}
  }
  let _=rec.kill().await;eprintln!("capture ended, reopening");sleep(Duration::from_millis(200)).await;
 }
}

#[tokio::main(flavor="current_thread")]
async fn main()->Result<()>{
 let options=parse_args()?;
 let(tx,_)=broadcast::channel::<Arc<Vec<u8>>>(64);let(stop_tx,stop_rx)=watch::channel(false);
 let workers:Vec<_>=options.targets.into_iter().map(|t|tokio::spawn(run_target(t,tx.clone(),stop_rx.clone()))).collect();
 let reader=match options.capture{
  Some(capture)=>tokio::spawn(loop_reader(capture,options.local,tx.clone())),
  None=>{let tx=tx.clone();tokio::spawn(async move{
   let mut stdin=tokio::io::stdin();let mut buf=vec![0u8;PACKET_BYTES];let started=Instant::now();let mut frames:u64=0;
   // paced to realtime so a file on stdin behaves like a live capture
   while stdin.read_exact(&mut buf).await.is_ok(){
    frames+=FRAMES_PER_PACKET as u64;let due=Duration::from_micros(frames*1_000_000/RATE);let elapsed=started.elapsed();
    if due>elapsed+Duration::from_millis(50){sleep(due-elapsed).await;}
    let _=tx.send(Arc::new(buf.clone()));
   }
  })}
 };
 let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
 tokio::select!{_=reader=>{},_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
 let _=stop_tx.send(true);
 for w in workers{let _=timeout(Duration::from_secs(3),w).await;}
 Ok(())
}
