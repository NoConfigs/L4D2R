//! Spatial-tree notifications -> persistent compact surface contacts.
//! The lifecycle provider owns graph registration, scheduling and retirement.
//! Default-filter broadphase only; recursive contacts keep their own callbacks.
use super::{Contacts,Entry,Input};
use crate::{ivp_trial_bounds as bounds,ivp_joint_apply::Body,ivp_world_cache::{WorldCache,Context as Clock},ivp_hull_budget::Budget};
use l4d2_formats::phy::{PhySolid,Convex};
use std::{cell::RefCell,collections::BTreeMap};

#[derive(Clone,Copy)]
pub(crate) struct Object<'a>{pub id:usize,pub core:usize,pub solid:&'a PhySolid,pub margin:f32,pub core_radius:f32,pub core_angular_radius:f32,pub cached_motion:[f32;2]}
#[path="ivp_compact_owner.rs"]
pub(crate) mod owner;
pub(crate) type FarRegistration=([f32;2],f32);
pub(crate) struct Created{pub id:usize,pub far:Option<FarRegistration>}
pub(crate) trait Lifecycle{
 fn created(&mut self,entry:&mut Entry<'_>,convexes:[&Convex;2],budgets:[Budget;2],motion:[crate::ivp_minimizer::owner::Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Created,&'static str>;
 fn reactivated(&mut self,_id:usize,_entry:&mut Entry<'_>,_convexes:[&Convex;2],_budgets:[Budget;2],_motion:[crate::ivp_minimizer::owner::Properties;2],_bodies:&mut[Body;2],_cache:&mut WorldCache)->Result<Option<FarRegistration>,&'static str>{Err("compact far reactivation provider is missing")}
 fn retired(&mut self,id:usize,entry:&mut Entry<'_>)->Result<(),&'static str>;
}
#[derive(Clone,Copy)]
struct Location{broad:usize,key:crate::ivp_surface_admission::Pair}
pub(crate) struct Discovery<'a>{world:bounds::World,objects:BTreeMap<usize,Object<'a>>,order:Vec<usize>,motion:BTreeMap<usize,crate::ivp_minimizer::owner::Properties>,far:BTreeMap<usize,f32>,coarse:BTreeMap<usize,Contacts<'a>>,locations:BTreeMap<usize,Location>,ledges:BTreeMap<(usize,usize),u32>,dt:f32}

impl<'a> Discovery<'a>{
 #[cfg(test)]
 pub(crate) fn query_snapshot(&self,owner:&super::ownership::Ownership,queue:&crate::ivp_event_queue::Queue)->Result<serde_json::Value,&'static str>{
  let mut rows=vec![];
  for (&id,loc) in &self.locations{let entry=self.coarse[&loc.broad].entries().iter().find(|e|e.key==loc.key).ok_or("diagnostic compact entry missing")?;let q=&entry.contact.query;
   rows.push(serde_json::json!({"id":id,"objects":owner.graph.objects(id)?.map(|i|i-1),"flags":entry.contact.flags,"kinds":q.kinds,"frame":q.frame,"geometry":q.geometry.map(|g|[g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]),"queue":queue.scheduled(id).map(|(epoch,key)|[epoch.to_bits(),key.to_bits() as u64])}));
  }Ok(serde_json::json!(rows))
 }
 pub(crate) fn snapshot_with_counters(&self,counters:&mut super::CounterSnapshot)->Self{
  let coarse=self.coarse.iter().map(|(&id,c)|(id,c.snapshot_with_counters(counters))).collect();
  Self{world:self.world.clone(),objects:self.objects.clone(),order:self.order.clone(),motion:self.motion.clone(),far:self.far.clone(),coarse,locations:self.locations.clone(),ledges:self.ledges.clone(),dt:self.dt}
 }
 pub(crate) fn try_clone_ordinary(&self)->Result<Self,&'static str>{
  let coarse=self.coarse.iter().map(|(&id,c)|c.try_clone_ordinary().map(|c|(id,c))).collect::<Result<_,_>>()?;
  Ok(Self{world:self.world.clone(),objects:self.objects.clone(),order:self.order.clone(),motion:self.motion.clone(),far:self.far.clone(),coarse,locations:self.locations.clone(),ledges:self.ledges.clone(),dt:self.dt})
 }
 pub(crate) fn cached_motion(&self,id:usize)->Result<[f32;2],&'static str>{self.objects.get(&id).map(|o|o.cached_motion).ok_or("unknown compact motion object")}
 #[cfg(test)]
 pub(crate) fn object_budget(&self,id:usize)->Result<Budget,&'static str>{self.world.object_budget(id)}
 #[cfg(test)]
 pub(crate) fn motion_snapshot(&self,id:usize)->serde_json::Value{
  let o=self.objects[&id];let p=self.motion[&id];let b=self.world.object_budget(id).unwrap();
  serde_json::json!({"linear":o.cached_motion[0].to_bits(),"speed":p.angular_speed.to_bits(),"bound":p.angular_bound.to_bits(),"axis":p.sweep_axis.to_array().map(f32::to_bits),"time":b.time.to_bits(),"rates":b.rates.map(f32::to_bits),"accumulated":b.accumulated.map(f32::to_bits),"predicted":b.predicted.to_bits(),"threshold":b.threshold.to_bits()})
 }
 #[cfg(test)]
 pub(crate) fn far_snapshot(&self,object:usize)->serde_json::Value{
  let mut rows=vec![];
  for (&id,&delta) in &self.far{
   let keys=self.world.distance_keys(id).unwrap();let entry=self.entry_read(id);
   if let Some(side)=entry.key.slots.iter().position(|&o|o==object){let g=entry.contact.query.geometry.unwrap();rows.push(serde_json::json!({"objects":entry.key.slots,"flags":entry.contact.flags&0x3fffffff,"frame":entry.contact.query.frame,"key":keys[side].to_bits(),"geometry":[g.projection.to_bits(),delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]}));}
  }
  rows.sort_by_key(|r|r["objects"].to_string());serde_json::Value::Array(rows)
 }
 #[cfg(test)]
 fn entry_read(&self,id:usize)->&Entry<'a>{
  let loc=self.locations[&id];let entry=self.coarse[&loc.broad].entries().iter().find(|e|e.key==loc.key).unwrap();entry
 }
 pub(crate) fn new(objects:Vec<Object<'a>>,order:&[usize],bodies:&[Body],dt:f32)->Result<Self,&'static str>{
  if bodies.is_empty()||objects.is_empty()||order.len()!=objects.len(){return Err("invalid compact discovery registration");}
  let mut registered=BTreeMap::new();let mut ledges=BTreeMap::new();let mut next=1u32;
  for object in objects{
   if object.core>=bodies.len()||!object.margin.is_finite()||object.margin<0.||!object.core_radius.is_finite()||object.core_radius<=0.||!object.core_angular_radius.is_finite()||object.core_angular_radius<0.||object.cached_motion.iter().any(|v|!v.is_finite()||*v<0.)||registered.insert(object.id,object).is_some(){return Err("invalid compact discovery object");}
   let tree=object.solid.compact_tree.as_ref().ok_or("compact discovery requires authored tree")?;
   for &key in tree.ledge_headers.keys(){ledges.insert((object.id,key),next);next=next.checked_add(1).ok_or("compact feature arena exhausted")?;}
  }
  let mut sorted=order.to_vec();sorted.sort_unstable();if sorted!=registered.keys().copied().collect::<Vec<_>>(){return Err("compact discovery order differs from registrations");}
  let size=registered.keys().next_back().unwrap().checked_add(1).ok_or("compact discovery object arena overflow")?;
  let mut descriptors=(0..size).map(|_|bounds::Object::new(0,0,true,0.,[0.;3])).collect::<Vec<_>>();
  for (&id,o) in &registered{let b=bodies[o.core];descriptors[id]=bounds::Object::new(o.core,b.object_flags,b.flags&2!=0,o.core_radius,b.current.position.to_array());descriptors[id].body.linear=o.cached_motion[0];descriptors[id].body.angular=o.cached_motion[1];}
  let motion=registered.iter().map(|(&id,o)|(id,crate::ivp_minimizer::owner::Properties{radius:o.core_radius,inverse_diameter:(2.*o.core_radius).recip(),angular_speed:0.,angular_bound:o.cached_motion[1],sweep_axis:bevy::prelude::Vec3::X})).collect();
  Ok(Self{world:bounds::World::new(descriptors,order,dt)?,objects:registered,order:order.to_vec(),motion,far:BTreeMap::new(),coarse:BTreeMap::new(),locations:BTreeMap::new(),ledges,dt})
 }
 ///100eb7d0 visits each unit core and its objects backwards after movement
 /// polling.100e8cd0 reactivates eligible inactive queries before graph repair.
 pub(crate) fn refresh_inactive_unit(&mut self,members:&[usize],owner:&mut super::ownership::Ownership,clock:Clock,bodies:&mut[Body],cache:&mut WorldCache,gravity:[f32;3])->Result<(),&'static str>{
  for &core in members.iter().rev(){
   let objects=self.order.iter().rev().copied().filter(|id|self.objects[id].core==core).collect::<Vec<_>>();
   for object in objects{owner.notify_object(object,|owner,id,_|{
    let slots=owner.graph.objects(id)?;let cores=slots.map(|s|self.objects[&s].core);let mut pair=cores.map(|c|bodies[c]);
    for side in 0..2{cache.update_core(cores[side],pair[side].prediction)?;cache.set_flags(slots[side],pair[side].object_flags)?;}
    let dt=self.dt;let (entry,convexes,_)=self.entry_mut(id)?;
    let result=entry.refresh_inactive_owned(owner,id,[false;2],cache,clock,&mut pair,convexes,dt,gravity);
    for side in 0..2{bodies[cores[side]]=pair[side];}result.map(|_|())
   })?;}
  }Ok(())
 }
 ///100e8d60 temporarily marks the core/object as state0x21. Restore only the
 /// state bytes afterwards, preserving any unrelated flags changed by callbacks.
 /// Motion inputs are the native cached linear/angular bounds at this phase,
 /// not fresh velocity lengths: wake subsequently rebuilds/clears those caches.
 ///100f1800 copies an awake movement classification without a wake/bounds
 /// notification. Keep both flag caches current without rebuilding queries.
 pub(crate) fn sync_active_flags(&mut self,id:usize,flags:u32,cache:&mut WorldCache)->Result<(),&'static str>{
  if !self.objects.contains_key(&id)||(flags&255)==0||(flags&255)>3{return Err("invalid active object flag copy");}
  self.world.set_flags(id,flags);cache.set_flags(id,flags)
 }
 pub(crate) fn notify_wake(&mut self,id:usize,clock:Clock,cached_motion:[f32;2],bodies:&mut[Body],cache:&mut WorldCache,lifecycle:&mut dyn Lifecycle)->Result<Vec<bounds::Request>,&'static str>{
  let object=*self.objects.get(&id).ok_or("unknown compact discovery object")?;
  if cached_motion.iter().any(|v|!v.is_finite()||*v<0.){return Err("invalid compact wake motion bounds");}
  if cache.object_core(id)?!=object.core{return Err("compact discovery cache/core differs");}
  let b=&mut bodies[object.core];let saved=(b.flags,b.object_flags);b.flags=(b.flags&0xff)|0x2100;b.object_flags=(b.object_flags&!0xff)|0x21;
  let flags=b.object_flags;let center=b.current.position.to_array();
  let result={let mut context=Dispatch{dt:self.dt,motion:&self.motion,far:&mut self.far,objects:&self.objects,coarse:&mut self.coarse,locations:&mut self.locations,bodies,cache,clock,lifecycle};
   self.world.notify_with_hooks(id,clock.time,self.dt,flags,center,cached_motion[0],cached_motion[1],&mut context)
  };
  let b=&mut bodies[object.core];b.flags=(b.flags&0xff)|(saved.0&0xff00);b.object_flags=(b.object_flags&!0xff)|(saved.1&0xff);
  self.world.set_flags(id,b.object_flags);let restored=cache.set_flags(id,b.object_flags);
  match result{Ok(requests)=>{restored?;Ok(requests)},Err(error)=>{restored?;Err(error)}}
 }
 /// Sleeping objects keep their motion caches, but stop accumulating hull
 /// motion. Rebase paired listeners before detaching the transform cache.
 pub(crate) fn notify_sleep(&mut self,id:usize,clock:Clock,bodies:&mut[Body],cache:&mut WorldCache,lifecycle:&mut dyn Lifecycle)->Result<Vec<bounds::Request>,&'static str>{
  let object=*self.objects.get(&id).ok_or("unknown compact sleep object")?;
  if cache.object_core(id)?!=object.core{return Err("compact sleep cache/core differs");}
  let body=*bodies.get(object.core).ok_or("unknown compact sleep core")?;
  if body.object_flags&255!=8{return Err("compact sleep notification has awake flags");}
  cache.update_core(object.core,body.prediction)?;cache.set_flags(id,body.object_flags)?;
  let requests={let mut context=Dispatch{dt:self.dt,motion:&self.motion,far:&mut self.far,objects:&self.objects,coarse:&mut self.coarse,locations:&mut self.locations,bodies,cache,clock,lifecycle};
   self.world.sleep_with_flags_and_hooks(id,clock.time,self.dt,body.current.position.to_array(),body.object_flags,&mut context)?
  };
  cache.detach(id)?;Ok(requests)
 }
 pub(crate) fn entry_mut(&mut self,id:usize)->Result<(&mut Entry<'a>,[&'a Convex;2],[u32;2]),&'static str>{
  let loc=*self.locations.get(&id).ok_or("unknown discovered compact child")?;
  let convexes=std::array::from_fn::<_,2,_>(|s|self.objects[&loc.key.slots[s]].solid.compact_ledge(loc.key.ledges[s]).ok_or("discovered compact ledge disappeared"));let [a,b]=convexes;
  let keys=std::array::from_fn::<_,2,_>(|s|self.ledges.get(&(loc.key.slots[s],loc.key.ledges[s])).copied().ok_or("discovered ledge lacks feature arena identity"));let [ka,kb]=keys;
  let entry=self.coarse.get_mut(&loc.broad).ok_or("discovered child lost parent")?.entries_mut().iter_mut().find(|e|e.key==loc.key).ok_or("discovered child lost entry")?;
  Ok((entry,[a?,b?],[ka?,kb?]))
 }
 /// Consume a popped event through the authoritative world queue. Geometry
 /// recovery writes back to the caller's canonical bank, including on failure;
 /// the encompassing tick owner is responsible for transaction rollback.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn dispatch_ordinary_event(&mut self,id:usize,owner:&mut super::ownership::Ownership,queue:&mut crate::ivp_event_queue::Queue,clock:Clock,deadline:f64,bodies:&mut[Body],cache:&mut WorldCache,gravity:[f32;3],config:crate::ivp_distance_schedule::Config,face_epsilon:f32)->Result<super::TimedEvent,&'static str>{
  let slots=owner.graph.objects(id)?;
  let definitions=slots.map(|s|self.objects.get(&s).copied().ok_or("event object is not registered in compact discovery"));let [a,b]=definitions;let definitions=[a?,b?];
  let cores=definitions.map(|o|o.core);
  if cores.iter().any(|&c|c>=bodies.len()){return Err("compact event body bank membership differs");}
  let properties=slots.map(|s|self.motion[&s]);
  let budgets=[self.world.object_budget(slots[0])?,self.world.object_budget(slots[1])?];
  let mut pair=cores.map(|c|bodies[c]);
  let timing=crate::ivp_minimizer::owner::Schedule{deadline,dt:self.dt,delay:255,config,face_epsilon,properties,budgets:std::array::from_fn(|i|crate::ivp_distance_schedule::Budget{time:budgets[i].time,flags:pair[i].object_flags,rates:budgets[i].rates,accumulated:budgets[i].accumulated}),allow_far:false};
  let (entry,convexes,_)=self.entry_mut(id)?;
  let result=owner.with_queue(queue,|owner|entry.dispatch_ordinary_event_owned(owner,id,cache,clock,&mut pair,convexes,gravity,timing).map(|(_,event)|event));
  for i in 0..2{bodies[cores[i]]=pair[i];}
  result
 }
 /// Ordinary branches of1010b6c0 then1010c450, after all predicted hull
 /// notifications. Paired far listeners belong to the same bounds world.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_schedule(&mut self,owner:&mut super::ownership::Ownership,queue:&mut crate::ivp_event_queue::Queue,clock:Clock,deadline:f64,bodies:&mut[Body],cache:&mut WorldCache,gravity:[f32;3],config:crate::ivp_distance_schedule::Config,face_epsilon:f32)->Result<(),&'static str>{
  owner.with_queue(queue,|owner|{
   owner.refresh_active(|owner,id|{
    let slots=owner.graph.objects(id)?;let cores=slots.map(|s|self.objects[&s].core);let mut pair=cores.map(|c|bodies[c]);
    let dt=self.dt;let (entry,convexes,_)=self.entry_mut(id)?;
    let result=entry.refresh_ordinary_psi_owned(owner,id,cache,clock,&mut pair,convexes,dt,gravity);
    for i in 0..2{bodies[cores[i]]=pair[i];}result.map(|_|())
   })?;
   for id in owner.graph.active().to_vec(){
    let slots=owner.graph.objects(id)?;let cores=slots.map(|s|self.objects[&s].core);let pair=cores.map(|c|bodies[c]);
    let properties=slots.map(|s|self.motion[&s]);let budgets=[self.world.object_budget(slots[0])?,self.world.object_budget(slots[1])?];
    let timing=crate::ivp_minimizer::owner::Schedule{deadline,dt:self.dt,delay:1,config,face_epsilon,properties,budgets:std::array::from_fn(|i|crate::ivp_distance_schedule::Budget{time:budgets[i].time,flags:pair[i].object_flags,rates:budgets[i].rates,accumulated:budgets[i].accumulated}),allow_far:true};
    let (entry,convexes,_)=self.entry_mut(id)?;
    if entry.recursive.is_some()||entry.contact.flags&0x3000!=0{return Err("ordinary PSI scheduling received recursive/pending contact");}
    let hulls=convexes.map(|c|c.topology.as_deref().ok_or("ordinary PSI schedule lacks topology"));
    let out=entry.contact.schedule(cache,clock,&pair,[hulls[0]?,hulls[1]?],&mut owner.queue,id,&mut owner.counter,timing)?;
    owner.graph.set_flags(id,entry.contact.flags)?;
    if let Some((keys,delta))=out.far{
     owner.graph.unlink(id)?;
     self.world.register_distance(id,slots,keys)?;
     if self.far.insert(id,delta).is_some(){return Err("active contact already has far ownership");}
    }
   }Ok(())
  })
 }
 ///10104320 prepares the reverse changed-core/object budgets before dispatching
 /// any hull callback. The wave has already rebuilt/clipped the physical bank.
 pub(crate) fn rebuild_bounds(&mut self,changed:&[(usize,crate::ivp_motion::PredictionMotion)],clock:Clock,deadline:f64,bodies:&mut[Body],cache:&mut WorldCache,lifecycle:&mut dyn Lifecycle)->Result<Vec<bounds::Request>,&'static str>{
  let predictions=changed.iter().rev().copied().collect::<Vec<_>>();
  self.predict_bounds(&predictions,clock,deadline,bodies,cache,lifecycle)
 }
 ///1010fd30/1010f960 consume the actual core prediction call order. All
 /// original deltas must be retained before endpoint normalization/gyro updates.
 pub(crate) fn predict_bounds(&mut self,changed:&[(usize,crate::ivp_motion::PredictionMotion)],clock:Clock,deadline:f64,bodies:&mut[Body],cache:&mut WorldCache,lifecycle:&mut dyn Lifecycle)->Result<Vec<bounds::Request>,&'static str>{
  let remaining=(deadline-clock.time) as f32;
  let mut seen=std::collections::BTreeSet::new();
  if !remaining.is_finite()||remaining<0.||changed.iter().any(|&(c,m)|c>=bodies.len()||!seen.insert(c)||bodies[c].flags&0x12!=0||m.time.to_bits()!=clock.time.to_bits()||m.dt.to_bits()!=remaining.to_bits()){return Err("invalid compact impact prediction batch");}
  for o in self.objects.values(){let b=bodies[o.core];cache.update_core(o.core,b.prediction)?;cache.set_flags(o.id,b.object_flags)?;self.world.set_flags(o.id,b.object_flags);}
  let mut predictions=vec![];
  for &(core,motion) in changed{
   let b=bodies[core];
   let delta=motion.delta;
   let objects=self.order.iter().rev().copied().filter(|id|self.objects[id].core==core).collect::<Vec<_>>();
   for id in objects{
    let o=self.objects[&id];let (axis,speed,bound)=crate::ivp_motion::rotation_sweep(delta,motion.rotation,motion.inverse_dt,o.core_angular_radius);
    self.motion.insert(id,crate::ivp_minimizer::owner::Properties{radius:o.core_radius,inverse_diameter:(2.*o.core_radius).recip(),angular_speed:speed,angular_bound:bound,sweep_axis:axis});
    let linear=crate::ivp_bounds::speed(bevy::prelude::Vec3::from_array(b.linear[..3].try_into().unwrap()));
    self.objects.get_mut(&id).unwrap().cached_motion=[linear,bound];
    predictions.push(bounds::Prediction{object:id,center:b.prediction.sample(clock.time,crate::ivp_motion::ObjectFrame{shift:None,rotation:None}).position.to_array(),linear,angular:bound});
   }
  }
  let prepared=self.world.prepare_predictions(clock.time,remaining,&predictions)?;
  let mut context=Dispatch{dt:self.dt,motion:&self.motion,far:&mut self.far,objects:&self.objects,coarse:&mut self.coarse,locations:&mut self.locations,bodies,cache,clock,lifecycle};
  self.world.dispatch_predictions(prepared,&mut context)
 }
 ///100f1600 stamps each changed core in reverse order.100e8b20 only queries
 /// pairs whose endpoint revisions differ; it never transfers them to far hulls.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn recheck_impact_pairs(&mut self,changed:&[usize],owner:&mut super::ownership::Ownership,clock:Clock,deadline:f64,bodies:&mut[Body],cache:&mut WorldCache,gravity:[f32;3],config:crate::ivp_distance_schedule::Config,face_epsilon:f32)->Result<(),&'static str>{
  for &core in changed.iter().rev(){
   let objects=self.order.iter().copied().filter(|id|self.objects[id].core==core).collect::<Vec<_>>();
   owner.notify_core_changed(core,&objects,|owner,id,_|{
    let slots=owner.graph.objects(id)?;
    let cores=slots.map(|s|self.objects[&s].core);
    let properties=slots.map(|s|self.motion[&s]);
    let budgets=[self.world.object_budget(slots[0])?,self.world.object_budget(slots[1])?];
    let mut pair=cores.map(|c|bodies[c]);
    let timing=crate::ivp_minimizer::owner::Schedule{deadline,dt:self.dt,delay:2,config,face_epsilon,properties,budgets:std::array::from_fn(|i|crate::ivp_distance_schedule::Budget{time:budgets[i].time,flags:pair[i].object_flags,rates:budgets[i].rates,accumulated:budgets[i].accumulated}),allow_far:false};
    let (entry,convexes,_)=self.entry_mut(id)?;
    let result=entry.refresh_active_owned(owner,id,cache,clock,&mut pair,convexes,gravity,timing);
    for i in 0..2{bodies[cores[i]]=pair[i];}
    match result?{super::ActiveNotification::Scheduled(_,out) if out.far.is_some()=>Err("compact impact recheck transferred to far tracking"),_=>Ok(())}
   })?;
  }Ok(())
 }
}

struct Dispatch<'a,'b>{dt:f32,motion:&'b BTreeMap<usize,crate::ivp_minimizer::owner::Properties>,far:&'b mut BTreeMap<usize,f32>,objects:&'b BTreeMap<usize,Object<'a>>,coarse:&'b mut BTreeMap<usize,Contacts<'a>>,locations:&'b mut BTreeMap<usize,Location>,bodies:&'b mut[Body],cache:&'b mut WorldCache,clock:Clock,lifecycle:&'b mut dyn Lifecycle}
impl bounds::Hooks for Dispatch<'_,'_>{
 fn distance(&mut self,event:bounds::DistanceEvent)->Result<bounds::DistanceAction,&'static str>{
  use crate::ivp_far_distance as far;
  let id=match event{bounds::DistanceEvent::Recheck{pair,..}|bounds::DistanceEvent::Rebase{pair,..}=>pair};
  let loc=*self.locations.get(&id).ok_or("far listener lost compact child")?;
  let delta=self.far.get_mut(&id).ok_or("far listener lacks retained linear delta")?;
  if let bounds::DistanceEvent::Rebase{motion,linear,..}=event{*delta=(motion-linear)+*delta;return Ok(bounds::DistanceAction::Keep);}
  let bounds::DistanceEvent::Recheck{objects,gap,budgets,..}=event else{unreachable!()};
  if objects!=loc.key.slots{return Err("far listener object order differs");}
  let definitions=objects.map(|s|self.objects[&s]);
  let entry=self.coarse.get_mut(&loc.broad).ok_or("far child lost parent")?.entries_mut().iter_mut().find(|e|e.key==loc.key).ok_or("far child disappeared")?;
  if entry.recursive.is_some(){return Err("recursive compact far callback is not connected");}
  let geometry=entry.contact.query.geometry.ok_or("far contact lacks retained geometry")?;
  let input=far::Input{flags:entry.contact.flags,now:self.clock.time,dt:self.dt,gap,geometry:far::Geometry{projection:geometry.projection,linear_delta:*delta,distance:geometry.distance,normal:geometry.normal},cores:definitions.map(|o|{let b=self.bodies[o.core];far::Core{time:b.prediction.time,start:b.prediction.start,velocity:b.prediction.velocity,linear:o.cached_motion[0],angular:o.cached_motion[1]}}),budgets};
  match far::update(&input,far::Config::default())?{
   far::Action::Rearm{geometry,keys,first}=>{
    *delta=geometry.linear_delta;entry.contact.query.geometry=Some(crate::ivp_minimizer::Geometry{projection:geometry.projection,distance:geometry.distance,normal:geometry.normal});Ok(bounds::DistanceAction::Rearm{keys,first})
   },
   far::Action::Activate{pending:true}=>Err("pending compact far reactivation is not connected"),
   far::Action::Activate{pending:false}=>Ok(bounds::DistanceAction::ActivateAfterRemoval)
  }
 }
 fn activated(&mut self,id:usize,objects:[usize;2],budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{
  let loc=*self.locations.get(&id).ok_or("reactivated compact child lost identity")?;
  if objects!=loc.key.slots||!self.far.contains_key(&id){return Err("reactivated compact far ownership differs");}
  let definitions=objects.map(|s|self.objects[&s]);let cores=definitions.map(|o|o.core);
  let convexes=std::array::from_fn::<_,2,_>(|s|definitions[s].solid.compact_ledge(loc.key.ledges[s]).ok_or("far child lost compact ledge"));let [a,b]=convexes;
  let entry=self.coarse.get_mut(&loc.broad).ok_or("reactivated child lost parent")?.entries_mut().iter_mut().find(|e|e.key==loc.key).ok_or("reactivated child disappeared")?;
  let mut bodies=cores.map(|c|self.bodies[c]);let result=self.lifecycle.reactivated(id,entry,[a?,b?],budgets,objects.map(|s|self.motion[&s]),&mut bodies,self.cache);
  for i in 0..2{self.bodies[cores[i]]=bodies[i];}
  let replacement=result?;
  if let Some((_,linear_delta))=replacement{self.far.insert(id,linear_delta);}else{self.far.remove(&id);}
  Ok(replacement.map(|(keys,_)|keys))
 }

 fn geometry(&mut self,broad:usize,objects:[usize;2],margin:f32,budgets:[Budget;2])->Result<Vec<bounds::DistanceEdit>,&'static str>{
  let definitions=objects.map(|id|self.objects.get(&id).copied().ok_or("spatial query returned unregistered compact object"));let [a,b]=definitions;let definitions=[a?,b?];
  for o in definitions{let body=self.bodies[o.core];if self.cache.object_core(o.id)?!=o.core{return Err("discovered object cache/core differs");}self.cache.update_core(o.core,body.prediction)?;self.cache.set_flags(o.id,body.object_flags)?;}
  let leases=self.cache.retain_pair(objects,self.clock)?;
  let poses=self.cache.lease_frame(&leases.0).and_then(|a|self.cache.lease_frame(&leases.1).map(|b|[a.pose,b.pose]));
  let poses=self.cache.with_leases(leases,|release|{release();poses})?;
  let input=Input{bodies:std::array::from_fn(|i|{let o=definitions[i];super::Body{prediction:self.bodies[o.core].prediction,prepared_pose:poses[i],core_radius:o.core_radius,margin:o.margin}}),at:self.clock.time,margin,provided:[None;2],restricted:[None;2]};
  if !self.coarse.contains_key(&broad){self.coarse.insert(broad,Contacts::new(objects,definitions.map(|o|o.solid))?);}
  let motion=objects.map(|id|self.motion[&id]);let mut edits=vec![];
  let shared=RefCell::new((&mut self.lifecycle,&mut self.bodies,&mut self.cache,&mut self.locations,&mut self.far,&mut edits));
  self.coarse.get_mut(&broad).unwrap().update(input,|key,entry,convexes|{
   let mut shared=shared.borrow_mut();let (lifecycle,bank,cache,locations,far,edits)=&mut *shared;let cores=definitions.map(|o|o.core);let mut bodies=cores.map(|id|bank[id]);
   let result=lifecycle.created(entry,convexes,budgets,motion,&mut bodies,cache);for i in 0..2{bank[cores[i]]=bodies[i];}let result=result?;let id=result.id;
   if let Some((keys,delta))=result.far{if far.insert(id,delta).is_some(){return Err("duplicate created far contact");}edits.push(bounds::DistanceEdit::Register{pair:id,objects:key.slots,keys});}
   if locations.contains_key(&id){return Err("compact lifecycle reused live child identity");}locations.insert(id,Location{broad,key:*key});Ok(())
  },|key,entry|{
   let mut shared=shared.borrow_mut();let (lifecycle,_,_,locations,far,edits)=&mut *shared;let id=locations.iter().find_map(|(&id,loc)|(loc.broad==broad&&loc.key==*key).then_some(id)).ok_or("retired compact child lacks identity")?;
   lifecycle.retired(id,entry)?;locations.remove(&id);if far.remove(&id).is_some(){edits.push(bounds::DistanceEdit::Clear{pair:id});}Ok(())
  })?;drop(shared);Ok(edits)
 }
 fn removed(&mut self,broad:usize,_:[usize;2])->Result<Vec<bounds::DistanceEdit>,&'static str>{
  let contacts=self.coarse.get_mut(&broad).ok_or("removed compact parent is missing")?;
  let mut edits=vec![];
  contacts.clear(|key,entry|{let id=self.locations.iter().find_map(|(&id,loc)|(loc.broad==broad&&loc.key==*key).then_some(id)).ok_or("removed compact child lacks identity")?;self.lifecycle.retired(id,entry)?;self.locations.remove(&id);if self.far.remove(&id).is_some(){edits.push(bounds::DistanceEdit::Clear{pair:id});}Ok(())})?;
  self.coarse.remove(&broad);Ok(edits)
 }
}
