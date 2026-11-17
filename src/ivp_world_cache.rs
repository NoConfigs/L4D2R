//! Owned core/object registry over100e7250/1007bfa0 and retained cache lookups.
//! The environment supplies time, frame generations and object movement flags.
//! World membership transitions and the general scheduler remain separate.
use crate::ivp_cache_pool::{self as pool,Object,Slot};
use crate::ivp_motion::{Prediction,ObjectFrame,PreparedObjectCache};
#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
struct Binding {core:usize,frame:ObjectFrame}
#[derive(Clone,Copy)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct Context {pub time:f64,pub frame:u32}
#[derive(Clone,Copy,Debug)]
pub(crate) struct Preparation {pub object:usize,pub slot:usize,pub references:u32,pub previous_stamp:u32,pub frame:u32,pub time:f64}
/// A native cache reference, consumed exactly once by release. The frame is read
/// from its slot, so later preparation of that slot remains visible to leases.
pub(crate) struct Lease {slot:usize}
#[derive(Clone)]
#[cfg_attr(test,derive(Debug))]
pub(crate) struct WorldCache {
 contact_objects:std::collections::BTreeMap<u64,usize>,
 cores:Vec<Prediction>,bindings:Vec<Binding>,objects:Vec<Object>,slots:Vec<Slot>,cursor:usize,
 payloads:Vec<Option<PreparedObjectCache>>,
}
impl WorldCache {
 pub(crate) fn new(capacity:usize)->Result<Self,&'static str>{
  if !capacity.is_power_of_two(){return Err("invalid cache capacity");}
  Ok(Self{contact_objects:std::collections::BTreeMap::new(),cores:vec![],bindings:vec![],objects:vec![],slots:vec![Slot{references:0,frame_stamp:0,owner:None};capacity],cursor:0,payloads:vec![None;capacity]})
 }
 pub(crate) fn register_core(&mut self,prediction:Prediction)->usize{let id=self.cores.len();self.cores.push(prediction);id}
 pub(crate) fn update_core(&mut self,id:usize,prediction:Prediction)->Result<(),&'static str>{*self.cores.get_mut(id).ok_or("unknown cache core")?=prediction;Ok(())}
 ///100f1480 rebases cores selected by simulation-unit membership. Sleeping
 /// and fixed cores must not be inferred from this registry's complete list.
 /// Cached payloads/stamps remain intact; the next normal lookup decides
 /// whether the new environment serial requires preparation.
 pub(crate) fn rebase_core_times(&mut self,selected:&[usize],shift:f64)->Result<Vec<usize>,&'static str>{
  if !shift.is_finite(){return Err("nonfinite core time shift");}
  let mut ids=std::collections::BTreeSet::new();
  for &id in selected{if id>=self.cores.len()||!ids.insert(id){return Err("invalid or duplicate reset core");}}
  for &id in selected{self.cores[id].time-=shift;}
  //100eadc0 preserves the selected core traversal;100f1480 visits each
  // core's attached-object vector backwards. Registration appends bindings.
  Ok(selected.iter().flat_map(|&core|self.bindings.iter().enumerate().rev()
      .filter_map(move |(object,b)|(b.core==core).then_some(object))).collect())
 }
 pub(crate) fn register_object(&mut self,core:usize,flags:u32,frame:ObjectFrame)->Result<usize,&'static str>{
  if core>=self.cores.len(){return Err("unknown cache core");}
  let id=self.objects.len();self.bindings.push(Binding{core,frame});self.objects.push(Object{flags,cache:None});Ok(id)
 }
 pub(crate) fn set_flags(&mut self,id:usize,flags:u32)->Result<(),&'static str>{self.objects.get_mut(id).ok_or("unknown cache object")?.flags=flags;Ok(())}
 pub(crate) fn detach(&mut self,object:usize)->Result<(),&'static str>{pool::detach(&mut self.slots,&mut self.objects,object).map_err(|_|"cache detach failed")}
 pub(crate) fn metadata(&self)->(&[Slot],&[Object],usize){(&self.slots,&self.objects,self.cursor)}
 pub(crate) fn object_core(&self,object:usize)->Result<usize,&'static str>{Ok(self.bindings.get(object).ok_or("unknown cache object")?.core)}
 /// Physical endpoint identities are distinct from core IDs and cache indices.
 pub(crate) fn bind_contact_object(&mut self,key:u64,object:usize)->Result<(),&'static str>{
  self.object_core(object)?;
  if self.contact_objects.get(&key).is_some_and(|&old|old!=object){return Err("physical object cache binding changed");}
  self.contact_objects.insert(key,object);Ok(())
 }
 pub(crate) fn contact_object(&self,key:u64,core:usize)->Result<usize,&'static str>{
  let object=if let Some(&object)=self.contact_objects.get(&key){object}else{
   // The centred-sphere adapter has exactly one object per core, registered
   // in core order. Multi-object registries must provide explicit bindings.
   if self.bindings.len()!=self.cores.len()||self.bindings.iter().enumerate().any(|(i,b)|b.core!=i){return Err("physical object lacks explicit cache binding");}core
  };
  if self.object_core(object)?!=core{return Err("physical object cache/core binding differs");}Ok(object)
 }
 pub(crate) fn get(&self,slot:usize)->Result<PreparedObjectCache,&'static str>{self.payloads.get(slot).and_then(|v|*v).ok_or("cache slot has no initialized payload")}
 pub(crate) fn object_frame(&self,object:usize)->Result<PreparedObjectCache,&'static str>{self.get(self.objects.get(object).ok_or("unknown cache object")?.cache.ok_or("object has no cache")?)}
 pub(crate) fn lease_frame(&self,lease:&Lease)->Result<PreparedObjectCache,&'static str>{self.get(lease.slot)}
 /// Current predictor inputs are separate from the retained prepared frame.
 /// Geometry admission needs both; reading these must not refresh cache slots.
 pub(crate) fn object_inputs(&self,object:usize)->Result<(Prediction,ObjectFrame,u32),&'static str>{
  let binding=*self.bindings.get(object).ok_or("unknown cache object")?;let flags=self.objects[object].flags;
  let frame=ObjectFrame{shift:if flags&0x800==0{Some(binding.frame.shift.unwrap_or(bevy::prelude::Vec3::ZERO))}else{None},rotation:binding.frame.rotation};
  Ok((self.cores[binding.core],frame,flags))
 }
 fn query(&mut self,object:usize,context:Context,retained:bool,mut note:impl FnMut(Preparation))->Result<usize,&'static str>{
  if !context.time.is_finite(){return Err("nonfinite cache clock");}
  let (prediction,frame,_)=self.object_inputs(object)?;
  let payloads=&mut self.payloads;let mut failure=None;
  let mut prepare=|slot,refs,previous_stamp|{
   note(Preparation{object,slot,references:refs,previous_stamp,frame:context.frame,time:context.time});
   let value=prediction.prepare_object_cache(context.time,frame,context.frame);
   if !value.pose.position.is_finite()||!value.pose.rotation.is_finite()||!value.core_position.is_finite()||value.quaternion.iter().any(|v|!v.is_finite()){failure=Some("nonfinite prepared cache frame");}
   else{payloads[slot]=Some(value);}
  };
  let slot=if retained{pool::retain(&mut self.slots,&mut self.objects,&mut self.cursor,object,context.frame,&mut prepare)}else{pool::access(&mut self.slots,&mut self.objects,&mut self.cursor,object,context.frame,&mut prepare)}.map_err(|_|"cache lookup failed")?;
  if let Some(error)=failure{if retained{pool::release(&mut self.slots,slot).map_err(|_|"cache rollback failed")?;}return Err(error);}
  // Never invent values for native uninitialized-memory cases. In normal
  // environments the first dynamic lookup has a positive frame generation.
  if self.payloads[slot].is_none(){if retained{pool::release(&mut self.slots,slot).map_err(|_|"cache rollback failed")?;}return Err("cache lookup returned uninitialized payload");}
  Ok(slot)
 }
 pub(crate) fn access(&mut self,object:usize,context:Context)->Result<usize,&'static str>{self.access_observed(object,context,|_|{})}
 pub(crate) fn access_observed(&mut self,object:usize,context:Context,note:impl FnMut(Preparation))->Result<usize,&'static str>{self.query(object,context,false,note)}
 pub(crate) fn retain(&mut self,object:usize,context:Context)->Result<Lease,&'static str>{self.retain_observed(object,context,|_|{})}
 pub(crate) fn retain_observed(&mut self,object:usize,context:Context,note:impl FnMut(Preparation))->Result<Lease,&'static str>{Ok(Lease{slot:self.query(object,context,true,note)?})}
 pub(crate) fn retain_pair(&mut self,objects:[usize;2],context:Context)->Result<(Lease,Lease),&'static str>{
  let left=self.retain(objects[0],context)?;
  match self.retain(objects[1],context){Ok(right)=>Ok((left,right)),Err(error)=>{self.release(left)?;Err(error)}}
 }
 /// Release at the geometry callback, in native left/right order. An early
 /// source error still releases both references instead of leaking the pool.
 pub(crate) fn with_leases<T>(&mut self,leases:(Lease,Lease),operation:impl FnOnce(&mut dyn FnMut())->Result<T,&'static str>)->Result<T,&'static str>{
  let mut outstanding=Some(leases);let mut failure=None;
  let result={let mut release=||{if let Some((left,right))=outstanding.take(){let a=self.release(left);let b=self.release(right);if let Err(error)=a.and(b){failure=Some(error);}}};let result=operation(&mut release);release();result};
  if let Some(error)=failure{return Err(error);}result
 }
 pub(crate) fn release(&mut self,lease:Lease)->Result<(),&'static str>{
  if self.slots.get(lease.slot).is_none_or(|s|s.references==0){return Err("cache reference is not retained");}
  pool::release(&mut self.slots,lease.slot).map_err(|_|"cache release failed")
 }
}

#[cfg(test)]
mod contact_binding_tests{
 use super::*;
 use bevy::prelude::Vec3;
 fn prediction(x:f32)->Prediction{Prediction{start:Vec3::new(x,0.,0.),velocity:Vec3::ZERO,rotation_start:[0.,0.,0.,1.],rotation_end:[0.,0.,0.,1.],time:0.,inverse_dt:60.}}
 #[test]
 fn physical_endpoint_bindings_select_frames_when_cache_and_core_orders_differ(){
  let mut cache=WorldCache::new(8).unwrap();cache.register_core(prediction(10.));cache.register_core(prediction(20.));
  let frame=ObjectFrame{shift:None,rotation:None};
  cache.register_object(1,0x801,frame).unwrap();cache.register_object(0,0x801,frame).unwrap();
  assert!(cache.contact_object(100,0).is_err());
  cache.bind_contact_object(100,1).unwrap();cache.bind_contact_object(200,0).unwrap();
  assert!(cache.bind_contact_object(100,0).is_err());assert!(cache.contact_object(100,1).is_err());
  for (key,core,x) in [(100,0,10.),(200,1,20.)]{
   let object=cache.contact_object(key,core).unwrap();let slot=cache.access(object,Context{time:0.,frame:1}).unwrap();assert_eq!(cache.get(slot).unwrap().pose.position.x,x);
  }
 }
}
