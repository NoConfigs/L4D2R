use super::*;
use crate::ivp_joint_apply::tests::{body,snapshot};
use controllers::{Controller,Notice};
#[test]
fn native_controller_callbacks_observe_canonical_body_flags_before_delivery(){
 let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-body-callback-notify.json")).unwrap();let mut bodies=d["initial_bank"].as_array().unwrap().iter().map(body).collect::<Vec<_>>();
 let mut owner=Ownership::new(14,8,8,0.).unwrap();for core in 1..4{owner.bind_objects([10,10+core],[0,core]).unwrap();}
 let mut c=Controller::new(10,0.1,true).unwrap();c.track_cores([(11,1),(12,2),(13,3)]).unwrap();for channel in 0..4{for id in 0..3{c.listen(channel,id).unwrap();}}owner.controllers.insert(c).unwrap();
 let objects=|id:usize|if id%2==0{[10,11+id%3]}else{[11+id%3,10]};let selected=[10,11,12,13];
 let check=|owner:&Ownership,bodies:&Vec<crate::ivp_joint_apply::Body>,row:&serde_json::Value,events:Vec<[usize;9]>|{
  assert_eq!(serde_json::json!(events),row["events"]);let c=owner.controllers.get(10).unwrap();assert_eq!(serde_json::json!((0..8).map(|i|c.contains(i)).collect::<Vec<_>>()),row["members"]);
  assert_eq!(serde_json::json!(bodies.iter().copied().map(snapshot).collect::<Vec<_>>()),row["body_bank"]);for i in 1..4{assert_eq!(owner.controllers.notifications.flags(10+i).unwrap(),bodies[i].object_flags);}
 };
 let observe=|events:&mut Vec<[usize;9]>,state:&Controller,e:Notice,bodies:&[crate::ivp_joint_apply::Body]|{
  let subject=if e.channel==1{e.subject-10}else{e.subject};let peer=if e.channel>=3||e.channel==1{subject}else{1+subject%3};events.push([e.channel,e.listener,subject,usize::from(e.add),state.count(),state.peer_count(10+peer) as usize,state.core_count(peer) as usize,usize::from(state.aliased(peer)),bodies[peer].object_flags as usize]);Ok(())
 };
 for row in d["steps"].as_array().unwrap(){owner.sync_controller_bodies(&selected,&mut bodies).unwrap();let id=row["id"].as_u64().unwrap() as usize;let mut events=vec![];
  if row["op"]==0{owner.change_controller_contact(10,id,objects(id),row["add"].as_bool().unwrap(),&mut bodies,|s,e,b|observe(&mut events,s,e,b)).unwrap();}
  else{owner.notify_controller_cores(&[id],&mut bodies,|_,s,e,b|observe(&mut events,s,e,b)).unwrap();}
  owner.commit_controller_body_flags(&selected,&mut bodies).unwrap();check(&owner,&bodies,row,events);
 }
 for id in 0..8{if owner.controllers.get(10).unwrap().contains(id){owner.change_controller_contact(10,id,objects(id),false,&mut bodies,|_,_,_|Ok(())).unwrap();}}owner.commit_controller_body_flags(&selected,&mut bodies).unwrap();
 for row in d["resolve_cleanup"].as_array().unwrap(){let mut events=vec![];owner.notify_controller_cores(&[row["id"].as_u64().unwrap() as usize],&mut bodies,|_,s,e,b|observe(&mut events,s,e,b)).unwrap();owner.commit_controller_body_flags(&selected,&mut bodies).unwrap();check(&owner,&bodies,row,events);}
 assert_eq!(d["steps"].as_array().unwrap().len(),640);
}
