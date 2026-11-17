//!100fbb50/100fbc80 contact membership and optional object-peer reference counts.
//! Core references and sleeping aliases share native callback phases; world lifetime emission remains separate.
use std::collections::{BTreeMap,BTreeSet};
#[path="ivp_object_notifications.rs"]
pub(crate) mod notifications;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Notice {pub channel:usize,pub listener:usize,pub subject:usize,pub add:bool}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Controller {
 pub object:usize,pub threshold:f32,pub object_peers:bool,
 members:BTreeMap<usize,[usize;2]>,peers:BTreeMap<usize,u32>,listeners:[Vec<usize>;4],cores:Option<BTreeMap<usize,usize>>,core_counts:BTreeMap<usize,u32>,sleeping:BTreeSet<usize>,aliases:BTreeMap<usize,usize>,
}
impl Controller {
 pub(crate) fn new(object:usize,threshold:f32,object_peers:bool)->Result<Self,&'static str>{
  if !threshold.is_finite(){return Err("invalid contact controller threshold");}
  Ok(Self{object,threshold,object_peers,members:BTreeMap::new(),peers:BTreeMap::new(),listeners:std::array::from_fn(|_|vec![]),cores:None,core_counts:BTreeMap::new(),sleeping:BTreeSet::new(),aliases:BTreeMap::new()})
 }
 pub(crate) fn contains(&self,pair:usize)->bool{self.members.contains_key(&pair)}
 pub(crate) fn count(&self)->usize{self.members.len()}
 pub(crate) fn peer_count(&self,object:usize)->u32{self.peers.get(&object).copied().unwrap_or(0)}
 /// Native core-set bookkeeping for provider-bound object/core identities.
 /// Object-lifetime registrations and live core replacement are separate.
 pub(crate) fn track_cores(&mut self,bindings:impl IntoIterator<Item=(usize,usize)>)->Result<(),&'static str>{
  if !self.members.is_empty()||self.cores.is_some(){return Err("core tracking requires an unbound empty controller");}
  let mut cores=BTreeMap::new();for (object,core) in bindings{if cores.insert(object,core).is_some(){return Err("duplicate controller core binding");}}
  self.cores=Some(cores);Ok(())
 }
 /// Object flag byte8 defers core exit through100fb680 alias ownership.
 pub(crate) fn set_sleeping(&mut self,object:usize,sleeping:bool)->Result<(),&'static str>{
  if !self.cores.as_ref().is_some_and(|c|c.contains_key(&object)){return Err("sleeping controller object lacks a core binding");}
  if sleeping{self.sleeping.insert(object);}else{self.sleeping.remove(&object);}Ok(())
 }
 pub(crate) fn aliased(&self,core:usize)->bool{self.aliases.contains_key(&core)}
 ///100fb700 releases the object's alias; only an otherwise unreferenced core
 /// emits the delayed controller core-exit notification.
 pub(crate) fn resolve_alias(&mut self,object:usize,notify:impl FnMut(&Self,Notice)->Result<(),&'static str>)->Result<(),&'static str>{self.resolve_alias_with_lifetime(object,notify,|_|Ok(()))}
 fn resolve_alias_with_lifetime(&mut self,object:usize,mut notify:impl FnMut(&Self,Notice)->Result<(),&'static str>,mut unregister:impl FnMut(usize)->Result<(),&'static str>)->Result<(),&'static str>{
  let core=*self.cores.as_ref().and_then(|c|c.get(&object)).ok_or("alias notification lacks a core binding")?;
  if self.aliases.contains_key(&core){unregister(object)?;self.aliases.remove(&core);if self.core_count(core)==0{self.emit(4,core,false,&mut notify)?;}}Ok(())
 }
 pub(crate) fn core_count(&self,core:usize)->u32{self.core_counts.get(&core).copied().unwrap_or(0)}
 pub(crate) fn listen(&mut self,channel:usize,listener:usize)->Result<(),&'static str>{self.listeners.get_mut(channel).ok_or("invalid contact listener channel")?.push(listener);Ok(())}
 fn emit(&self,channel:usize,subject:usize,add:bool,notify:&mut impl FnMut(&Self,Notice)->Result<(),&'static str>)->Result<(),&'static str>{
  for &listener in self.listeners[if channel==4{2}else{channel}].iter().rev(){notify(self,Notice{channel,listener,subject,add})?;}Ok(())
 }
 pub(crate) fn change(&mut self,pair:usize,objects:[usize;2],add:bool,notify:impl FnMut(&Self,Notice)->Result<(),&'static str>)->Result<(),&'static str>{self.change_with_lifetime(pair,objects,add,notify,|_,_|Ok(()))}
 fn change_with_lifetime(&mut self,pair:usize,objects:[usize;2],add:bool,mut notify:impl FnMut(&Self,Notice)->Result<(),&'static str>,mut lifetime:impl FnMut(usize,bool)->Result<(),&'static str>)->Result<(),&'static str>{
  if objects[0]==objects[1]||!objects.contains(&self.object)||self.contains(pair)==add{return Err("invalid contact controller membership transition");}
  if !add&&self.members.get(&pair)!=Some(&objects){return Err("contact controller object binding differs");}
  let peer=objects[usize::from(objects[0]==self.object)];
  let count=self.peer_count(peer);
  if self.object_peers&&((add&&count==u32::MAX)||(!add&&count==0)){return Err("invalid contact peer reference count");}
  let core=if let Some(cores)=&self.cores{Some(*cores.get(&peer).ok_or("missing controller peer core binding")?)}else{None};
  let core_count=core.map_or(0,|id|self.core_count(id));
  if core.is_some()&&((add&&core_count==u32::MAX)||(!add&&core_count==0)){return Err("invalid controller core reference count");}
  if add{self.members.insert(pair,objects);}else{self.members.remove(&pair);}
  self.emit(0,pair,add,&mut notify)?;
  if self.object_peers{
   if add{self.peers.insert(peer,count+1);if count==0{self.emit(1,peer,true,&mut notify)?;}}
   else if count<2{self.peers.remove(&peer);self.emit(1,peer,false,&mut notify)?;}
   else{self.peers.insert(peer,count-1);}
  }
  if let Some(core)=core{
   if add{self.core_counts.insert(core,core_count+1);if core_count==0{self.emit(3,core,true,&mut notify)?;if self.aliases.contains_key(&core){lifetime(peer,false)?;self.aliases.remove(&core);}else{self.emit(4,core,true,&mut notify)?;}}}
   else if core_count<2{self.core_counts.remove(&core);self.emit(3,core,false,&mut notify)?;if self.sleeping.contains(&peer){lifetime(peer,true)?;self.aliases.insert(core,peer);}else{self.emit(4,core,false,&mut notify)?;}}
   else{self.core_counts.insert(core,core_count-1);}
  }
  self.emit(2,pair,add,&mut notify)
 }
}
#[derive(Clone,Default)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Bank {controllers:BTreeMap<usize,Controller>,pub notifications:notifications::Registry}
impl Bank {
 pub(crate) fn is_empty(&self)->bool{self.controllers.is_empty()}
 pub(crate) fn insert(&mut self,controller:Controller)->Result<(),&'static str>{if self.controllers.contains_key(&controller.object){return Err("contact controller already bound");}self.controllers.insert(controller.object,controller);Ok(())}
 pub(crate) fn validate_body(&self,object:usize,core:usize,flags:u32)->Result<(),&'static str>{
  if self.notifications.flags(object).is_err()&&flags&0x1000!=0{return Err("unowned initial body notification flag");}
  for c in self.controllers.values(){if let Some(expected)=c.cores.as_ref().and_then(|m|m.get(&object)){if *expected!=core{return Err("controller/provider core binding differs");}}}Ok(())
 }
 pub(crate) fn sync_body(&mut self,object:usize,core:usize,flags:u32)->Result<(),&'static str>{
  self.validate_body(object,core,flags)?;self.notifications.bind(object,flags)?;
  for c in self.controllers.values_mut(){if c.cores.as_ref().is_some_and(|m|m.contains_key(&object)){c.set_sleeping(object,flags&0xff==8)?;}}Ok(())
 }
 pub(crate) fn change(&mut self,object:usize,pair:usize,objects:[usize;2],add:bool,mut notify:impl FnMut(&Controller,Notice)->Result<(),&'static str>)->Result<(),&'static str>{self.change_observed(object,pair,objects,add,|c,e,_|notify(c,e))}
 pub(crate) fn change_observed(&mut self,object:usize,pair:usize,objects:[usize;2],add:bool,mut notify:impl FnMut(&Controller,Notice,&notifications::Registry)->Result<(),&'static str>)->Result<(),&'static str>{
  let registry=std::cell::RefCell::new(&mut self.notifications);
  self.controllers.get_mut(&object).ok_or("missing contact controller")?.change_with_lifetime(pair,objects,add,|state,event|notify(state,event,&registry.borrow()),|peer,add|if add{registry.borrow_mut().add(peer,object)}else{registry.borrow_mut().remove(peer,object)})
 }
 pub(crate) fn notify_object(&mut self,object:usize,mut notify:impl FnMut(usize,&Controller,Notice)->Result<(),&'static str>)->Result<usize,&'static str>{self.notify_object_observed(object,|id,c,e,_|notify(id,c,e))}
 pub(crate) fn notify_object_observed(&mut self,object:usize,mut notify:impl FnMut(usize,&Controller,Notice,&notifications::Registry)->Result<(),&'static str>)->Result<usize,&'static str>{
  let controllers=&mut self.controllers;self.notifications.emit(object,|registry,id|{
   let c=controllers.get_mut(&id).ok_or("object listener lacks controller")?;let core=*c.cores.as_ref().and_then(|m|m.get(&object)).ok_or("object notifier lacks controller core binding")?;
   if c.aliased(core){registry.remove(object,id)?;}
   c.resolve_alias(object,|state,event|notify(id,state,event,registry))
  })
 }
 pub(crate) fn get(&self,object:usize)->Option<&Controller>{self.controllers.get(&object)}
 pub(crate) fn get_mut(&mut self,object:usize)->Result<&mut Controller,&'static str>{self.controllers.get_mut(&object).ok_or("missing contact controller")}
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn native_registered_alias_notification_flags_and_self_unregistration_match(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-object-alias-notify.json")).unwrap();let mut c=Controller::new(0,0.1,true).unwrap();c.track_cores([(1,1),(2,2),(3,3)]).unwrap();for object in 1..4{c.set_sleeping(object,true).unwrap();}
  for channel in 0..4{for listener in 0..3{c.listen(channel,listener).unwrap();}}
  let mut bank=Bank::default();bank.insert(c).unwrap();for object in 0..4{bank.notifications.bind(object,2056).unwrap();}
  let objects=|id:usize|if id%2==0{[0,1+id%3]}else{[1+id%3,0]};let mut callbacks=0;
  let check=|c:&Controller,r:&serde_json::Value,events:Vec<[usize;8]>|{
   assert_eq!(serde_json::json!(events),r["events"],"alias op{} id{}",r["op"],r["id"]);assert_eq!(serde_json::json!((0..8).map(|id|c.contains(id)).collect::<Vec<_>>()),r["members"]);
   assert_eq!(serde_json::json!((1..4).map(|o|c.peer_count(o)).collect::<Vec<_>>()),r["peers"]);assert_eq!(serde_json::json!((1..4).map(|o|c.core_count(o)).collect::<Vec<_>>()),r["cores"]);assert_eq!(serde_json::json!((1..4).map(|o|c.aliased(o)).collect::<Vec<_>>()),r["aliases"]);
  };
  for r in d["steps"].as_array().unwrap(){let id=r["id"].as_u64().unwrap() as usize;let mut events=vec![];let mut observe=|state:&Controller,e:Notice|{let peer=if e.channel>=3||e.channel==1{e.subject}else{1+e.subject%3};events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(peer) as usize,state.core_count(peer) as usize,usize::from(state.aliased(peer))]);Ok(())};
   if r["op"]==0{bank.change(0,id,objects(id),r["add"].as_bool().unwrap(),&mut observe).unwrap();}else{bank.notify_object(id,|_,state,event|observe(state,event)).unwrap();}callbacks+=events.len();check(bank.get(0).unwrap(),r,events);assert_eq!(serde_json::json!((1..4).map(|o|bank.notifications.flags(o).unwrap()).collect::<Vec<_>>()),r["registry_flags"]);
  }
  for id in 0..8{if bank.get(0).unwrap().contains(id){bank.change(0,id,objects(id),false,|_,_|Ok(())).unwrap();}}
  for r in d["resolve_cleanup"].as_array().unwrap(){let mut events=vec![];bank.notify_object(r["id"].as_u64().unwrap() as usize,|_,state,e|{events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(e.subject) as usize,state.core_count(e.subject) as usize,usize::from(state.aliased(e.subject))]);Ok(())}).unwrap();check(bank.get(0).unwrap(),r,events);assert_eq!(serde_json::json!((1..4).map(|o|bank.notifications.flags(o).unwrap()).collect::<Vec<_>>()),r["registry_flags"]);}
  assert_eq!(callbacks,4287);assert_eq!(d["steps"].as_array().unwrap().len(),640);let c=bank.get(0).unwrap();assert!(c.aliases.is_empty());assert!(c.core_counts.is_empty());assert_eq!(c.count(),0);
 }
 #[test]
 fn native_sleeping_alias_suppression_and_delayed_core_exit_phases_match(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-alias-controller.json")).unwrap();let mut c=Controller::new(0,0.1,true).unwrap();c.track_cores([(1,1),(2,2),(3,3)]).unwrap();for object in 1..4{c.set_sleeping(object,true).unwrap();}
  for channel in 0..4{for listener in 0..3{c.listen(channel,listener).unwrap();}}
  let objects=|id:usize|if id%2==0{[0,1+id%3]}else{[1+id%3,0]};let mut callbacks=0;
  let check=|c:&Controller,r:&serde_json::Value,events:Vec<[usize;8]>|{
   assert_eq!(serde_json::json!(events),r["events"],"alias op{} id{}",r["op"],r["id"]);assert_eq!(serde_json::json!((0..8).map(|id|c.contains(id)).collect::<Vec<_>>()),r["members"]);
   assert_eq!(serde_json::json!((1..4).map(|o|c.peer_count(o)).collect::<Vec<_>>()),r["peers"]);assert_eq!(serde_json::json!((1..4).map(|o|c.core_count(o)).collect::<Vec<_>>()),r["cores"]);assert_eq!(serde_json::json!((1..4).map(|o|c.aliased(o)).collect::<Vec<_>>()),r["aliases"]);
  };
  for r in d["steps"].as_array().unwrap(){let id=r["id"].as_u64().unwrap() as usize;let mut events=vec![];let mut observe=|state:&Controller,e:Notice|{let peer=if e.channel>=3||e.channel==1{e.subject}else{1+e.subject%3};events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(peer) as usize,state.core_count(peer) as usize,usize::from(state.aliased(peer))]);Ok(())};
   if r["op"]==0{c.change(id,objects(id),r["add"].as_bool().unwrap(),&mut observe).unwrap();}else{c.resolve_alias(id,&mut observe).unwrap();}callbacks+=events.len();check(&c,r,events);
  }
  for id in 0..8{if c.contains(id){c.change(id,objects(id),false,|_,_|Ok(())).unwrap();}}
  for r in d["resolve_cleanup"].as_array().unwrap(){let mut events=vec![];c.resolve_alias(r["id"].as_u64().unwrap() as usize,|state,e|{events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(e.subject) as usize,state.core_count(e.subject) as usize,usize::from(state.aliased(e.subject))]);Ok(())}).unwrap();check(&c,r,events);}
  assert_eq!(callbacks,4287);assert_eq!(d["steps"].as_array().unwrap().len(),640);assert!(c.aliases.is_empty());assert!(c.core_counts.is_empty());assert_eq!(c.count(),0);
 }
 #[test]
 fn native_shared_core_reference_counts_and_callback_phases_match(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-core-controller.json")).unwrap();let mut c=Controller::new(0,0.1,true).unwrap();c.track_cores([(1,1),(2,1),(3,3)]).unwrap();
  for channel in 0..4{for listener in 0..3{c.listen(channel,listener).unwrap();}}
  let objects=|id:usize|if id%2==0{[0,1+id%3]}else{[1+id%3,0]};let mut events_count=0;
  for (step,r) in d["steps"].as_array().unwrap().iter().enumerate(){let id=r["pair"].as_u64().unwrap() as usize;let add=r["add"].as_bool().unwrap();let mut events=vec![];
   c.change(id,objects(id),add,|state,e|{let peer=if e.channel>=3||e.channel==1{e.subject}else{1+e.subject%3};let core=if peer==2{1}else{peer};events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(peer) as usize,state.core_count(core) as usize]);Ok(())}).unwrap();
   assert_eq!(serde_json::json!(events),r["events"],"step{step} callback phases");events_count+=events.len();
   assert_eq!(serde_json::json!((0..8).map(|i|c.contains(i)).collect::<Vec<_>>()),r["members"]);
   for peer in 1..4{let n=&r["peers"][peer-1];assert_eq!(c.peer_count(peer) as u64,n["count"].as_u64().unwrap());let core=if peer==2{1}else{peer};let n=&r["cores"][peer-1];assert_eq!(c.core_count(core) as u64,n["count"].as_u64().unwrap());assert_eq!(c.core_count(core)>0,n["member"].as_bool().unwrap());}
  }
  for id in 0..8{if c.contains(id){c.change(id,objects(id),false,|_,_|Ok(())).unwrap();}}
  assert_eq!(c.count(),0);assert!(c.peers.is_empty());assert!(c.core_counts.is_empty());assert_eq!(d["cleanup"],serde_json::json!({"contacts":0,"peers":0,"cores":0,"aliases":0}));assert_eq!(events_count,d["callbacks"].as_u64().unwrap() as usize);assert_eq!(d["steps"].as_array().unwrap().len(),512);
 }
 #[test]
 fn native_contact_membership_peer_reference_counts_and_callback_phases_match(){
  let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-contact-controller.json")).unwrap();let mut c=Controller::new(0,0.1,true).unwrap();
  for channel in 0..3{for listener in 0..3{c.listen(channel,listener).unwrap();}}
  let objects=|id:usize|if id%2==0{[0,1+id%3]}else{[1+id%3,0]};let mut events_count=0;
  for (step,r) in d["steps"].as_array().unwrap().iter().enumerate(){let id=r["pair"].as_u64().unwrap() as usize;let add=r["add"].as_bool().unwrap();let mut events=vec![];
   c.change(id,objects(id),add,|state,e|{let peer=if e.channel==1{e.subject}else{1+e.subject%3};events.push([e.channel,e.listener,e.subject,usize::from(e.add),state.count(),state.peer_count(peer) as usize]);Ok(())}).unwrap();
   assert_eq!(serde_json::json!(events),r["events"],"step{step} callback phases");events_count+=events.len();
   assert_eq!(serde_json::json!((0..8).map(|i|c.contains(i)).collect::<Vec<_>>()),r["members"]);
   for peer in 1..4{let n=&r["peers"][peer-1];assert_eq!(c.peer_count(peer) as u64,n["count"].as_u64().unwrap());assert_eq!(c.peer_count(peer)>0,n["member"].as_bool().unwrap());}
  }
  for id in 0..8{if c.contains(id){c.change(id,objects(id),false,|_,_|Ok(())).unwrap();}}
  assert_eq!(c.count(),0);assert!(c.peers.is_empty());assert_eq!(d["cleanup"],serde_json::json!({"contacts":0,"peers":0}));assert_eq!(events_count,3621);assert_eq!(d["steps"].as_array().unwrap().len(),512);
 }
}
