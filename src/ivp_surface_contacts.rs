//!1010ce30 surface selection bound to persistent compact Contact instances.
//! Lifecycle hooks receive real constructor/teardown context from the owner.
use super::*;
use crate::ivp_minimizer::owner::Contact;
use l4d2_formats::phy::{PhySolid,Convex};
use std::sync::{Arc,atomic::{AtomicI32,Ordering}};
#[path="ivp_recursive_contacts.rs"]
pub(crate) mod recursion;
#[path="ivp_contact_ownership.rs"]
pub(crate) mod ownership;
#[path="ivp_pending_contacts.rs"]
pub(crate) mod pending;
#[path="ivp_compact_discovery.rs"]
pub(crate) mod discovery;
pub(crate) type Counter=Arc<AtomicI32>;
pub(crate) struct Entry<'a>{pub key:Pair,pub contact:Contact,pub recursive:Option<Box<Recursive<'a>>>}
pub(crate) struct Recursive<'a>{pub split:Option<usize>,pub count:Counter,pub descendants:Contacts<'a>}
pub(crate) struct Contacts<'a>{objects:[usize;2],solids:[&'a PhySolid;2],entries:Vec<Entry<'a>>,listeners:Vec<Counter>}
#[derive(Clone,Copy)]
pub(crate) struct Input{pub bodies:[Body;2],pub at:f64,pub margin:f32,pub provided:[Option<usize>;2],pub restricted:[Option<usize>;2]}
pub(crate) struct Changes{pub lists:[Vec<usize>;2],pub queries:[Option<Query>;2],pub created:Vec<Pair>,pub removed:Vec<Pair>}
impl<'a> Contacts<'a>{
 pub(crate) fn try_clone_ordinary(&self)->Result<Self,&'static str>{
  if !self.listeners.is_empty()||self.entries.iter().any(|e|e.recursive.is_some()){return Err("recursive contact snapshots require independent counter ownership");}
  Ok(Self{objects:self.objects,solids:self.solids,entries:self.entries.iter().map(|e|Entry{key:e.key,contact:e.contact.clone(),recursive:None}).collect(),listeners:vec![]})
 }
 pub(crate) fn new(objects:[usize;2],solids:[&'a PhySolid;2])->Result<Self,&'static str>{
    if objects[0]==objects[1]{return Err("surface contacts require distinct objects");}
    Ok(Self{objects,solids,entries:Vec::new(),listeners:Vec::new()})
 }
 fn notify(&self,delta:i32){for listener in &self.listeners{listener.fetch_add(delta,Ordering::Relaxed);}}
 pub(crate) fn clear(&mut self,mut retire:impl FnMut(&Pair,&mut Entry<'a>)->Result<(),&'static str>)->Result<(),&'static str>{
    for entry in self.entries.iter_mut().rev(){let key=entry.key;retire(&key,&mut *entry)?;}
    self.notify(-(self.entries.len() as i32));self.entries.clear();Ok(())
 }
 pub(crate) fn entries(&self)->&[Entry<'a>]{&self.entries}
 pub(crate) fn entries_mut(&mut self)->&mut[Entry<'a>]{&mut self.entries}
 pub(crate) fn update(&mut self,input:Input,mut initialize:impl FnMut(&Pair,&mut Entry<'a>,[&Convex;2])->Result<(),&'static str>,mut retire:impl FnMut(&Pair,&mut Entry<'a>)->Result<(),&'static str>)->Result<Changes,&'static str>{
    let old_count=self.entries.len();
    let trees=self.solids.map(|s|s.compact_tree.as_deref().ok_or("missing compact surface tree"));let trees=[trees[0]?,trees[1]?];
    let (lists,queries)=select(trees,input.bodies,input.at,input.margin,input.provided,input.restricted)?;
    let mut keys=self.entries.iter().map(|e|e.key).collect();let (created,removed)=reconcile(self.objects,&lists,&mut keys)?;
    //1010cc80 uses object margins, not the admission search padding.
    let margin=input.bodies[0].margin+input.bodies[1].margin;
    if !margin.is_finite(){return Err("nonfinite compact contact margin");}
    let mut resolved=Vec::new();
    for key in &created{
      let convexes=std::array::from_fn::<_,2,_>(|j|self.solids[j].compact_ledge(key.ledges[j]).ok_or("unresolved admitted compact ledge"));let convexes=[convexes[0]?,convexes[1]?];
      let mut recursive=false;
      for j in 0..2{
        let header=trees[j].ledge_headers.get(&key.ledges[j]).ok_or("missing admitted ledge header")?;
        recursive|=header[8]&3!=0;
        if convexes[j].topology.is_none(){return Err("admitted ledge has no compact topology");}
      }resolved.push((convexes,recursive));
    }
    let mut staged=Vec::new();
    for (key,(convexes,recursive)) in created.iter().zip(resolved){
      let contact=Contact::new(self.objects,crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS,margin)?;
      //10121420 initializes an empty descendant vector and split=-1 before
      // the ordinary base constructor. Expansion/recursive callback timing is
      // a subsequent owner phase; a coarse parent is never a flat leaf.
      let recursive=if recursive{let count=Arc::new(AtomicI32::new(0));let mut descendants=Contacts::new(self.objects,self.solids)?;descendants.listeners=self.listeners.clone();descendants.listeners.insert(0,count.clone());Some(Box::new(Recursive{split:None,count,descendants}))}else{None};
      let mut entry=Entry{key:*key,contact,recursive};initialize(key,&mut entry,convexes)?;staged.push(entry);
    }
    // Native creates the new records before tearing down the old suffix.
    // The teardown hook owns queue cancellation and graph/resource removal.
    for key in &removed{
      let entry=self.entries.iter_mut().find(|e|e.key==*key).ok_or("missing retired surface contact")?;retire(key,entry)?;
    }
    let mut old=std::mem::take(&mut self.entries);
    for key in keys{
      let entry=if let Some(at)=old.iter().position(|e|e.key==key){old.remove(at)}else{let at=staged.iter().position(|e|e.key==key).ok_or("missing staged surface contact")?;staged.remove(at)};
      self.entries.push(entry);
    }
    self.notify(self.entries.len() as i32-old_count as i32);
    Ok(Changes{lists,queries,created,removed})
 }
}

impl Entry<'_>{
 /// Transfer this query's current features, not a sphere proxy or the birth
 /// edge. Arena ledge keys must remain stable across repeated admissions.
 pub(crate) fn physical_features(&self,ledge_keys:[u32;2],convexes:[&Convex;2])->Result<[crate::ivp_managed_response::Feature;2],&'static str>{
    if self.recursive.is_some()||self.contact.flags&0xf000!=0||self.contact.query.frame==0||self.contact.query.geometry.is_none(){return Err("physical handoff requires a queried ordinary compact contact");}
    let q=&self.contact.query;let edges=[q.vertex,q.face];
    let features=std::array::from_fn::<_,2,_>(|i|crate::ivp_managed_response::Feature::from_compact(self.key.slots[i] as u64,ledge_keys[i],convexes[i],q.kinds[i],edges[i]));
    let [a,b]=features;Ok([a?,b?])
 }
 /// Called after the wake-neighbour distance query, while its cache generation
 /// is current. References survive physical admission and are released on error.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn admit_wake_neighbour(&self,physical:&mut crate::ivp_registered_contacts::CollisionContext<'_>,ledge_keys:[u32;2],convexes:[&Convex;2],cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,waking:usize,other:usize)->Result<bool,&'static str>{
    if self.contact.query.frame!=clock.frame||clock.time.to_bits()!=physical.current_time().to_bits(){return Err("compact wake admission requires current query features");}
    let features=self.physical_features(ledge_keys,convexes)?;
    for object in self.key.slots{cache.bind_contact_object(object as u64,object)?;}
    let leases=cache.retain_pair(self.key.slots,clock)?;
    let frames=cache.lease_frame(&leases.0).and_then(|a|cache.lease_frame(&leases.1).map(|b|[a,b]));
    cache.with_leases(leases,|release|{let result=frames.and_then(|frames|physical.admit_wake_features(self.contact.flags,features,frames,waking,other));release();result})
 }
 /// Collision admission holds the query caches through predictor staging, so
 /// trimming rotation endpoints cannot cause this serial's frames to resample.
 pub(crate) fn admit_impact(&self,physical:&mut crate::ivp_registered_contacts::CollisionContext<'_>,ledge_keys:[u32;2],convexes:[&Convex;2],cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context)->Result<crate::ivp_registered_contacts::AdmittedContact,&'static str>{
    if self.contact.query.frame!=clock.frame||clock.time.to_bits()!=physical.current_time().to_bits(){return Err("compact impact admission requires current query features");}
    let features=self.physical_features(ledge_keys,convexes)?;
    for object in self.key.slots{cache.bind_contact_object(object as u64,object)?;}let leases=cache.retain_pair(self.key.slots,clock)?;
    let frames=cache.lease_frame(&leases.0).and_then(|a|cache.lease_frame(&leases.1).map(|b|[a,b]));
    cache.with_leases(leases,|release|{let result=frames.and_then(|frames|physical.admit_features_with_frames(self.contact.flags,features,Some(frames)));release();result})
 }
 /// Constructor query notification is virtual: recursive10121340 is a RET,
 /// while an ordinary compact contact uses the default recovery response.
 pub(crate) fn refresh(&mut self,cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],dt:f32,gravity:[f32;3])->Result<crate::ivp_minimizer::owner::Update,&'static str>{
    let hulls=convexes.map(|c|c.topology.as_deref().ok_or("contact refresh requires native topology"));let hulls=[hulls[0]?,hulls[1]?];let headers=convexes.map(|c|c.triangle_headers.as_slice());
    if self.recursive.is_some(){self.contact.refresh_geometry(cache,clock,bodies,hulls,headers)}else{self.contact.refresh(cache,clock,bodies,hulls,headers,dt,gravity)}
 }
}

pub(crate) enum Registration {
 Scheduled(crate::ivp_distance_schedule::queue::Outcome),
 InvalidGeometry,
}
pub(crate) enum Recheck {
 Rearmed,
 Activated(crate::ivp_minimizer::owner::Update,Registration),
 Pending(crate::ivp_minimizer::owner::Update,pending::Outcome),
}
pub(crate) enum ActiveNotification {
 SameRevision,
 Invalid(crate::ivp_minimizer::owner::Update),
 Scheduled(crate::ivp_minimizer::owner::Update,crate::ivp_distance_schedule::queue::Outcome),
}
pub(crate) enum TimedEvent {
 InvalidGeometry,
 Impact,
 Retry(crate::ivp_distance_schedule::queue::Outcome),
}
#[cfg(test)]
#[path="ivp_initial_registration_tests.rs"]
mod registration_tests;
#[cfg(test)]
#[path="ivp_far_registration_tests.rs"]
mod far_registration_tests;
#[cfg(test)]
#[path="ivp_far_recheck_owner_tests.rs"]
mod far_recheck_tests;
#[cfg(test)]
#[path="ivp_pending_contact_tests.rs"]
mod pending_tests;
#[cfg(test)]
#[path="ivp_all_far_recheck_tests.rs"]
mod all_far_recheck_tests;
#[cfg(test)]
#[path="ivp_fresh_far_query_tests.rs"]
mod fresh_far_query_tests;
#[cfg(test)]
#[path="ivp_ordinary_inactive_tests.rs"]
mod ordinary_inactive_tests;
#[cfg(test)]
#[path="ivp_inactive_notify_tests.rs"]
mod inactive_notify_tests;
#[cfg(test)]
#[path="ivp_core_notify_tests.rs"]
mod core_notify_tests;
#[cfg(test)]
#[path="ivp_ordinary_psi_tests.rs"]
mod ordinary_psi_tests;
impl Entry<'_>{
 /// Ordinary1010bc90 runs after its heap entry has been removed. Geometry
 /// failure leaves the active links intact; only PSI's slot7 deactivates it.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn dispatch_ordinary_event_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  mut timing:crate::ivp_minimizer::owner::Schedule,
 )->Result<(crate::ivp_minimizer::owner::Update,TimedEvent),&'static str>{
  if self.recursive.is_some()||self.contact.flags&0x3000!=0||owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags||!owner.graph.active().contains(&id)||owner.queue.scheduled(id).is_some(){return Err("timed dispatch requires a popped ordinary active compact event");}
  let update=self.refresh(cache,clock,bodies,convexes,timing.dt,gravity)?;owner.graph.set_flags(id,self.contact.flags)?;
  let distance=if self.contact.flags&0xc000!=0{0.}else{self.contact.query.geometry.ok_or("valid timed event lacks closest geometry")?.distance};
  match crate::ivp_distance_schedule::callback(self.contact.flags,distance,timing.config)?{
   crate::ivp_distance_schedule::Callback::InvalidGeometry=>Ok((update,TimedEvent::InvalidGeometry)),
   crate::ivp_distance_schedule::Callback::Physical=>Ok((update,TimedEvent::Impact)),
   crate::ivp_distance_schedule::Callback::Reschedule{delay}=>{
    timing.delay=delay;timing.allow_far=false;
    let hulls=convexes.map(|c|c.topology.as_deref().ok_or("timed compact event requires native topology"));
    let result=self.contact.schedule(cache,clock,bodies,[hulls[0]?,hulls[1]?],&mut owner.queue,id,&mut owner.counter,timing)?;
    if result.far.is_some(){return Err("timed event retry transferred to far tracking");}
    owner.graph.set_flags(id,self.contact.flags)?;Ok((update,TimedEvent::Retry(result)))
   }
  }
 }
 /// Ordinary branch of1010b6c0. This PSI stage refreshes geometry regardless
 /// of collision revision and preserves valid contacts' timed events.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_ordinary_psi_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],dt:f32,gravity:[f32;3],
 )->Result<(crate::ivp_minimizer::owner::Update,bool),&'static str>{
  if self.recursive.is_some()||self.contact.flags&0x3000!=0||owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags||!owner.graph.active().contains(&id){return Err("ordinary PSI refresh requires matching ordinary active contact");}
  let update=self.refresh(cache,clock,bodies,convexes,dt,gravity)?;owner.graph.set_flags(id,self.contact.flags)?;
  let inactive=self.contact.flags&0xc000!=0;
  if inactive{
   if owner.queue.scheduled(id).is_some(){owner.queue.cancel(id)?;}
   owner.graph.make_inactive(id)?;self.contact.flags=owner.graph.flags(id)?;
  }Ok((update,inactive))
 }
 ///100e8b20 skips equal core collision revisions before touching query/cache state.
 /// Changed-revision valid geometry schedules with allow-far=false/delay2; a failed
 /// refresh stays active and preserves its existing queued event.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_active_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  mut timing:crate::ivp_minimizer::owner::Schedule,
 )->Result<ActiveNotification,&'static str>{
  if owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags||!owner.graph.active().contains(&id){return Err("active query requires matching active ownership");}
  if owner.core_revision(bodies[0].id)?==owner.core_revision(bodies[1].id)?{return Ok(ActiveNotification::SameRevision);}
  let update=self.refresh(cache,clock,bodies,convexes,timing.dt,gravity)?;owner.graph.set_flags(id,self.contact.flags)?;
  if self.contact.flags&0xc000!=0{return Ok(ActiveNotification::Invalid(update));}
  timing.allow_far=false;timing.delay=2;
  let hulls=convexes.map(|c|c.topology.as_deref().ok_or("active query requires native topology"));
  let out=self.contact.schedule(cache,clock,bodies,[hulls[0]?,hulls[1]?],&mut owner.queue,id,&mut owner.counter,timing)?;
  owner.graph.set_flags(id,self.contact.flags)?;Ok(ActiveNotification::Scheduled(update,out))
 }
 ///100e8cd0 performs1010db00 while still inactive. Only after the query's
 /// virtual notification does it move non-0x4000 results back to active lists;
 /// this object notification does not schedule a new timed event.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_inactive_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],dt:f32,gravity:[f32;3],
 )->Result<(crate::ivp_minimizer::owner::Update,bool),&'static str>{
  if self.recursive.is_some()||owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags||!owner.graph.inactive().contains(&id){return Err("inactive query requires matching ordinary inactive ownership");}
  let hulls=convexes.map(|c|c.topology.as_deref().ok_or("inactive query requires native topology"));let headers=convexes.map(|c|c.triangle_headers.as_slice());
  let update=self.contact.refresh_inactive(cache,clock,bodies,[hulls[0]?,hulls[1]?],headers,dt,gravity)?;
  owner.graph.set_flags(id,self.contact.flags)?;
  let activated=self.contact.flags&0xc000!=0x4000;
  if activated{owner.graph.reactivate(id,fast_cores)?;self.contact.flags=owner.graph.flags(id)?;}
  Ok((update,activated))
 }
 /// Ordinary native slot7 moves failed geometry to the inactive lists.
 /// Recursive records require their separate expansion callback instead.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn initialize_ordinary_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  timing:crate::ivp_minimizer::owner::Schedule,
 )->Result<(crate::ivp_minimizer::owner::Update,Registration),&'static str>{
  if self.recursive.is_some(){return Err("recursive contact requires recursive invalid policy");}
  self.initialize_owned(owner,id,fast_cores,cache,clock,bodies,convexes,gravity,timing,|entry,graph,queue|{
   if queue.scheduled(id).is_some(){queue.cancel(id)?;}
   graph.make_inactive(id)?;entry.contact.flags=graph.flags(id)?;Ok(())
  })
 }
 /// Bind native far recheck math to the actual contact, current physical
 /// predictors and object-owned motion budgets. Pending1010bf00 and normal
 ///1010bdf0 use their distinct registration rules after paired-list removal.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn recheck_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  timing:crate::ivp_minimizer::owner::Schedule,gap:f32,config:crate::ivp_far_distance::Config,
  invalid:impl FnOnce(&mut Self,&mut crate::ivp_mindist_graph::Graph,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  controllers:[Option<f32>;2],notify:impl FnMut(usize,bool)->Result<(),&'static str>,
 )->Result<Recheck,&'static str>{
  if owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags{return Err("far contact/graph binding differs");}
  let record=*owner.far(id).ok_or("far contact has no paired hull ownership")?;
  let geometry=self.contact.query.geometry.ok_or("far contact requires retained geometry")?;
  let input=crate::ivp_far_distance::Input{
   flags:self.contact.flags,now:clock.time,dt:timing.dt,gap,
   geometry:crate::ivp_far_distance::Geometry{projection:geometry.projection,linear_delta:record.linear_delta,distance:geometry.distance,normal:geometry.normal},
   cores:std::array::from_fn(|j|{let b=bodies[j];crate::ivp_far_distance::Core{time:b.prediction.time,start:b.prediction.start,velocity:b.prediction.velocity,linear:crate::ivp_bounds::speed(bevy::prelude::Vec3::from_array(b.linear[..3].try_into().unwrap())),angular:timing.properties[j].angular_bound}}),
   budgets:record.objects.map(|j|owner.hulls[j].budget),
  };
  match owner.recheck(id,&input,config)?{
   crate::ivp_far_distance::Action::Rearm{geometry,..}=>{
    self.contact.query.geometry=Some(crate::ivp_minimizer::Geometry{distance:geometry.distance,projection:geometry.projection,normal:geometry.normal});Ok(Recheck::Rearmed)
   },
   crate::ivp_far_distance::Action::Activate{pending:true}=>{
    let (update,result)=self.initialize_pending_owned(owner,id,fast_cores,cache,clock,bodies,convexes,timing,controllers,notify,record.linear_delta)?;Ok(Recheck::Pending(update,result))
   },
   crate::ivp_far_distance::Action::Activate{pending:false}=>{
    let (update,result)=self.initialize_owned(owner,id,fast_cores,cache,clock,bodies,convexes,gravity,timing,invalid)?;Ok(Recheck::Activated(update,result))
   },
  }
 }
 /// Constructor with paired hull-manager ownership as well as graph/queue links.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn initialize_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  timing:crate::ivp_minimizer::owner::Schedule,
  invalid:impl FnOnce(&mut Self,&mut crate::ivp_mindist_graph::Graph,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
 )->Result<(crate::ivp_minimizer::owner::Update,Registration),&'static str>{
  owner.bind_objects(self.key.slots,bodies.map(|b|b.id))?;
  let (update,result)=self.initialize_registered(&mut owner.graph,id,fast_cores,cache,clock,bodies,convexes,gravity,&mut owner.queue,&mut owner.counter,timing,invalid)?;
  if let Registration::Scheduled(out)=&result{if let Some((keys,delta))=out.far{owner.track_far(id,keys,delta)?;}}
  Ok((update,result))
 }
 ///1010bdf0 constructor ownership through its first query and event decision.
 /// Shape/sweep/budget properties are provider inputs. Far-list transfers and
 /// recursive slot7 effects remain explicit owner responsibilities.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn initialize_registered(
  &mut self,graph:&mut crate::ivp_mindist_graph::Graph,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],gravity:[f32;3],
  queue:&mut crate::ivp_event_queue::Queue,counter:&mut u32,
  mut timing:crate::ivp_minimizer::owner::Schedule,
  invalid:impl FnOnce(&mut Self,&mut crate::ivp_mindist_graph::Graph,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
 )->Result<(crate::ivp_minimizer::owner::Update,Registration),&'static str>{
  if graph.objects(id)?!=self.key.slots{return Err("constructor graph/object binding differs");}
  if graph.flags(id)?!=self.contact.flags{return Err("constructor graph/contact flags differ");}
  let mut update=None;
  let route=graph.activate_initial(id,fast_cores,|_,flags|{
   self.contact.flags=flags;
   update=Some(self.refresh(cache,clock,bodies,convexes,timing.dt,gravity)?);
   Ok(self.contact.flags)
  })?;
  let result=match route{
   crate::ivp_mindist_graph::InitialRoute::InvalidGeometry=>{
    invalid(self,graph,queue)?;Registration::InvalidGeometry
   },
   crate::ivp_mindist_graph::InitialRoute::Schedule=>{
    // Native constructor passes delay0 and derives hull eligibility from the
    // two core movement-state bytes, rather than accepting the caller's mode.
    timing.delay=0;timing.allow_far=((bodies[0].flags>>8)|(bodies[1].flags>>8))<0x21;
    let hulls=convexes.map(|c|c.topology.as_deref().ok_or("constructor requires native topology"));
    let outcome=self.contact.schedule(cache,clock,bodies,[hulls[0]?,hulls[1]?],queue,id,counter,timing)?;
    if outcome.far.is_some(){graph.unlink(id)?;}
    graph.set_flags(id,self.contact.flags)?;Registration::Scheduled(outcome)
   },
  };
  Ok((update.ok_or("constructor query did not run")?,result))
 }
}

#[cfg(test)]
#[path="ivp_pending_psi_tests.rs"]
mod pending_psi_tests;

#[cfg(test)]
mod snapshot_tests{
 use super::*;
 #[test]
 fn ordinary_staging_refuses_shared_recursive_counter_ownership(){
  let solid=PhySolid::default();let count=Arc::new(AtomicI32::new(7));
  let mut root=Contacts::new([0,1],[&solid,&solid]).unwrap();root.listeners.push(count.clone());
  assert!(root.try_clone_ordinary().is_err());assert_eq!(count.load(Ordering::Relaxed),7);assert_eq!(Arc::strong_count(&count),2);
  root.listeners.clear();
  let mut descendants=Contacts::new([0,1],[&solid,&solid]).unwrap();descendants.listeners.push(count.clone());
  root.entries.push(Entry{key:Pair{slots:[0,1],ledges:[0,0]},contact:Contact::new([0,1],crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS,0.).unwrap(),recursive:Some(Box::new(Recursive{split:None,count:count.clone(),descendants}))});
  assert!(root.try_clone_ordinary().is_err());assert_eq!(count.load(Ordering::Relaxed),7);assert_eq!(Arc::strong_count(&count),3);
 }
}
