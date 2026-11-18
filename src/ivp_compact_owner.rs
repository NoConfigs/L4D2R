//! Transactional ordinary compact tick composition. The canonical physical
//! world owns bodies and the clock; this owner stages contact/cache/hull state.
//! Recursive counters, custom controllers and held/wheel lifecycle remain
//! explicit unsupported branches rather than shallow-cloned mutable aliases.
use super::{Discovery,Lifecycle,Created,FarRegistration};
use crate::{ivp_surface_admission::contacts::{Entry,ownership::Ownership,TimedEvent,Registration},
 ivp_world_cache::{WorldCache,Context as Clock},ivp_joint_apply::Body,
 ivp_registered_contacts::{ContactWorld,CollisionContext},ivp_event_queue::Queue,
 ivp_minimizer::owner::{Properties,Schedule},ivp_distance_schedule::Config};
use l4d2_formats::phy::Convex;
use std::{cell::RefCell,collections::BTreeMap};
#[cfg(test)]
thread_local!{pub(crate) static OWNER_AUDIT:RefCell<Option<Vec<serde_json::Value>>>=const{RefCell::new(None)};}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum Phase{Inactive,Prediction,Predicted,Timed,Impact,Woken,Rebuilt}
#[derive(Clone,Copy)]
pub(crate) struct Event<'a>{pub phase:Phase,pub clock:Clock,pub objects:Option<[usize;2]>,pub flags:u32,pub bodies:&'a [Body],#[cfg(test)] pub contacts:&'a crate::ivp_managed_response::ResponseWorld}
pub(crate) struct Owner<'a>{
 discovery:Discovery<'a>,ownership:Ownership,cache:WorldCache,
 flags:BTreeMap<usize,u32>,core_objects:BTreeMap<usize,usize>,gravity:[f32;3],config:Config,face_epsilon:f32,psi_owner:usize,
}
impl<'a> Owner<'a>{
 #[cfg(test)]
 fn audit(&self,clock:Clock,phase:Phase,queue:&Queue)->Result<(),&'static str>{
  if OWNER_AUDIT.with(|a|a.borrow().is_some()){let row=serde_json::json!({"time":clock.time.to_bits(),"phase":format!("{phase:?}"),"queries":self.discovery.query_snapshot(&self.ownership,queue)?});OWNER_AUDIT.with(|a|a.borrow_mut().as_mut().unwrap().push(row));}Ok(())
 }
 /// Take already registered/discovered state without rebuilding contacts or
 /// resampling caches. Physical endpoint IDs and cache-object IDs are explicit.
 pub(crate) fn from_parts(discovery:Discovery<'a>,ownership:Ownership,mut cache:WorldCache,physical:&ContactWorld,gravity:[f32;3],config:Config,face_epsilon:f32)->Result<Self,&'static str>{
  if discovery.dt.to_bits()!=physical.clock().timestep().to_bits()||!ownership.controllers.is_empty()||ownership.queue.first().is_some()||!ownership.graph.fast().is_empty(){return Err("unsupported compact owner transfer boundary");}
  if gravity.into_iter().any(|v|!v.is_finite())||!face_epsilon.is_finite()||face_epsilon<0.||config.targets.iter().any(|v|!v.is_finite()||*v<0.)||[config.slow_gate,config.time_scale,config.gap].into_iter().any(|v|!v.is_finite()||v<0.){return Err("invalid compact owner configuration");}
  if cache.metadata().0.iter().any(|s|s.references!=0){return Err("compact owner transfer retains external cache leases");}
  let mut core_objects=BTreeMap::new();
  for (&id,o) in &discovery.objects{
   let (core,frame)=physical.object_binding(id as u64)?;
   if core!=o.core||cache.object_core(id)?!=core||core_objects.insert(core,id).is_some(){return Err("compact owner requires one registered object per physical core");}
   let (_,cached,_)=cache.object_inputs(id)?;
   if frame.shift.map(|v|v.to_array().map(f32::to_bits))!=cached.shift.map(|v|v.to_array().map(f32::to_bits))||frame.rotation.map(|v|v.map(f32::to_bits))!=cached.rotation.map(|v|v.map(f32::to_bits)){return Err("compact owner object frames differ");}
   cache.bind_contact_object(id as u64,id)?;
  }
  if core_objects.keys().copied().ne(0..physical.bodies().len())||discovery.locations.keys().any(|&id|id==physical.clock().prediction_owner()){return Err("compact owner bank or queue namespace differs");}
  discovery.try_clone_ordinary()?;ownership.try_clone_unbound()?;
  let flags=discovery.objects.iter().map(|(&id,o)|(id,physical.bodies()[o.core].object_flags)).collect();
  Ok(Self{discovery,ownership,cache,flags,core_objects,gravity,config,face_epsilon,psi_owner:physical.clock().prediction_owner()})
 }
 fn staged(&self)->Result<Self,&'static str>{
  if self.cache.metadata().0.iter().any(|s|s.references!=0){return Err("compact tick started with outstanding cache leases");}
  Ok(Self{discovery:self.discovery.snapshot_with_counters(&mut super::super::CounterSnapshot::default()),ownership:self.ownership.try_clone_unbound()?,cache:self.cache.clone(),flags:self.flags.clone(),core_objects:self.core_objects.clone(),gravity:self.gravity,config:self.config,face_epsilon:self.face_epsilon,psi_owner:self.psi_owner})
 }
 pub(crate) fn into_parts(self)->(Discovery<'a>,Ownership,WorldCache){(self.discovery,self.ownership,self.cache)}
 #[cfg(test)]
 pub(crate) fn snapshot(&self)->serde_json::Value{
  serde_json::json!({"cache_frames":self.core_objects.iter().map(|(&core,&object)|self.cache.object_frame(object).ok().map(|f|serde_json::json!({"core":core,"object":object,"frame":f.frame_stamp,"quaternion":f.quaternion.map(f32::to_bits),"matrix":f.pose.rotation.to_cols_array().map(f32::to_bits)}))).collect::<Vec<_>>(),"bounds":format!("{:?}",self.discovery.world),"ownership":format!("{:?}",self.ownership),"cache":format!("{:?}",self.cache),
   "contacts":self.discovery.coarse.iter().map(|(&id,c)|serde_json::json!({"broad":id,"entries":c.entries().iter().map(|e|serde_json::json!({"slots":e.key.slots,"ledges":e.key.ledges,"contact":format!("{:?}",e.contact)})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
   "locations":self.discovery.locations.iter().map(|(&id,l)|serde_json::json!([id,l.broad,l.key.slots,l.key.ledges])).collect::<Vec<_>>(),
   "objects":self.discovery.order.iter().map(|&id|serde_json::json!({"id":id,"motion":self.discovery.motion_snapshot(id),"far":self.discovery.far_snapshot(id)})).collect::<Vec<_>>(),
   "flags":self.flags,"core_objects":self.core_objects,"gravity":self.gravity.map(f32::to_bits),"targets":self.config.targets.map(f32::to_bits).to_vec(),"epsilon":self.face_epsilon.to_bits(),"psi_owner":self.psi_owner})
 }
 /// Public material edits share the native cache pool with future geometry.
 /// Commit both owners together so a rejected edit cannot leak cache leases.
 pub(crate) fn assign_material(&mut self,physical:&mut ContactWorld,object:u64,index:i32)->Result<(),&'static str>{
  let mut owner=self.staged()?;let mut world=physical.clone();if !world.prepare_material_assignment(object,index)?{return Ok(());}
  world.with_candidate_context(|context,clock,deadline,queue|owner.wake_endpoints([object as usize],context,clock,deadline,queue))?;
  world.refresh_material_assignment_cached(object,&mut owner.cache)?;*self=owner;*physical=world;Ok(())
 }
 pub(crate) fn advance(&mut self,physical:&mut ContactWorld,delta:f32)->Result<(),&'static str>{self.advance_observed(physical,delta,|_|Ok(()))}
 /// Only these two owners commit. Observer-owned side effects are external and
 /// must be staged by their caller if they need the same rollback guarantee.
 pub(crate) fn advance_observed(&mut self,physical:&mut ContactWorld,delta:f32,mut observe:impl FnMut(Event<'_>)->Result<(),&'static str>)->Result<(),&'static str>{
  if physical.clock().prediction_owner()!=self.psi_owner||physical.bodies().len()!=self.core_objects.len(){return Err("compact tick physical membership changed");}
  let next=RefCell::new(self.staged()?);let observer=RefCell::new(&mut observe);
  physical.advance_with_lifecycle_context(delta,|bodies,ids,clock|next.borrow_mut().prepare(bodies,ids,clock),|context,clock,deadline,queue|{
   let queued=context.pending_cores().into_iter().rev().map(|core|next.borrow().core_objects[&core]).collect::<Vec<_>>();
   next.borrow_mut().wake_endpoints(queued,context,clock,deadline,queue)?;
   let notices=context.take_object_notifications();next.borrow_mut().notify_objects(context,&notices,clock,deadline,queue)
  },|context,members,clock,_,queue|{
   let mut owner=next.borrow_mut();let Owner{discovery,ownership,cache,gravity,..}=&mut *owner;
   ownership.with_queue(queue,|ownership|discovery.refresh_inactive_unit(members,ownership,clock,context.bodies_mut()?,cache,*gravity))?;
   observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Inactive,clock,objects:None,flags:0})
  },|context,order,notices,clock,deadline,queue|{
   let mut owner=next.borrow_mut();observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Prediction,clock,objects:None,flags:0})?;
   owner.predict(context,order,notices,clock,deadline,queue)?;
   #[cfg(test)] owner.audit(clock,Phase::Prediction,queue)?;
   observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Predicted,clock,objects:None,flags:0})
  },|context,id,clock,deadline,queue|{
   let mut owner=next.borrow_mut();let objects=owner.ownership.graph.objects(id)?;
   #[cfg(test)] owner.audit(clock,Phase::Timed,queue)?;
   observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Timed,clock,objects:Some(objects),flags:owner.ownership.graph.flags(id)?})?;
   let out={let Owner{discovery,ownership,cache,gravity,config,face_epsilon,..}=&mut *owner;
    discovery.dispatch_ordinary_event(id,ownership,queue,clock,deadline,context.bodies_mut()?,cache,*gravity,*config,*face_epsilon)?};
   if let TimedEvent::Impact=out{
    let impact_flags=owner.ownership.graph.flags(id)?;
    observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Impact,clock,objects:Some(objects),flags:impact_flags})?;
    owner.wake_endpoints(objects,context,clock,deadline,queue)?;
    observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Woken,clock,objects:Some(objects),flags:impact_flags})?;
    let Owner{discovery,ownership,cache,gravity,config,face_epsilon,psi_owner,..}=&mut *owner;
    ownership.begin_collision_revision();let (entry,convexes,keys)=discovery.entry_mut(id)?;
    let admitted=entry.admit_impact(context,keys,convexes,cache,clock)?;
    if !context.has_impact_elasticities(admitted.key)?{return Err("compact impact material binding is missing");}
    context.primary_impact(admitted.key)?;
    let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,cache,clock)?;
    while wave.step_cached(context,cache,clock)?.is_some(){wave.continue_iteration()?;}
    let changed=wave.rebuild(context)?;
    let motion=changed.iter().map(|&id|context.prediction_motion(id).map(|m|(id,m))).collect::<Result<Vec<_>,_>>()?;
    let dt=discovery.dt;
    discovery.rebuild_bounds(&motion,clock,deadline,context.bodies_mut()?,cache,&mut Constructor{owner:ownership,queue,clock,deadline,dt,gravity:*gravity,config:*config,face_epsilon:*face_epsilon,psi_owner:*psi_owner})?;
    ownership.with_queue(queue,|owner|discovery.recheck_impact_pairs(&changed,owner,clock,deadline,context.bodies_mut()?,cache,*gravity,*config,*face_epsilon))?;
    #[cfg(test)] owner.audit(clock,Phase::Rebuilt,queue)?;
    // Bounds callbacks may retire the descriptor; retain its collision-entry flags for this diagnostic.
    observer.borrow_mut()(Event{#[cfg(test)] contacts:context.contacts_for_observer(),bodies:context.bodies()?,phase:Phase::Rebuilt,clock,objects:Some(objects),flags:impact_flags})?;
   }Ok(())
  })?;
  *self=next.into_inner();Ok(())
 }
 fn wake_endpoints(&mut self,objects:impl IntoIterator<Item=usize>,physical:&mut CollisionContext<'_>,clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
  let Self{discovery,ownership,cache,flags,gravity,config,face_epsilon,psi_owner,core_objects}=self;let dt=discovery.dt;
  for object in objects{
   let core=discovery.objects[&object].core;
   if physical.bodies()?[core].object_flags&255!=8{continue;}
   let awakened=physical.wake_impact_unit_with_admission(core,ownership,|physical,owner,waking|{
    let object=*core_objects.get(&waking).ok_or("waking compact core lacks object binding")?;
    let motion=discovery.cached_motion(object)?;
    discovery.notify_wake(object,clock,motion,physical.bodies_mut()?,cache,&mut Constructor{owner,queue,clock,deadline,dt,gravity:*gravity,config:*config,face_epsilon:*face_epsilon,psi_owner:*psi_owner})?;
    let mut created=false;
    for link in owner.graph.object_links(object)?.to_vec(){
     let ends=owner.graph.objects(link.pair)?;let other=discovery.objects[&ends[link.side^1]].core;
     if owner.graph.flags(link.pair)?&0x3000!=0||physical.bodies()?[other].flags&2!=0||physical.core_has_contact_system(other){continue;}
     let cores=ends.map(|o|discovery.objects[&o].core);let pair=cores.map(|c|physical.bodies().unwrap()[c]);
     let (entry,convexes,keys)=discovery.entry_mut(link.pair)?;
     let [a,b]=convexes.map(|c|c.topology.as_deref().ok_or("wake neighbour lacks topology"));
     entry.contact.refresh_geometry(cache,clock,&pair,[a?,b?],convexes.map(|c|c.triangle_headers.as_slice()))?;
     owner.graph.set_flags(link.pair,entry.contact.flags)?;
     if entry.contact.flags&0xc000==0&&entry.contact.query.geometry.ok_or("waking compact contact lacks geometry")?.distance<f32::from_bits(1021967909){created|=entry.admit_wake_neighbour(physical,keys,convexes,cache,clock,waking,other)?;}
    }Ok(created)
   },|_,_,_,_|Ok(()))?;
   for core in awakened{flags.insert(core_objects[&core],physical.bodies()?[core].object_flags);}
  }Ok(())
 }
 fn prepare(&mut self,bodies:&[Body],ids:&[usize],clock:Clock)->Result<(),&'static str>{
  for &core in ids{let object=*self.core_objects.get(&core).ok_or("unregistered compact cache core")?;self.cache.update_core(core,bodies[core].prediction)?;self.cache.set_flags(object,bodies[core].object_flags)?;self.cache.access(object,clock)?;}Ok(())
 }
 fn notify_objects(&mut self,context:&mut CollisionContext<'_>,notices:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
  let Self{discovery,ownership,cache,gravity,config,face_epsilon,psi_owner,core_objects,flags} = self;
  let dt=discovery.dt;
  for &core in notices{
   let object=*core_objects.get(&core).ok_or("unregistered compact notification core")?;
   if flags.get(&object)==Some(&context.bodies()?[core].object_flags){continue;}
   let before=*flags.get(&object).ok_or("missing compact object flags")?;let after=context.bodies()?[core].object_flags;
   if before&!255==after&!255&&(1..=3).contains(&(before&255))&&(1..=3).contains(&(after&255)){
    discovery.sync_active_flags(object,after,cache)?;flags.insert(object,after);continue;
   }

   let mut constructor=Constructor{owner:ownership,queue,clock,deadline,dt,gravity:*gravity,config:*config,face_epsilon:*face_epsilon,psi_owner:*psi_owner};
   if context.bodies()?[core].object_flags&255==8{
    discovery.notify_sleep(object,clock,context.bodies_mut()?,cache,&mut constructor)?;
   }else{
    let motion=discovery.cached_motion(object)?;
    discovery.notify_wake(object,clock,motion,context.bodies_mut()?,cache,&mut constructor)?;
   }
   flags.insert(object,context.bodies()?[core].object_flags);
  }
  Ok(())
 }
 fn predict(&mut self,context:&mut CollisionContext<'_>,order:&[usize],notices:&[usize],clock:Clock,deadline:f64,queue:&mut Queue)->Result<(),&'static str>{
  self.notify_objects(context,notices,clock,deadline,queue)?;
  let Self{discovery,ownership,cache,gravity,config,face_epsilon,psi_owner,core_objects,flags}=self;let dt=discovery.dt;
  if core_objects.iter().any(|(&core,&object)|flags.get(&object)!=Some(&context.bodies().unwrap()[core].object_flags)){return Err("compact object flag transition lacks a notification");}
  let motion=order.iter().map(|&id|context.prediction_motion(id).map(|m|(id,m))).collect::<Result<Vec<_>,_>>()?;
  discovery.predict_bounds(&motion,clock,deadline,context.bodies_mut()?,cache,&mut Constructor{owner:ownership,queue,clock,deadline,dt,gravity:*gravity,config:*config,face_epsilon:*face_epsilon,psi_owner:*psi_owner})?;
  discovery.refresh_schedule(ownership,queue,clock,deadline,context.bodies_mut()?,cache,*gravity,*config,*face_epsilon)
 }
}

struct Constructor<'a>{owner:&'a mut Ownership,queue:&'a mut Queue,clock:Clock,deadline:f64,dt:f32,gravity:[f32;3],config:Config,face_epsilon:f32,psi_owner:usize}
impl Constructor<'_>{
 fn initialize(&mut self,id:usize,entry:&mut Entry<'_>,convexes:[&Convex;2],budgets:[crate::ivp_hull_budget::Budget;2],properties:[Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Option<FarRegistration>,&'static str>{
  if entry.recursive.is_some()||entry.contact.flags&0x3000!=0||id==self.psi_owner{return Err("unsupported compact constructor or reserved queue identity");}
  self.owner.bind_objects(entry.key.slots,bodies.map(|b|b.id))?;
  let timing=Schedule{deadline:self.deadline,dt:self.dt,delay:0,config:self.config,face_epsilon:self.face_epsilon,properties,budgets:std::array::from_fn(|i|crate::ivp_distance_schedule::Budget{time:budgets[i].time,flags:bodies[i].object_flags,rates:budgets[i].rates,accumulated:budgets[i].accumulated}),allow_far:false};
  self.owner.with_queue(self.queue,|owner|{
   let (_,result)=entry.initialize_registered(&mut owner.graph,id,[false;2],cache,self.clock,bodies,convexes,self.gravity,&mut owner.queue,&mut owner.counter,timing,|entry,graph,queue|{
    if queue.scheduled(id).is_some(){queue.cancel(id)?;}graph.make_inactive(id)?;entry.contact.flags=graph.flags(id)?;Ok(())
   })?;
   Ok(match result{Registration::Scheduled(out)=>out.far,Registration::InvalidGeometry=>None})
  })
 }
}
impl Lifecycle for Constructor<'_>{
 fn created(&mut self,entry:&mut Entry<'_>,convexes:[&Convex;2],budgets:[crate::ivp_hull_budget::Budget;2],motion:[Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Created,&'static str>{
  let id=self.owner.graph.register(entry.key.slots)?;
  if self.queue.scheduled(id).is_some(){return Err("new compact contact collides with another world event identity");}
  let far=self.initialize(id,entry,convexes,budgets,motion,bodies,cache)?;Ok(Created{id,far})
 }
 fn reactivated(&mut self,id:usize,entry:&mut Entry<'_>,convexes:[&Convex;2],budgets:[crate::ivp_hull_budget::Budget;2],motion:[Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Option<FarRegistration>,&'static str>{
  if self.queue.scheduled(id).is_some(){return Err("far compact contact unexpectedly owns a timed event");}
  self.initialize(id,entry,convexes,budgets,motion,bodies,cache)
 }
 fn retired(&mut self,id:usize,_:&mut Entry<'_>)->Result<(),&'static str>{self.owner.with_queue(self.queue,|owner|owner.retire(id))}
}
