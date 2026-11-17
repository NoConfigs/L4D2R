//! Global/object/queue ownership plus1010a910/1010a8e0 paired hull entries.
//! Geometry, PSI dispatch and recursive class callbacks remain separate.
use crate::{ivp_mindist_graph::Graph,ivp_event_queue::Queue,ivp_hull_manager::Manager};
use std::collections::BTreeMap;
#[path="ivp_contact_controller.rs"]
pub(crate) mod controllers;
#[derive(Clone,Copy,Debug)]
pub(crate) struct Far {pub objects:[usize;2],pub handles:[u16;2],pub linear_delta:f32}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Ownership {
 pub controllers:controllers::Bank,pub graph:Graph,pub queue:Queue,pub hulls:Vec<Manager>,pub counter:u32,
 far:BTreeMap<usize,Far>,deadlines:Vec<i32>,prediction_serial:u64,queue_bound:bool,
 revision:u32,core_revisions:BTreeMap<usize,u32>,object_cores:BTreeMap<usize,usize>,core_object_counts:BTreeMap<usize,usize>,
}
pub(crate) struct Prediction {pub object:usize,pub linear:f32,pub angular:f32}
pub(crate) struct Prepared {serial:u64,now:f64,objects:Vec<(usize,bool)>}
#[derive(Clone,Copy,Debug)]
pub(crate) enum TickEvent {
 Recheck{object:usize,notice:crate::ivp_hull_manager::Notice},
 Rebase{object:usize,notice:crate::ivp_hull_manager::Notice},
}
#[cfg(test)]
#[path="ivp_contact_tick_tests.rs"]
mod tick_tests;
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn registered_retirement_removes_controller_references_before_contact_id_reuse(){
  let mut o=Ownership::new(3,8,8,0.).unwrap();let a=o.graph.register([0,1]).unwrap();let b=o.graph.register([0,2]).unwrap();
  let mut c=controllers::Controller::new(0,0.1,true).unwrap();c.track_cores([(1,10),(2,10)]).unwrap();c.listen(2,7).unwrap();c.change(a,[0,1],true,|_,_|Ok(())).unwrap();c.change(b,[0,2],true,|_,_|Ok(())).unwrap();o.controllers.insert(c).unwrap();
  assert!(o.retire(a).is_err());assert!(o.graph.objects(a).is_ok());let mut seen=vec![];
  o.retire_with_controllers(a,|object,c,e|{assert_eq!(object,0);assert!(!c.contains(a));assert!(c.contains(b));seen.push(e);Ok(())}).unwrap();assert_eq!(seen.len(),1);assert_eq!(seen[0].subject,a);assert!(!seen[0].add);
  let id=o.graph.register([0,1]).unwrap();assert_eq!(id,a);let c=o.controllers.get(0).unwrap();assert!(!c.contains(id));assert_eq!(c.peer_count(1),0);assert_eq!(c.peer_count(2),1);assert_eq!(c.core_count(10),1);o.retire(id).unwrap();o.retire_with_controllers(b,|_,_,_|Ok(())).unwrap();assert_eq!(o.controllers.get(0).unwrap().count(),0);
 }
 #[test]
 fn global_psi_walk_preserves_saved_next_and_defers_new_head_records(){
  let mut o=Ownership::new(3,8,8,0.).unwrap();let a=o.graph.register([0,1]).unwrap();let b=o.graph.register([1,2]).unwrap();let c=o.graph.register([0,2]).unwrap();for id in [a,b,c]{o.graph.activate(id,[true,false]).unwrap();}
  let mut inserted=None;let mut seen=vec![];
  assert_eq!(o.refresh_active(|o,id|{seen.push(id);o.graph.make_inactive(id)?;if id==c{let new=o.graph.register([0,1])?;o.graph.activate(new,[false,false])?;inserted=Some(new);}Ok(())}).unwrap(),3);
  assert_eq!(seen,vec![c,b,a]);assert_eq!(o.graph.active(),&[inserted.unwrap()]);assert_eq!(o.graph.inactive(),&[a,b,c]);
  for id in [a,b,c,inserted.unwrap()]{o.retire(id).unwrap();}assert!(o.graph.active().is_empty());assert!(o.graph.inactive().is_empty());assert!(o.graph.fast().is_empty());
 }
 #[test]
 fn changed_core_stamps_all_shared_objects_before_reverse_object_traversal(){
  let mut o=Ownership::new(4,8,8,0.).unwrap();let mut pairs=vec![];
  for object in 0..3{o.bind_objects([object,3],[10,20]).unwrap();let id=o.graph.register([object,3]).unwrap();o.graph.activate(id,[false,false]).unwrap();pairs.push(id);}
  assert_eq!(o.begin_collision_revision(),1);let mut seen=vec![];
  assert_eq!(o.notify_core_changed(10,&[0,1,2],|o,id,side|{assert_eq!(o.core_revision(10)?,1);assert_eq!(o.core_revision(20)?,0);seen.push((id,side));Ok(())}).unwrap(),3);
  assert_eq!(seen,vec![(pairs[2],0),(pairs[1],0),(pairs[0],0)]);seen.clear();
  assert_eq!(o.notify_core_changed(20,&[3],|o,id,side|{assert_eq!(o.core_revision(10)?,o.core_revision(20)?);seen.push((id,side));Ok(())}).unwrap(),3);
  assert_eq!(seen,vec![(pairs[2],1),(pairs[1],1),(pairs[0],1)]);
  assert!(o.notify_core_changed(10,&[0,0],|_,_,_|panic!("invalid vector ran")).is_err());
  for id in pairs{o.retire(id).unwrap();}assert!(o.graph.active().is_empty());
 }
 #[test]
 fn object_notification_keeps_saved_order_while_current_links_are_reactivated(){
  let mut o=Ownership::new(3,8,8,0.).unwrap();let a=o.graph.register([0,1]).unwrap();let b=o.graph.register([2,0]).unwrap();let c=o.graph.register([0,2]).unwrap();let peer=o.graph.register([1,2]).unwrap();
  for id in [a,b,c,peer]{o.graph.link_inactive(id).unwrap();}
  let mut visited=vec![];assert_eq!(o.notify_object(0,|o,id,side|{visited.push((id,side));o.graph.reactivate(id,[false,false])}).unwrap(),3);
  assert_eq!(visited,vec![(c,0),(b,1),(a,0)]);assert!(o.graph.inactive_object_links(0).unwrap().is_empty());assert_eq!(o.graph.inactive(),&[peer]);assert_eq!(o.graph.active(),&[a,b,c]);
  for id in [a,b,c,peer]{o.retire(id).unwrap();}assert!(o.graph.active().is_empty());assert!(o.graph.inactive().is_empty());
 }
 fn far(o:&mut Ownership,objects:[usize;2],keys:[f32;2])->usize{
  let id=o.graph.register(objects).unwrap();o.graph.activate(id,[true,true]).unwrap();o.graph.unlink(id).unwrap();o.graph.set_flags(id,0x0fd40000).unwrap();o.track_far(id,keys,0.).unwrap();id
 }
 #[test]
 fn shared_object_listeners_survive_retirement_and_contact_id_reuse(){
  let mut o=Ownership::new(3,2,2,0.).unwrap();let a=far(&mut o,[0,1],[1.,2.]);let b=far(&mut o,[1,2],[3.,4.]);let retained=*o.far(b).unwrap();
  assert!(o.track_far(a,[1.,2.],0.).is_err());o.retire(a).unwrap();assert_eq!(o.hulls[1].entry(retained.handles[0]).unwrap(),(b*2,3.));
  let c=far(&mut o,[0,1],[5.,6.]);assert_eq!(c,a);assert_eq!(o.hulls[1].entry(retained.handles[0]).unwrap(),(b*2,3.));o.retire(c).unwrap();o.retire(b).unwrap();
  assert!(o.hulls.iter().all(|h|h.first().is_none()));assert!(o.far.is_empty());
 }
 #[test]
 fn foreign_listener_is_rejected_before_either_side_is_removed(){
  let mut o=Ownership::new(2,2,2,0.).unwrap();let a=far(&mut o,[0,1],[1.,2.]);let record=*o.far(a).unwrap();
  o.hulls[1].remove(record.handles[1]).unwrap();assert_eq!(o.hulls[1].insert(999,9.).unwrap(),record.handles[1]);
  assert_eq!(o.release_far(a).unwrap_err(),"contact hull listener identity differs");assert_eq!(o.hulls[0].entry(record.handles[0]),Some((a*2,1.)));assert!(o.far(a).is_some());
 }
}
impl Ownership {
 pub(crate) fn try_clone_unbound(&self)->Result<Self,&'static str>{if self.queue_bound{return Err("cannot stage borrowed ownership queue");}Ok(self.clone())}
 pub(crate) fn new(objects:usize,hull_capacity:usize,queue_capacity:usize,epoch:f64)->Result<Self,&'static str>{
  let hulls=(0..objects).map(|_|Manager::new(hull_capacity)).collect::<Result<Vec<_>,_>>()?;
  Ok(Self{controllers:Default::default(),graph:Graph::new(objects),queue:Queue::new(queue_capacity,epoch)?,hulls,counter:0,far:BTreeMap::new(),deadlines:vec![0;objects],prediction_serial:0,queue_bound:false,revision:0,core_revisions:BTreeMap::new(),object_cores:BTreeMap::new(),core_object_counts:BTreeMap::new()})
 }
 pub(crate) fn bind_objects(&mut self,objects:[usize;2],cores:[usize;2])->Result<(),&'static str>{
  if objects[0]==objects[1]{return Err("notification bindings require distinct objects");}
  for j in 0..2{if objects[j]>=self.hulls.len()||self.object_cores.get(&objects[j]).is_some_and(|&c|c!=cores[j]){return Err("object/core notification binding differs");}}
  for j in 0..2{if self.object_cores.insert(objects[j],cores[j]).is_none(){*self.core_object_counts.entry(cores[j]).or_default()+=1;}self.core_revisions.entry(cores[j]).or_insert(0);}Ok(())
 }
 pub(crate) fn begin_collision_revision(&mut self)->u32{self.revision=self.revision.wrapping_add(1);self.revision}
 pub(crate) fn core_revision(&self,core:usize)->Result<u32,&'static str>{self.core_revisions.get(&core).copied().ok_or("unbound collision revision core")}
 ///100f1600 stamps a changed core before traversing its object vector backwards.
 pub(crate) fn notify_core_changed(&mut self,core:usize,objects:&[usize],mut refresh:impl FnMut(&mut Self,usize,usize)->Result<(),&'static str>)->Result<usize,&'static str>{
  let mut seen=std::collections::BTreeSet::new();
  if objects.iter().any(|o|!seen.insert(*o)||self.object_cores.get(o)!=Some(&core)){return Err("changed-core object vector differs from bindings");}
  *self.core_revisions.get_mut(&core).ok_or("unbound changed core")?=self.revision;let mut visited=0;
  for &object in objects.iter().rev(){visited+=self.notify_active_object(object,|o,id,side|refresh(o,id,side))?;}Ok(visited)
 }
 /// Bind the authoritative event-world queue for a synchronous owner callback.
 /// No queue is cloned/merged. Initialization and every event/PSI callback must
 /// use the same queue; the local diagnostic queue is restored even on Err.
 pub(crate) fn with_queue<T>(&mut self,queue:&mut Queue,run:impl FnOnce(&mut Self)->Result<T,&'static str>)->Result<T,&'static str>{
  if self.queue_bound{return Err("contact event queue is already borrowed");}
  self.queue_bound=true;std::mem::swap(&mut self.queue,queue);
  let result=run(self);
  std::mem::swap(&mut self.queue,queue);self.queue_bound=false;result
 }
 /// Native prediction completes every object's budget before reversing the
 /// queued dispatch vector. Prepared batches cannot be replayed or superseded.
 pub(crate) fn prepare_predictions(&mut self,now:f64,dt:f32,inputs:&[Prediction])->Result<Prepared,&'static str>{
  let mut ids=std::collections::BTreeSet::new();
  if !now.is_finite()||!dt.is_finite()||dt<0.||inputs.iter().any(|p|p.object>=self.hulls.len()||!ids.insert(p.object)||![p.linear,p.angular].into_iter().all(|v|v.is_finite()&&v>=0.)){return Err("invalid hull prediction batch");}
  let objects=inputs.iter().map(|p|(p.object,self.hulls[p.object].budget.advance(now,dt,p.linear,p.angular))).collect();
  self.prediction_serial=self.prediction_serial.wrapping_add(1);Ok(Prepared{serial:self.prediction_serial,now,objects})
 }
 /// World callbacks can renew/remove paired entries while no manager borrow
 /// is held. Quota/rebase primitives are shared with Manager's native controls.
 pub(crate) fn dispatch_predictions(
  &mut self,prepared:Prepared,initial:i32,
  mut callback:impl FnMut(&mut Self,TickEvent)->Result<(),&'static str>,
  mut refill:impl FnMut(usize,i32)->i32,
 )->Result<(),&'static str>{
  if prepared.serial!=self.prediction_serial{return Err("superseded hull prediction batch");}
  self.prediction_serial=self.prediction_serial.wrapping_add(1);
  for (object,due) in prepared.objects.into_iter().rev(){
   if !due{continue;}
   let mut quota=crate::ivp_hull_manager::Quota::new(initial);
   while let Some(notice)=self.hulls[object].due()?{
    callback(self,TickEvent::Recheck{object,notice})?;
    if !quota.after(|n|refill(object,n)){return Err("hull dispatch exhausted quota");}
   }
   if (self.deadlines[object] as f64)<self.hulls[object].budget.time{
    let mut cursor=self.hulls[object].begin_rebase();let mut failure=None;
    while let Some(notice)=cursor.next(&mut self.hulls[object]){
     if let Some((id,_))=self.listener_pair(object,notice){
      let far=self.far.get_mut(&id).unwrap();far.linear_delta=(notice.a-notice.b)+far.linear_delta;
     }
     if let Err(e)=callback(self,TickEvent::Rebase{object,notice}){if failure.is_none(){failure=Some(e);}}
    }
    cursor.finish(&mut self.hulls[object]);
    self.deadlines[object]=(prepared.now+10.).trunc() as i32;
    if let Some(e)=failure{return Err(e);}
   }
  }Ok(())
 }
 pub(crate) fn listener_pair(&self,object:usize,notice:crate::ivp_hull_manager::Notice)->Option<(usize,usize)>{
  let id=notice.owner/2;let side=notice.owner%2;let record=self.far.get(&id)?;
  (record.objects[side]==object&&record.handles[side]==notice.handle).then_some((id,side))
 }
 pub(crate) fn far(&self,id:usize)->Option<&Far>{self.far.get(&id)}
 ///100e8cd0 saves the next object-side inactive link before each query can
 /// reactivate the current contact. Ordinary callbacks do not reorder peers.
 pub(crate) fn notify_object(&mut self,object:usize,mut refresh:impl FnMut(&mut Self,usize,usize)->Result<(),&'static str>)->Result<usize,&'static str>{
  let links=self.graph.inactive_object_links(object)?.to_vec();
  for link in &links{
   if !self.graph.inactive_object_links(object)?.contains(link){return Err("inactive notification lost saved object link");}
   refresh(self,link.pair,link.side)?;
  }Ok(links.len())
 }
 pub(crate) fn notify_active_object(&mut self,object:usize,mut refresh:impl FnMut(&mut Self,usize,usize)->Result<(),&'static str>)->Result<usize,&'static str>{
  let links=self.graph.object_links(object)?.to_vec();
  for link in &links{
   if !self.graph.object_links(object)?.contains(link){return Err("active notification lost saved object link");}
   refresh(self,link.pair,link.side)?;
  }Ok(links.len())
 }
 ///1010b6c0 saves the next global link before a callback can deactivate the
 /// current pair or insert newly discovered contacts at the head.
 pub(crate) fn refresh_active(&mut self,mut refresh:impl FnMut(&mut Self,usize)->Result<(),&'static str>)->Result<usize,&'static str>{
  let ids=self.graph.active().to_vec();
  for &id in &ids{if !self.graph.active().contains(&id){return Err("PSI refresh lost saved active link");}refresh(self,id)?;}
  Ok(ids.len())
 }
 ///1010c8a0 renews the selected side first, retaining the existing listener
 /// identities. A reactivation removes both physical-side entries before
 /// handing ownership to the appropriate constructor callback.
 pub(crate) fn recheck(&mut self,id:usize,input:&crate::ivp_far_distance::Input,config:crate::ivp_far_distance::Config)->Result<crate::ivp_far_distance::Action,&'static str>{
  let record=*self.far.get(&id).ok_or("far recheck requires owned hull entries")?;
  if self.graph.flags(id)?!=input.flags||record.linear_delta.to_bits()!=input.geometry.linear_delta.to_bits(){return Err("far recheck ownership/input differs");}
  self.validate_far(id,record)?;
  let action=crate::ivp_far_distance::update(input,config)?;
  match action{
   crate::ivp_far_distance::Action::Rearm{geometry,keys,first}=>{
    let mut handles=record.handles;
    for side in [first,1-first]{handles[side]=self.hulls[record.objects[side]].replace(handles[side],keys[side])?;}
    self.far.insert(id,Far{handles,linear_delta:geometry.linear_delta,..record});
   },
   crate::ivp_far_distance::Action::Activate{..}=>{self.release_far(id)?;},
  }Ok(action)
 }
 fn token(id:usize,side:usize)->Result<usize,&'static str>{id.checked_mul(2).and_then(|v|v.checked_add(side)).ok_or("hull listener identity overflow")}
 /// The schedule owner has already removed active links and committed far flags.
 pub(crate) fn track_far(&mut self,id:usize,keys:[f32;2],linear_delta:f32)->Result<(),&'static str>{
  if self.far.contains_key(&id){return Err("contact already owns hull entries");}
  let objects=self.graph.objects(id)?;let phase=(self.graph.flags(id)?>>18)&15;
  if !matches!(phase,4|5)||self.graph.active().contains(&id)||self.queue.scheduled(id).is_some(){return Err("hull admission requires detached far contact");}
  if !keys.into_iter().all(f32::is_finite)||!linear_delta.is_finite(){return Err("nonfinite paired hull admission");}
  let tokens=[Self::token(id,0)?,Self::token(id,1)?];
  let first=self.hulls[objects[0]].insert(tokens[0],keys[0])?;
  let second=match self.hulls[objects[1]].insert(tokens[1],keys[1]){Ok(h)=>h,Err(e)=>{self.hulls[objects[0]].remove(first)?;return Err(e);}};
  self.far.insert(id,Far{objects,handles:[first,second],linear_delta});Ok(())
 }
 /// Validate both listener identities before mutating either object's list.
 pub(crate) fn release_far(&mut self,id:usize)->Result<Far,&'static str>{
  let record=*self.far.get(&id).ok_or("contact owns no hull entries")?;
  self.validate_far(id,record)?;
  for side in 0..2{self.hulls[record.objects[side]].remove(record.handles[side])?;}
  self.far.remove(&id);Ok(record)
 }
 fn validate_far(&self,id:usize,record:Far)->Result<(),&'static str>{
  for side in 0..2{
   let entry=self.hulls[record.objects[side]].entry(record.handles[side]).ok_or("missing contact hull entry")?;
   if entry.0!=Self::token(id,side)?{return Err("contact hull listener identity differs");}
  }
  Ok(())
 }
 /// Resource removal precedes graph ID reuse; neighbouring listeners survive.
 /// One object per core is required until the physical bank has separate
 /// compound-object flags. Object IDs need not equal body/core IDs.
 fn controller_body_bindings(&self,objects:&[usize],bodies:&[crate::ivp_joint_apply::Body])->Result<Vec<(usize,usize)>,&'static str>{
  let mut seen=std::collections::BTreeSet::new();let mut result=vec![];
  if bodies.iter().map(|b|b.id).collect::<std::collections::BTreeSet<_>>().len()!=bodies.len(){return Err("duplicate controller physical core");}
  for &object in objects{
   if !seen.insert(object){return Err("duplicate controller physical object");}
   let core=*self.object_cores.get(&object).ok_or("unbound controller physical object")?;
   if self.core_object_counts.get(&core)!=Some(&1){return Err("compound controller objects need separate physical flag storage");}
   let index=bodies.iter().position(|b|b.id==core).ok_or("missing controller physical core")?;result.push((object,index));
  }Ok(result)
 }
 pub(crate) fn sync_controller_bodies(&mut self,objects:&[usize],bodies:&mut[crate::ivp_joint_apply::Body])->Result<(),&'static str>{
  let bindings=self.controller_body_bindings(objects,bodies)?;
  for &(object,index) in &bindings{self.controllers.validate_body(object,bodies[index].id,bodies[index].object_flags)?;}
  for &(object,index) in &bindings{self.controllers.sync_body(object,bodies[index].id,bodies[index].object_flags)?;}
  self.commit_controller_body_flags(objects,bodies)
 }
 pub(crate) fn commit_controller_body_flags(&self,objects:&[usize],bodies:&mut[crate::ivp_joint_apply::Body])->Result<(),&'static str>{
  let bindings=self.controller_body_bindings(objects,bodies)?;let flags=bindings.iter().map(|&(o,_)|self.controllers.notifications.flags(o)).collect::<Result<Vec<_>,_>>()?;
  for ((_,index),flags) in bindings.into_iter().zip(flags){bodies[index].object_flags=flags;}Ok(())
 }
 fn publish_controller_flags(bindings:&[(usize,usize)],registry:&controllers::notifications::Registry,bodies:&mut[crate::ivp_joint_apply::Body])->Result<(),&'static str>{
  for &(object,index) in bindings{bodies[index].object_flags=registry.flags(object)?;}Ok(())
 }
 pub(crate) fn change_controller_contact(&mut self,controller:usize,pair:usize,objects:[usize;2],add:bool,bodies:&mut[crate::ivp_joint_apply::Body],mut notify:impl FnMut(&controllers::Controller,controllers::Notice,&[crate::ivp_joint_apply::Body])->Result<(),&'static str>)->Result<(),&'static str>{
  let selected=self.object_cores.keys().copied().collect::<Vec<_>>();self.sync_controller_bodies(&selected,bodies)?;let bindings=self.controller_body_bindings(&selected,bodies)?;
  let result=self.controllers.change_observed(controller,pair,objects,add,|state,event,registry|{Self::publish_controller_flags(&bindings,registry,bodies)?;notify(state,event,bodies)});
  self.commit_controller_body_flags(&selected,bodies)?;result
 }
 /// Consume native-order core activation notices. Publish object flag changes
 /// before each observer sees the delayed core-exit callback,not only at return.
 pub(crate) fn notify_controller_cores(&mut self,cores:&[usize],bodies:&mut[crate::ivp_joint_apply::Body],mut notify:impl FnMut(usize,&controllers::Controller,controllers::Notice,&[crate::ivp_joint_apply::Body])->Result<(),&'static str>)->Result<usize,&'static str>{
  let selected=self.object_cores.keys().copied().collect::<Vec<_>>();self.sync_controller_bodies(&selected,bodies)?;let bindings=self.controller_body_bindings(&selected,bodies)?;
  let by_core=self.object_cores.iter().map(|(&o,&c)|(c,o)).collect::<BTreeMap<_,_>>();let objects=cores.iter().filter_map(|c|by_core.get(c).copied()).collect::<Vec<_>>();let mut count=0;
  for object in objects{
   let result=self.controllers.notify_object_observed(object,|id,state,event,registry|{Self::publish_controller_flags(&bindings,registry,bodies)?;notify(id,state,event,bodies)});
   self.commit_controller_body_flags(&selected,bodies)?;count+=result?;
  }Ok(count)
 }
 pub(crate) fn retire_with_controllers(&mut self,id:usize,mut notify:impl FnMut(usize,&controllers::Controller,controllers::Notice)->Result<(),&'static str>)->Result<(),&'static str>{
  let objects=self.graph.objects(id)?;
  for object in objects{if let Some(c)=self.controllers.get(object){if c.contains(id){self.controllers.change(object,id,objects,false,|state,event|notify(object,state,event))?;}}}
  self.retire(id)
 }
 pub(crate) fn retire(&mut self,id:usize)->Result<(),&'static str>{
  let objects=self.graph.objects(id)?;
  if objects.into_iter().any(|o|self.controllers.get(o).is_some_and(|c|c.contains(id))){return Err("contact retirement requires controller notifications");}
  if self.far.contains_key(&id){self.release_far(id)?;}
  if self.queue.scheduled(id).is_some(){self.queue.cancel(id)?;}
  self.graph.remove(id)
 }
}

#[cfg(test)]
#[path="ivp_controller_body_tests.rs"]
mod controller_body_tests;

#[cfg(test)]
#[path="ivp_controller_callback_body_tests.rs"]
mod controller_callback_body_tests;
