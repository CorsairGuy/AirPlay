use anyhow::Result;
use airplay_mrp::{control::Control,model::{self,Feed,Identity},server,sources};
use tokio::sync::watch;
#[tokio::main(flavor="current_thread")]
async fn main()->Result<()>{
 let args:Vec<String>=std::env::args().collect();
 if args.iter().any(|v|v=="--version"){println!("airplay_mrp 0.1.0 native");return Ok(())}
 if args.iter().any(|v|v=="--list-outputs"){let identity=sources::identity().await?;for device in airplay_mrp::outputs::discover(&identity).await?{println!("{} | {} | {}:{} | aliases={:?}",device.uid,device.name,device.address,device.port,device.aliases);}return Ok(())}
 if args.iter().any(|v|v=="--self-test"){
  let identity=Identity{device:"12345678-1234-1234-1234-123456789abc".into(),group:"87654321-4321-4321-4321-cba987654321".into()};
  let feed=Feed{title:"Native metadata test".into(),content_id:"test-content".into(),control_available:true,..Default::default()};
  let mut messages=model::topology(&identity,&feed);messages.extend(model::announcement(&identity,&feed));messages.push(model::device_info(&identity,&feed,None));
  for message in messages{message.validate("ProtocolMessage")?;}
  let archive=model::decode_plist(&model::queue_archive(600,600))?;assert!(archive.as_dictionary().is_some());
  println!("Native MRP schema, scalar wire types, topology, state and queue archive validated");return Ok(())
 }
 let route_fixture=args.iter().any(|v|v=="--fixture-route");
 let fixture=route_fixture||args.iter().any(|v|v=="--fixture");let identity=if fixture{Identity{device:"12345678-1234-1234-1234-123456789abc".into(),group:"87654321-4321-4321-4321-cba987654321".into()}}else{sources::identity().await?};
 let mut initial=if fixture{Feed{title:"Native metadata fixture".into(),content_id:"native-fixture".into(),..Default::default()}}else{Feed::default()};
 if !fixture{initial.output_control=airplay_mrp::outputs::Outputs::default().enabled();}
 if route_fixture{initial.route=Some(model::Route{group:"AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE".into(),active:true,leader:false,contains_leader:false});}
 let(sender,receiver)=watch::channel(initial);let control=Control::new(sender);if !fixture{control.outputs.start(identity.clone(),receiver.clone());sources::start(control.clone());}
 let port=std::env::var("MINI2_MRP_LOCAL_PORT").ok().and_then(|v|v.parse().ok()).unwrap_or(18902);
 tokio::select!{result=server::run(port,identity,receiver,control)=>result,_=tokio::signal::ctrl_c()=>Ok(())}
}
