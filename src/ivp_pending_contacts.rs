//!1010bf00 pending query, controller notification and predicted hull admission.
//! Controller thresholds/listener delivery are explicit object-provider hooks.
use super::*;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum PsiOutcome {Active,Tracked}
pub(crate) enum Outcome {Scheduled(crate::ivp_distance_schedule::queue::Outcome),Tracked}
impl Entry<'_>{
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn initialize_pending_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,fast_cores:[bool;2],
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],
  mut timing:crate::ivp_minimizer::owner::Schedule,controllers:[Option<f32>;2],
  mut notify:impl FnMut(usize,bool)->Result<(),&'static str>,linear_delta:f32,
 )->Result<(crate::ivp_minimizer::owner::Update,Outcome),&'static str>{
  if owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags{return Err("pending contact/graph binding differs");}
  owner.bind_objects(self.key.slots,bodies.map(|b|b.id))?;
  if self.contact.flags&0x3000!=0x1000||owner.graph.active().contains(&id)||owner.far(id).is_some()||owner.queue.scheduled(id).is_some(){return Err("pending initialization requires detached pending contact");}
  if !linear_delta.is_finite()||!controllers.into_iter().flatten().all(f32::is_finite){return Err("invalid pending controller input");}
  let hulls=convexes.map(|c|c.topology.as_deref().ok_or("pending contact requires native topology"));let hulls=[hulls[0]?,hulls[1]?];let headers=convexes.map(|c|c.triangle_headers.as_slice());
  let update=self.contact.refresh_pending(cache,clock,bodies,hulls,headers)?;
  let ordinary=self.contact.flags&0xc000==0;
  let distance=if ordinary{self.contact.query.geometry.ok_or("ordinary pending query has no geometry")?.distance}else{0.};
  if ordinary&&distance>0.{
   if self.contact.flags&0xc00!=0{
    self.contact.flags&=0xfffcf3ff;
    for side in 0..2{if controllers[side].is_some(){notify(self.key.slots[side],false)?;}}
   }
   owner.graph.set_flags(id,self.contact.flags)?;owner.graph.activate(id,fast_cores)?;self.contact.flags=owner.graph.flags(id)?;
   timing.delay=2;timing.allow_far=true;
   let out=self.contact.schedule(cache,clock,bodies,hulls,&mut owner.queue,id,&mut owner.counter,timing)?;
   if out.far.is_some(){owner.graph.unlink(id)?;}owner.graph.set_flags(id,self.contact.flags)?;
   if let Some((keys,delta))=out.far{owner.track_far(id,keys,delta)?;}
   return Ok((update,Outcome::Scheduled(out)));
  }
  let mut allowance=if ordinary{f32::from_bits(distance.to_bits()^0x80000000)}else{self.contact.margin()};
  if self.contact.flags&0xc00==0{
   self.contact.flags=(self.contact.flags&0xfffdf7ff)|0x10400;
   for side in 0..2{if controllers[side].is_some(){notify(self.key.slots[side],true)?;}}
  }
  if let Some(threshold)=controllers[0].or(controllers[1]){allowance=threshold+allowance;}
  self.track_pending_hulls(owner,id,bodies,timing.properties,allowance,linear_delta)?;Ok((update,Outcome::Tracked))
 }
 /// Ordinary compact pending branch of1010b6c0. Slot6 is1010af70 (return0);
 /// recursive class side effects require their own owner and are rejected.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_pending_psi_owned(
  &mut self,owner:&mut ownership::Ownership,id:usize,
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],dt:f32,gravity:[f32;3],
  properties:[crate::ivp_minimizer::owner::Properties;2],controllers:[Option<f32>;2],
  mut notify:impl FnMut(usize,bool)->Result<(),&'static str>,linear_delta:f32,
 )->Result<(crate::ivp_minimizer::owner::Update,PsiOutcome),&'static str>{
  if self.recursive.is_some()||self.contact.flags&0x3000!=0x1000||owner.graph.objects(id)?!=self.key.slots||owner.graph.flags(id)?!=self.contact.flags||!owner.graph.active().contains(&id){return Err("pending PSI requires matching active compact contact");}
  if controllers==[None,None]||!controllers.into_iter().flatten().all(f32::is_finite)||!linear_delta.is_finite(){return Err("pending PSI requires a finite controller threshold and retained delta");}
  let update=self.refresh(cache,clock,bodies,convexes,dt,gravity)?;
  let ordinary=self.contact.flags&0xc000==0;
  let distance=if ordinary{self.contact.query.geometry.ok_or("pending PSI has no geometry")?.distance}else{0.};
  if ordinary&&distance>0.{
   if self.contact.flags&0xc00!=0{
    self.contact.flags&=0xfffcf3ff;
    for side in 0..2{if controllers[side].is_some(){notify(self.key.slots[side],false)?;}}
   }
   owner.graph.set_flags(id,self.contact.flags)?;return Ok((update,PsiOutcome::Active));
  }
  // Two-controller pairs remain active even for failed/penetrating geometry.
  // A sole controller also stays active when membership was already installed.
  if controllers.iter().all(Option::is_some)||self.contact.flags&0xc00!=0{
   owner.graph.set_flags(id,self.contact.flags)?;return Ok((update,PsiOutcome::Active));
  }
  let allowance=controllers[0].or(controllers[1]).unwrap()+if ordinary{f32::from_bits(distance.to_bits()^0x80000000)}else{self.contact.margin()};
  self.contact.flags=(self.contact.flags&0xfffdf7ff)|0x10400;
  for side in 0..2{if controllers[side].is_some(){notify(self.key.slots[side],true)?;}}
  owner.graph.set_flags(id,self.contact.flags)?;
  if owner.queue.scheduled(id).is_some(){owner.queue.cancel(id)?;}
  owner.graph.unlink(id)?;self.contact.flags=owner.graph.flags(id)?;
  self.track_pending_hulls(owner,id,bodies,properties,allowance,linear_delta)?;
  Ok((update,PsiOutcome::Tracked))
 }
 /// Bind the PSI transition to retained controller membership, instead of
 /// requiring the caller to maintain an independent shadow membership set.
 #[allow(clippy::too_many_arguments)]
 pub(crate) fn refresh_pending_psi_registered(
  &mut self,owner:&mut ownership::Ownership,id:usize,
  cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context,
  bodies:&mut[crate::ivp_joint_apply::Body;2],convexes:[&Convex;2],dt:f32,gravity:[f32;3],
  properties:[crate::ivp_minimizer::owner::Properties;2],
  mut notify:impl FnMut(usize,&ownership::controllers::Controller,ownership::controllers::Notice)->Result<(),&'static str>,linear_delta:f32,
 )->Result<(crate::ivp_minimizer::owner::Update,PsiOutcome),&'static str>{
  owner.sync_controller_bodies(&self.key.slots,bodies)?;
  let mut bank=std::mem::take(&mut owner.controllers);
  let slots=self.key.slots;let result=(||{for side in 0..2{bank.notifications.bind(slots[side],bodies[side].object_flags)?;}let thresholds=slots.map(|o|bank.get(o).map(|c|c.threshold));
  self.refresh_pending_psi_owned(owner,id,cache,clock,bodies,convexes,dt,gravity,properties,thresholds,|object,add|bank.change(object,id,slots,add,|state,event|notify(object,state,event)),linear_delta)})();
  owner.controllers=bank;owner.commit_controller_body_flags(&self.key.slots,bodies)?;result
 }
 fn track_pending_hulls(&mut self,owner:&mut ownership::Ownership,id:usize,bodies:&[crate::ivp_joint_apply::Body;2],properties:[crate::ivp_minimizer::owner::Properties;2],allowance:f32,linear_delta:f32)->Result<(),&'static str>{
  let predicted=self.key.slots.map(|j|owner.hulls[j].budget.predicted);
  let motion=std::array::from_fn(|j|[crate::ivp_bounds::speed(Vec3::from_array(bodies[j].linear[..3].try_into().unwrap())),properties[j].angular_bound]);
  let keys=crate::ivp_distance_schedule::auxiliary_hull_keys(predicted,bodies.map(|b|b.object_flags),motion,allowance)?;
  self.contact.flags=(self.contact.flags&0xffd7ffff)|0x140000;owner.graph.set_flags(id,self.contact.flags)?;
  owner.track_far(id,keys,linear_delta)
 }
}
