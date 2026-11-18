//! Registered preparation/base/joint/contact/poll/prediction composition.
//! Candidate discovery, cache-resource lifetimes and playable deployment are
//! separate. This owner consumes FreeWorld: bodies and registry are not mirrored.
use bevy::prelude::*;
use std::collections::{BTreeMap,BTreeSet};
#[cfg(test)]
thread_local!{pub(crate) static PRIMARY_AUDIT:std::cell::RefCell<Option<Vec<serde_json::Value>>>=const{std::cell::RefCell::new(None)};}
use crate::{ivp_managed_response::{ResponseWorld,Object as ContactObject,Settings,Feature},ivp_motion::ObjectFrame,
 ivp_registered_physical::{Properties,ActiveGroup},ivp_group_members::{Members,Object},
 ivp_unit_registry::{Registry,ControllerKey,Effect},ivp_joint_apply::Body,
 ivp_movement::{self as movement,History,Poll,Input}};
#[derive(Clone)]
pub(crate) struct Definition{pub objects:BTreeMap<u64,ContactObject>,pub frames:BTreeMap<u64,ObjectFrame>,pub settings:Settings,
 pub decay:f32,pub initial_work:f32,pub blocked:Vec<bool>,pub dissipation:crate::ivp_dissipation::Settings,pub allowance:f32,pub energy_bound:f32}
pub(crate) struct Parts{pub registry:Registry,pub clock:crate::ivp_event_world::World,pub poll:Poll,pub dt:f32,pub gravity:Vec3,pub alternate:Vec3,pub rescue_gravity:Option<Vec3>,
 pub objects:Vec<Object>,pub bodies:BTreeMap<usize,Body>,pub histories:BTreeMap<usize,History>,pub properties:BTreeMap<usize,Properties>,pub joints:BTreeMap<usize,ActiveGroup>,
 pub object_offsets:BTreeMap<usize,Option<[f32;3]>>,pub retired_constraints:BTreeSet<usize>,pub detached_constraints:BTreeMap<usize,[usize;2]>,pub object_joint_listeners:BTreeMap<usize,BTreeSet<usize>>,pub prediction_calls:Vec<usize>,pub metadata:BTreeMap<usize,Members>}
#[derive(Clone)]
pub(crate) struct ContactWorld{clock:crate::ivp_event_world::World,state:State}
#[derive(Clone)]
struct State{prediction_motion:BTreeMap<usize,crate::ivp_motion::PredictionMotion>,response:ResponseWorld,poll:Poll,dt:f32,gravity:Vec3,alternate:Vec3,rescue_gravity:Option<Vec3>,objects:Vec<Object>,histories:BTreeMap<usize,History>,properties:BTreeMap<usize,Properties>,object_notifications:Vec<usize>,
 joints:BTreeMap<usize,ActiveGroup>,metadata:BTreeMap<usize,Members>,frames:BTreeMap<u64,ObjectFrame>,contact_cores:BTreeMap<u64,usize>,
 spheres:BTreeMap<usize,f32>,elasticities:BTreeMap<u64,f32>,impact_counts:BTreeMap<usize,u16>,object_offsets:BTreeMap<usize,Option<[f32;3]>>,retired_constraints:BTreeSet<usize>,detached_constraints:BTreeMap<usize,[usize;2]>,object_joint_listeners:BTreeMap<usize,BTreeSet<usize>>,prediction_calls:Vec<usize>,released:Vec<usize>,released_features:Vec<[crate::ivp_create::Endpoint;2]>,#[cfg(any(test,debug_assertions))] calls:Vec<ControllerKey>}
/// Scoped access to one collision's canonical world candidate. No body copy.
pub(crate) struct CollisionContext<'a>{state:&'a mut State,now:f64,deadline:f64}
pub(crate) struct PrimaryImpact{pub kernel:crate::ivp_impact::Outcome,pub deferred_core:Option<usize>}
pub(crate) struct AdmittedContact{pub key:u64,pub saved:Vec<crate::ivp_impact_state::Saved>}
pub(crate) type AdmittedSphere=AdmittedContact;
impl CollisionContext<'_>{
 pub(crate) fn take_object_notifications(&mut self)->Vec<usize>{std::mem::take(&mut self.state.object_notifications)}
 pub(crate) fn pending_cores(&self)->Vec<usize>{self.state.response.registry().pending_cores().to_vec()}

 #[cfg(test)]
 pub(crate) fn contacts_for_observer(&self)->&ResponseWorld{&self.state.response}
 pub(crate) fn current_time(&self)->f64{self.now}
 pub(crate) fn prediction_motion(&self,id:usize)->Result<crate::ivp_motion::PredictionMotion,&'static str>{
  self.state.prediction_motion.get(&id).copied().filter(|m|m.time.to_bits()==self.now.to_bits()).ok_or("core has no prediction motion at this event time")
 }
 pub(crate) fn bodies(&self)->Result<&[Body],&'static str>{self.state.response.physical_bodies()}
 pub(crate) fn bodies_mut(&mut self)->Result<&mut [Body],&'static str>{self.state.response.physical_bodies_mut()}
 pub(crate) fn contacts(&self)->&ResponseWorld{&self.state.response}
 pub(crate) fn histories(&self)->&BTreeMap<usize,History>{&self.state.histories}
 pub(crate) fn impact_groups(&self,key:u64)->Result<(usize,Vec<crate::ivp_contact_system::Pair>,usize),&'static str>{self.state.response.impact_groups(key)}
 pub(crate) fn refresh_impact_contacts(&mut self,keys:&[usize])->Result<Vec<usize>,&'static str>{let retired=self.state.response.refresh_impact_contacts(keys)?;self.state.effects(self.now,self.deadline)?;Ok(retired)}
 pub(crate) fn refresh_impact_contacts_cached(&mut self,keys:&[usize],cache:&mut crate::ivp_world_cache::WorldCache,clock:crate::ivp_world_cache::Context)->Result<Vec<usize>,&'static str>{
  if clock.time.to_bits()!=self.now.to_bits(){return Err("impact cache clock mismatch");}
  let mut retired=vec![];
  for &key in keys.iter().rev(){
   let (cores,_,_,_)=self.state.response.impact_contact(key as u64)?;let objects=self.state.response.impact_objects(key as u64)?;let mut frames=vec![];
   for side in 0..2{let id=cores[side];let b=self.bodies()?[id];cache.update_core(id,b.prediction)?;let object=cache.contact_object(objects[side],id)?;cache.set_flags(object,b.object_flags)?;let slot=cache.access(object,clock)?;let f=cache.get(slot)?;let r=f.pose.rotation.transpose();frames.push((objects[side],[r.x_axis,r.y_axis,r.z_axis],f.pose.position));}
   self.state.response.set_selected_physical_context(self.now,&frames)?;
   retired.extend(self.state.response.refresh_impact_contacts(&[key])?);self.state.effects(self.now,self.deadline)?;
  }Ok(retired)
 }
 pub(crate) fn invalidate_impact_core(&mut self,system:usize,core:usize)->Result<(),&'static str>{self.state.response.invalidate_impact_core(system,core)}
 pub(crate) fn impact_score(&mut self,key:u64)->Result<f32,&'static str>{let (cores,_)=self.impact_endpoints(key)?;let radii=cores.map(|id|self.state.properties[&id].radius);self.state.response.impact_score(key,self.state.dt,radii)}
 pub(crate) fn stage_impact_core(&mut self,id:usize)->Result<crate::ivp_impact_state::Saved,&'static str>{let now=self.now;crate::ivp_impact_state::begin(&mut self.bodies_mut()?[id],now)?.ok_or("wave core needs a wake/held staging path")}
 pub(crate) fn impact_endpoints(&self,key:u64)->Result<([usize;2],[bool;2]),&'static str>{let(c,g,_,_)=self.state.response.impact_contact(key)?;Ok((c,g.active))}
 pub(crate) fn next_impact_push(&mut self,key:u64)->Result<(),&'static str>{self.state.response.next_impact_push(key)}
 pub(crate) fn impact_counts(&self)->&BTreeMap<usize,u16>{&self.state.impact_counts}
 ///10104320/1010f960 physical prediction phase. The impact environment stage
 /// disables gyro integration; pending impulses and the event pose stay intact.
 pub(crate) fn rebuild_impact_core(&mut self,id:usize)->Result<(),&'static str>{
  let remaining=(self.deadline-self.now) as f32;
  if !self.now.is_finite()||!remaining.is_finite()||remaining<0.{return Err("invalid impact prediction interval");}
  let p=*self.state.properties.get(&id).ok_or("missing impact prediction properties")?;
  let now=self.now;let b=self.bodies_mut()?.get_mut(id).ok_or("missing impact prediction body")?;
  if b.flags&0x12!=0{return Err("held/static impact prediction is not connected");}
  let result=b.prediction.advance(crate::ivp_motion::MotionStep{time:now,dt:remaining,velocity:Vec3::from_array(b.linear[..3].try_into().unwrap()),angular:Vec3::from_array(b.angular[..3].try_into().unwrap()),inertia:p.inertia,inverse_inertia:Vec3::from_array(b.inverse[..3].try_into().unwrap()),limits:p.limits,angular_limit_exact:p.angular_limit_exact,gyro:false});
  // Owned10104320 uses a finite fallback below its epsilon, including zero.
  b.prediction.inverse_dt=if remaining>f32::from_bits(0x2edbe6ff){remaining.recip()}else{f32::from_bits(0x501502f9)};
  b.linear[..3].copy_from_slice(&b.prediction.velocity.to_array());b.angular[..3].copy_from_slice(&result.angular.to_array());
  let motion=crate::ivp_motion::PredictionMotion{time:now,dt:remaining,delta:result.delta,rotation:b.current.rotation,inverse_dt:b.prediction.inverse_dt};
  self.state.prediction_motion.insert(id,motion);Ok(())
 }
 pub(crate) fn has_impact_elasticities(&self,key:u64)->Result<bool,&'static str>{let objects=self.state.response.impact_objects(key)?;let features=self.state.response.impact_feature_elasticities(key)?;Ok((0..2).all(|side|features[side].is_some()||self.state.elasticities.contains_key(&objects[side])))}
 ///100fdf50 admits a newly close neighbour before impact staging, then
 ///100f0460 resets that neighbour's movement clocks only for a new contact.
 pub(crate) fn core_has_contact_system(&self,id:usize)->bool{self.state.response.core_has_contact_system(id)}
 pub(crate) fn admit_wake_neighbour(&mut self,flags:u32,cores:[usize;2],cached:[crate::ivp_motion::PreparedObjectCache;2],waking:usize,other:usize)->Result<(),&'static str>{
  let mut objects=[0;2];
  for side in 0..2{let found=self.state.contact_cores.iter().filter_map(|(&o,&c)|(c==cores[side]).then_some(o)).collect::<Vec<_>>();if found.len()!=1{return Err("wake admission requires one object per core");}objects[side]=found[0];}
  let features=objects.map(|object|Feature{material_code:0,endpoint:crate::ivp_create::Endpoint{object,feature:crate::ivp_create::Feature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});
  self.admit_wake_features(flags,features,cached,waking,other).map(|_|())
 }
 ///10104010 consumes the retained mindist features for compact neighbours too.
 /// Return creation separately: only a new contact resets the neighbour clock
 /// and requires the caller to restart its dormant-unit scan.
 pub(crate) fn admit_wake_features(&mut self,flags:u32,features:[Feature;2],cached:[crate::ivp_motion::PreparedObjectCache;2],waking:usize,other:usize)->Result<bool,&'static str>{
  let objects=features.each_ref().map(|f|f.endpoint.object);
  let cores=objects.map(|o|self.state.contact_cores.get(&o).copied().ok_or("unknown wake feature object"));let cores=[cores[0]?,cores[1]?];
  if waking==other||!((cores[0]==waking&&cores[1]==other)||(cores[1]==waking&&cores[0]==other)){return Err("wake features do not bind the requested cores");}
  if !self.state.histories.contains_key(&other){return Err("missing wake neighbour history");}
  self.state.refresh_context(self.now)?;self.state.override_context_frames(self.now,objects,cached)?;
  let (_,created)=self.state.response.resolve_created(flags,features,Some(waking))?;self.state.effects(self.now,self.deadline)?;
  if created{let now=self.now;self.bodies_mut()?[other].stamps=[now;2];self.state.histories.get_mut(&other).ok_or("missing wake neighbour history")?.reset_clock(now)?;}
  Ok(created)
 }
 pub(crate) fn queue_wake(&mut self,id:usize)->Result<(),&'static str>{self.state.response.physical_wake_in_candidate(id)?;self.state.effects(self.now,self.deadline)}
 pub(crate) fn dormant_unit_members(&self,id:usize)->Result<Vec<usize>,&'static str>{let r=self.state.response.registry();r.unit_cores(id)?.iter().rev().copied().filter_map(|id|match (r.core_fixed(id),r.movement(id)){(Ok(false),Ok(m)) if m>=8=>Some(Ok(id)),(Ok(_),Ok(_))=>None,(Err(e),_)|(_,Err(e))=>Some(Err(e))}).collect()}
 pub(crate) fn wake_impact_core(&mut self,id:usize)->Result<Vec<usize>,&'static str>{let first=self.state.object_notifications.len();self.state.response.physical_wake_core_in_candidate(id)?;self.state.effects(self.now,self.deadline)?;Ok(self.state.object_notifications.split_off(first))}
 /// Compact owner participates at the immediate member-wake boundary,after
 /// physical effects and before the caller proceeds to bounds/prediction work.
 pub(crate) fn wake_impact_core_with_compact(&mut self,id:usize,owner:&mut crate::ivp_surface_admission::contacts::ownership::Ownership,notify:impl FnMut(usize,&crate::ivp_surface_admission::contacts::ownership::controllers::Controller,crate::ivp_surface_admission::contacts::ownership::controllers::Notice,&[Body])->Result<(),&'static str>)->Result<Vec<usize>,&'static str>{
  let notices=self.wake_impact_core(id)?;owner.notify_controller_cores(&notices,self.bodies_mut()?,notify)?;Ok(notices)
 }
 pub(crate) fn finish_impact_wake(&mut self,id:usize)->Result<(),&'static str>{self.state.response.physical_finish_wake_in_candidate(id)?;self.state.effects(self.now,self.deadline)}
 pub(crate) fn wake_impact_unit(&mut self,id:usize)->Result<Vec<usize>,&'static str>{
  if self.bodies()?.get(id).ok_or("unknown impact wake core")?.object_flags&0xff!=8{return Ok(vec![]);}
  let first=self.state.object_notifications.len();self.state.response.physical_wake_now_in_candidate(id)?;self.state.effects(self.now,self.deadline)?;
  Ok(self.state.object_notifications.split_off(first))
 }
 ///100eab20 wakes members individually in reverse unit order before moving
 /// the unit onto the active list. The entry core itself may already be awake.
 pub(crate) fn wake_impact_unit_with_compact(&mut self,id:usize,owner:&mut crate::ivp_surface_admission::contacts::ownership::Ownership,mut notify:impl FnMut(usize,&crate::ivp_surface_admission::contacts::ownership::controllers::Controller,crate::ivp_surface_admission::contacts::ownership::controllers::Notice,&[Body])->Result<(),&'static str>)->Result<Vec<usize>,&'static str>{
  let members=self.dormant_unit_members(id)?;let mut notices=vec![];
  for member in members{notices.extend(self.wake_impact_core_with_compact(member,owner,|id,c,e,b|notify(id,c,e,b))?);}
  self.finish_impact_wake(id)?;Ok(notices)
 }
 ///101073c0 performs bounds/neighbour work before publishing object callbacks.
 /// The provider owns discovery and the current compact mindist links. It must
 /// report a newly created contact separately from a retained-contact lookup.
 pub(crate) fn wake_impact_member_with_admission(&mut self,id:usize,owner:&mut crate::ivp_surface_admission::contacts::ownership::Ownership,mut admit:impl FnMut(&mut Self,&mut crate::ivp_surface_admission::contacts::ownership::Ownership,usize)->Result<bool,&'static str>,notify:impl FnMut(usize,&crate::ivp_surface_admission::contacts::ownership::controllers::Controller,crate::ivp_surface_admission::contacts::ownership::controllers::Notice,&[Body])->Result<(),&'static str>)->Result<(Vec<usize>,bool),&'static str>{
  let notices=self.wake_impact_core(id)?;if notices.is_empty(){return Ok((notices,false));}
  let created=admit(self,owner,id)?;
  owner.notify_controller_cores(&notices,self.bodies_mut()?,notify)?;Ok((notices,created))
 }
 ///100eab20 restarts its reverse member scan when admission merges units.
 pub(crate) fn wake_impact_unit_with_admission(&mut self,id:usize,owner:&mut crate::ivp_surface_admission::contacts::ownership::Ownership,mut admit:impl FnMut(&mut Self,&mut crate::ivp_surface_admission::contacts::ownership::Ownership,usize)->Result<bool,&'static str>,mut notify:impl FnMut(usize,&crate::ivp_surface_admission::contacts::ownership::controllers::Controller,crate::ivp_surface_admission::contacts::ownership::controllers::Notice,&[Body])->Result<(),&'static str>)->Result<Vec<usize>,&'static str>{
  let mut notices=vec![];
  loop{let members=self.dormant_unit_members(id)?;let mut restart=false;
   for member in members{let (emitted,created)=self.wake_impact_member_with_admission(member,owner,|c,o,m|admit(c,o,m),|i,c,e,b|notify(i,c,e,b))?;notices.extend(emitted);if created{restart=true;break;}}
   if !restart{break;}
  }
  self.finish_impact_wake(id)?;Ok(notices)
 }
 /// Primary10103940/10103020 response. Coupled propagation and hold/anomaly callbacks,
 /// changed-backup flags and prediction restamping are subsequent phases.
 pub(crate) fn primary_impact(&mut self,key:u64)->Result<PrimaryImpact,&'static str>{
  self.primary_impact_inner(key,None)
 }
 /// Neighbour pushes consume the rescue term prepared by10103c10, including
 /// compact rotation. The initial impact computes its own retained rotation term.
 pub(crate) fn primary_wave_impact(&mut self,key:u64)->Result<PrimaryImpact,&'static str>{
  let bias=self.state.response.wave_impact_bias(key)?;self.primary_impact_inner(key,Some(bias))
 }
 fn primary_impact_inner(&mut self,key:u64,wave_bias:Option<f32>)->Result<PrimaryImpact,&'static str>{
  let (cores,g,friction,pushes)=self.state.response.impact_contact(key)?;
  #[cfg(test)]
  let mut audit=if PRIMARY_AUDIT.with(|a|a.borrow().is_some()){
   Some(serde_json::json!({"key":key,"time":self.now.to_bits(),"cores":cores,"counts_before":(0..self.bodies()?.len()).map(|id|self.state.impact_counts[&id]).collect::<Vec<_>>(),"before":self.bodies()?.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),"normal":g.normal.to_array().map(f32::to_bits),"friction":friction.to_bits(),"pushes":pushes}))
  }else{None};
  let objects=self.state.response.impact_objects(key)?;let features=self.state.response.impact_feature_elasticities(key)?;
  let coefficients=std::array::from_fn::<_,2,_>(|side|features[side].or_else(||self.state.elasticities.get(&objects[side]).copied()).ok_or("immediate response lacks explicit object elasticity"));let coefficients=[coefficients[0]?,coefficients[1]?];
  let energy=crate::ivp_material::clamped_product(coefficients[0],coefficients[1]) as f32;
  let bodies=self.state.response.physical_bodies()?;
  let budget=|id:usize|->Result<crate::ivp_impact::ImpactBudget,&'static str>{Ok(crate::ivp_impact::ImpactBudget{count:*self.state.impact_counts.get(&id).ok_or("impact core lacks a constructor budget")?,flags:bodies[id].flags,limit:6})};
  let mut budgets=[budget(cores[0])?,budget(cores[1])?];
  let dt=self.state.dt;
  let bias=match wave_bias{Some(bias)=>bias,None=>self.state.response.initial_impact_bias(key,dt,cores.map(|id|self.state.properties[&id].radius))?};
  let rescue=(f32::from_bits(1011875418)+bias)*f32::from_bits(1067030938);
  let p=self.state.properties[&cores[0]];let q=self.state.properties[&cores[1]];
  if p.limits!=q.limits||p.angular_limit_exact.to_bits()!=q.angular_limit_exact.to_bits(){return Err("impact cores have different world caps");}
  let bodies=self.state.response.physical_bodies()?;
  let mut work=std::array::from_fn::<_,2,_>(|side|{let b=bodies[cores[side]];crate::ivp_impact::CoreBody{flags:b.flags,inverse_mass:b.inverse[3],inverse_inertia:Vec3::from_array([b.inverse[0],b.inverse[1],b.inverse[2]]),rotation:b.current.rotation,arm:g.arms[side],velocity:Vec3::from_array(std::array::from_fn(|i|b.linear[i]+b.pending_linear[i])),angular:Vec3::from_array(std::array::from_fn(|i|b.angular[i]+b.pending_angular[i]))}});
  let raw=std::array::from_fn::<_,2,_>(|side|{let b=bodies[cores[side]];let mut v=work[side];v.velocity=Vec3::from_array(b.linear[..3].try_into().unwrap());v.angular=Vec3::from_array(b.angular[..3].try_into().unwrap());v});
  let [a,b]=&mut work;let outcome=crate::ivp_impact::resolve_cores(a,b,g.normal,crate::ivp_impact::Settings{energy,friction,system_pushes:pushes as u32,rescue_speed:rescue,linear_limit:p.limits.x,angular_limit:p.limits.y,angular_limit_exact:p.angular_limit_exact}).ok_or("invalid primary impact response")?;
  let deferred=crate::ivp_impact::deferred_core(raw,work,g.normal,rescue);
  //10102be0 cancels the deferred core's increment. Its committed opposite
  // still runs10101a20, including the anomaly callback at count > limit.
  // Skipping that callback permits an extra push before both cores are held.
  if deferred.is_some(){
   for side in 0..2{if deferred!=Some(side){budgets[side].register(true);}}
  }else{for budget in &mut budgets{budget.register(true);}}
  // Default1008c670 accepts holding when no external game callback is bound.
  //10103020 transfers BOTH endpoints when either endpoint carries a decision.
  let held=budgets.iter().any(|b|b.flags&0xc0!=0);
  let bodies=self.state.response.physical_bodies_mut()?;
  for side in 0..2{let id=cores[side];let body=&mut bodies[id];
   body.pending_linear[..3].fill(0.);body.pending_angular[..3].fill(0.);
   if deferred==Some(side){
    body.pending_linear[..3].copy_from_slice(&(work[side].velocity-raw[side].velocity).to_array());body.pending_angular[..3].copy_from_slice(&(work[side].angular-raw[side].angular).to_array());
   }else{body.linear[..3].copy_from_slice(&work[side].velocity.to_array());body.angular[..3].copy_from_slice(&work[side].angular.to_array());}
   body.flags=budgets[side].flags;self.state.impact_counts.insert(id,budgets[side].count);
   if held{
    body.flags=(body.flags&!0xc0)|if body.flags&2==0{0x40}else{0};
    for k in 0..3{body.pending_linear[k]+=body.linear[k];body.pending_angular[k]+=body.angular[k];body.linear[k]=0.;body.angular[k]=0.;}
   }
  }
  #[cfg(test)]
  if let Some(mut row)=audit.take(){row["counts_after"]=serde_json::json!((0..self.bodies()?.len()).map(|id|self.state.impact_counts[&id]).collect::<Vec<_>>());row["after"]=serde_json::json!(self.bodies()?.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>());PRIMARY_AUDIT.with(|a|a.borrow_mut().as_mut().unwrap().push(row));}
  Ok(PrimaryImpact{kernel:outcome,deferred_core:if held{None}else{deferred.map(|side|cores[side])}})
 }
 pub(crate) fn admit_spheres(&mut self,flags:u32,cores:[usize;2],radii:[f32;2])->Result<AdmittedSphere,&'static str>{
  self.admit_spheres_with_frames(flags,cores,radii,None)
 }
 pub(crate) fn admit_spheres_with_frames(&mut self,flags:u32,cores:[usize;2],radii:[f32;2],cached:Option<[crate::ivp_motion::PreparedObjectCache;2]>)->Result<AdmittedSphere,&'static str>{
  if cores[0]==cores[1]||cores.iter().zip(radii).any(|(id,r)|self.state.spheres.get(id).map(|r|r.to_bits())!=Some(r.to_bits())){return Err("impact endpoints are not registered constructor spheres");}
  let mut objects=[0;2];
  for side in 0..2{
   let matches=self.state.contact_cores.iter().filter_map(|(&object,&core)|(core==cores[side]).then_some(object)).collect::<Vec<_>>();
   if matches.len()!=1{return Err("sphere impact requires one object per core");}objects[side]=matches[0];
   let frame=self.state.frames[&objects[side]];if frame.shift.is_some()||frame.rotation.is_some(){return Err("centred sphere impact has an object offset");}
  }
  let features=objects.map(|object|Feature{material_code:0,endpoint:crate::ivp_create::Endpoint{object,feature:crate::ivp_create::Feature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});
  self.admit_features_with_frames(flags,features,cached)
 }
 ///10104820 retains object geometry before staging100f04b0;101045e0 then
 /// admits those same feature identities. Compact contacts keep their actual
 /// point/edge/face data and object frames, including shifted poly objects.
 pub(crate) fn admit_features_with_frames(&mut self,flags:u32,features:[Feature;2],cached:Option<[crate::ivp_motion::PreparedObjectCache;2]>)->Result<AdmittedContact,&'static str>{
  let objects=features.each_ref().map(|f|f.endpoint.object);
  let cores=objects.map(|object|self.state.contact_cores.get(&object).copied().ok_or("unknown impact feature object"));let cores=[cores[0]?,cores[1]?];
  if cores[0]==cores[1]{return Err("impact features belong to the same physical core");}
  let bodies=self.bodies()?;let hint=cores[if (bodies[cores[0]].object_flags as u8 as i8)<8{1}else{0}];
  // Native10dcd0 retains object caches BEFORE100f04b0 trims q1. Sampling
  // again after trimming would interpolate that shortened rotation twice.
  self.state.refresh_context(self.now)?;
  if let Some(frames)=cached{
   // Event geometry already retained these object frames before wake/staging.
   // A wake changes predictor stamps, but must not replace this serial's frame.
   self.state.override_context_frames(self.now,objects,frames)?;
  }
  let bodies=self.state.response.physical_bodies_mut()?;let mut saved=vec![];
  for id in cores{if let Some(s)=crate::ivp_impact_state::begin(&mut bodies[id],self.now)?{saved.push(s);}}
  let key=self.state.response.resolve(flags,features,Some(hint))?.ok_or("impact event did not admit a contact")?;
  self.state.effects(self.now,self.deadline)?;Ok(AdmittedContact{key,saved})
 }
}
fn settings()->movement::Settings{movement::Settings{translation_squared:0.0001,rotation_squared:0.000025,far_translation_squared:0.1f32*0.1,far_rotation_squared:0.2f32*0.2,far_timeout:4.,one:1.,two:2.,angular_factor0:std::f32::consts::PI,angular_factor1:0.75}}
fn input(b:Body,p:Properties)->Input{Input{radius:p.radius,sleep_time:p.sleep_time,angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),position:b.prediction.start,start:b.prediction.rotation_start,end:b.prediction.rotation_end}}
fn core(b:Body)->crate::ivp_contact::Core{crate::ivp_contact::Core{inverse_mass:b.inverse[3],inverse_inertia:Vec3::new(b.inverse[0],b.inverse[1],b.inverse[2]),linear:Vec3::new(b.linear[0],b.linear[1],b.linear[2]),angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),pending_linear:Vec3::new(b.pending_linear[0],b.pending_linear[1],b.pending_linear[2]),pending_angular:Vec3::new(b.pending_angular[0],b.pending_angular[1],b.pending_angular[2])}}
fn write_core(b:&mut Body,c:crate::ivp_contact::Core){b.linear[..3].copy_from_slice(&c.linear.to_array());b.angular[..3].copy_from_slice(&c.angular.to_array());b.pending_linear[..3].copy_from_slice(&c.pending_linear.to_array());b.pending_angular[..3].copy_from_slice(&c.pending_angular.to_array());}
impl ContactWorld{
 /// Runtime defaults recovered from the owned retail environment. No fixture I/O.
 pub(crate) fn empty_native(dt:f32,seed:u32,gravity:Vec3,alternate:Vec3)->Result<Self,&'static str>{
  let epsilon=f32::from_bits(0x2edbe6ff);let area_epsilon=f64::from_bits(0x3c32725de0000000);let minimum=f32::from_bits(0x358637bd);
  let definition=Definition{objects:BTreeMap::new(),frames:BTreeMap::new(),settings:Settings{dt,admission:crate::ivp_managed_contacts::Settings{distance:f32::from_bits(0x3c4ffe5a),area_epsilon},refresh:crate::ivp_refresh::Settings{epsilon,bary_bias:f32::from_bits(0x00800000),area_epsilon,one:1.,opposite:-1.},threshold_extra:minimum,minimum_impulse:minimum,minimum_determinant_squared:f32::from_bits(0x1e3ce509)},decay:crate::ivp_dissipation::decay_for_timestep(dt),initial_work:0.,blocked:vec![],dissipation:crate::ivp_dissipation::Settings{epsilon,one:1.,opposite:-1.,two:2.,half:0.5,huge:10000.,small:0.0001,fraction:0.1},allowance:f32::from_bits(0x411d47ae),energy_bound:10.};
  crate::ivp_registered_physical::FreeWorld::with_forces(usize::MAX,&[],dt,seed,gravity,alternate)?.into_contacts(definition)
 }
 pub(crate) fn from_parts(p:Parts,d:Definition)->Result<Self,&'static str>{
  if p.retired_constraints.iter().copied().ne(p.detached_constraints.keys().copied())||p.detached_constraints.iter().any(|(id,ends)|ends[0]==ends[1]||ends.iter().any(|body|!p.bodies.contains_key(body)||!p.object_joint_listeners.get(body).is_some_and(|listeners|listeners.contains(id)))){return Err("invalid detached-wrapper transfer boundary");}
  if d.settings.dt.to_bits()!=p.dt.to_bits()||p.bodies.keys().copied().ne(0..p.bodies.len())||d.objects.len()!=d.frames.len()
   ||d.objects.keys().ne(d.frames.keys())||d.objects.values().any(|o|!p.bodies.contains_key(&o.core))
   ||d.frames.values().any(|f|f.shift.is_some_and(|s|!s.is_finite())||f.rotation.is_some_and(|q|q.iter().any(|v|!v.is_finite()))){return Err("invalid registered contact conversion boundary");}
  let energy=p.bodies.values().map(|b|crate::ivp_batch_normal::Physical{flags:b.flags,mass:b.mass,inertia:p.properties[&b.id].inertia}).collect();
  let contact_cores=d.objects.iter().map(|(&id,o)|(id,o.core)).collect();
  let response=ResponseWorld::new_physical(p.registry,p.bodies.into_values().collect(),d.objects,d.settings,p.clock.now)?
   .with_energy(energy,d.allowance,d.energy_bound)?.with_pair_work(d.decay,d.initial_work,d.blocked,d.dissipation)?;
  let state=State{prediction_motion:BTreeMap::new(),object_notifications:vec![],response,poll:p.poll,dt:p.dt,gravity:p.gravity,alternate:p.alternate,rescue_gravity:p.rescue_gravity,objects:p.objects,histories:p.histories,properties:p.properties,joints:p.joints,metadata:p.metadata,frames:d.frames,contact_cores,spheres:BTreeMap::new(),elasticities:BTreeMap::new(),impact_counts:BTreeMap::new(),object_offsets:p.object_offsets,retired_constraints:p.retired_constraints,detached_constraints:p.detached_constraints,object_joint_listeners:p.object_joint_listeners,prediction_calls:p.prediction_calls,released:vec![],released_features:vec![],#[cfg(any(test,debug_assertions))] calls:vec![]};
  Ok(Self{clock:p.clock,state})
 }
 pub(crate) fn bodies(&self)->&[Body]{self.state.response.physical_bodies().unwrap()}
 pub(crate) fn contacts(&self)->&ResponseWorld{&self.state.response}
 pub(crate) fn registry(&self)->&Registry{self.state.response.registry()}
 pub(crate) fn histories(&self)->&BTreeMap<usize,History>{&self.state.histories}
 pub(crate) fn object_binding(&self,object:u64)->Result<(usize,ObjectFrame),&'static str>{Ok((*self.state.contact_cores.get(&object).ok_or("unknown physical object")?,*self.state.frames.get(&object).ok_or("physical object lacks frame")?))}
 pub(crate) fn object_offsets(&self)->&BTreeMap<usize,Option<[f32;3]>>{&self.state.object_offsets}
 pub(crate) fn group(&self,id:usize)->Option<&Members>{self.state.metadata.get(&id)}
 pub(crate) fn joint_group(&self,id:usize)->Option<&ActiveGroup>{self.state.joints.get(&id)}
 pub(crate) fn clock(&self)->&crate::ivp_event_world::World{&self.clock}
 pub(crate) fn poll(&self)->Poll{self.state.poll}
 pub(crate) fn prediction_calls(&self)->&[usize]{&self.state.prediction_calls}
 #[cfg(any(test,debug_assertions))]
 pub(crate) fn controller_calls(&self)->&[ControllerKey]{&self.state.calls}
 pub(crate) fn take_releases(&mut self)->(Vec<usize>,Vec<[crate::ivp_create::Endpoint;2]>){(std::mem::take(&mut self.state.released),std::mem::take(&mut self.state.released_features))}
 /// Pending impulses and public velocity writes keep contact/joint ownership intact.
 pub(crate) fn apply_impulses(&mut self,commands:&[crate::ivp_joint_tail::Command])->Result<(),&'static str>{
  use crate::ivp_joint_tail::Command;
  for &command in commands{let(id,finite)=match command{Command::PointImpulse{body,point,impulse}=>(body,point.into_iter().chain(impulse).all(f32::is_finite)),Command::AngularPush{body,delta}=>(body,delta.into_iter().all(f32::is_finite))};if !finite||self.bodies().get(id).is_none(){return Err("invalid contact-world impulse");}}
  let mut next=self.clone();next.state.response.physical_impulses_in_candidate(commands)?;
  for body in next.state.response.physical_bodies()?{if !body.pending_linear.into_iter().chain(body.pending_angular).all(f32::is_finite){return Err("contact-world impulse overflow");}let history=next.state.histories.get_mut(&body.id).ok_or("missing contact-world impulse history")?;history.close_time=body.stamps[0];history.far_time=body.stamps[1];}
  *self=next;Ok(())
 }
 pub(crate) fn wake(&mut self,id:usize)->Result<(),&'static str>{
  let dormant=self.bodies().get(id).ok_or("unknown contact-world wake core")?.object_flags&0xff==8;
  let mut next=self.clone();next.state.response.physical_wake_in_candidate(id)?;
  if dormant{next.state.response.physical_bodies_mut()?[id].flags|=4;}else{next.state.effects(next.clock.now,next.clock.deadline)?;}
  *self=next;Ok(())
 }
 pub(crate) fn set_velocity(&mut self,id:usize,linear:Option<Vec3>,angular:Option<Vec3>)->Result<(),&'static str>{
  let b=self.bodies().get(id).ok_or("unknown contact-world public velocity core")?;if b.flags&2!=0{return Ok(());}
  let mut next=self.clone();next.wake(id)?;let limits=next.state.properties[&id].limits;
  crate::ivp_public_velocity::assign(&mut next.state.response.physical_bodies_mut()?[id],limits,linear,angular)?;
  *self=next;Ok(())
 }
 /// Append one ordinary constructor result without rebuilding contact history.
 fn append_created(&mut self,object_id:u64,binding:crate::ivp_sphere_constructor::Binding,offset:Option<[f32;3]>,radius:f32,friction:f32)->Result<usize,&'static str>{
  if object_id==0||self.state.frames.contains_key(&object_id)||!friction.is_finite()||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite())){return Err("invalid created contact identity/material/frame");}
  let (object,body,history,properties)=binding;let id=body.id;let frame=ObjectFrame{shift:offset.map(Vec3::from_array),rotation:None};
  let cache=body.prediction.prepare_object_cache(self.clock.now,frame,0);let rotation=cache.pose.rotation.transpose();let contact=ContactObject{core:id,rotation:[rotation.x_axis,rotation.y_axis,rotation.z_axis],translation:cache.pose.position,radius,material_flags:0,friction};
  let mut next=self.clone();next.state.response.append_physical_body_in_candidate(body,object_id,contact,properties.inertia,false)?;
  for metadata in next.state.metadata.values_mut(){metadata.register_object(object)?;}
  next.state.impact_counts.insert(id,0);next.state.objects.push(object);next.state.histories.insert(id,history);next.state.properties.insert(id,properties);next.state.object_offsets.insert(id,offset);next.state.frames.insert(object_id,frame);next.state.contact_cores.insert(object_id,id);*self=next;Ok(id)
 }
 pub(crate) fn create_sphere(&mut self,object_id:u64,parameters:crate::ivp_sphere_constructor::Parameters,friction:f32)->Result<usize,&'static str>{
  let id=self.bodies().len();let binding=crate::ivp_sphere_constructor::construct_at(id,parameters,self.state.dt,crate::ivp_group_effects::Context{now:self.clock.now,next:self.clock.deadline})?;let radius=binding.3.radius;
  let id=self.append_created(object_id,binding,None,radius,friction)?;self.state.spheres.insert(id,radius);Ok(id)
 }
 /// Explicit ordinary material elasticity; no coefficient is inferred from
 /// friction or from a body radius. Missing material input blocks response.
 pub(crate) fn configure_sphere_elasticity(&mut self,id:usize,coefficient:f32)->Result<(),&'static str>{
  if !self.state.spheres.contains_key(&id)||!coefficient.is_finite(){return Err("invalid ordinary sphere elasticity binding");}
  let objects=self.state.contact_cores.iter().filter_map(|(&object,&core)|(core==id).then_some(object)).collect::<Vec<_>>();
  if objects.len()!=1{return Err("sphere elasticity requires one object per core");}self.configure_object_elasticity(objects[0],coefficient)
 }
 /// Material elasticity belongs to an object endpoint, not its shape class or
 /// core. Separate objects sharing a core may carry different materials.
 pub(crate) fn configure_object_elasticity(&mut self,object:u64,coefficient:f32)->Result<(),&'static str>{
  if !self.state.contact_cores.contains_key(&object)||!coefficient.is_finite(){return Err("invalid object elasticity binding");}
  self.state.elasticities.insert(object,coefficient);Ok(())
 }
 /// Publish a value-owned ordinary registry/remap without rewriting retained
 /// contact friction. Subsequent impacts resolve elasticity against this table.
 pub(crate) fn set_feature_materials(&mut self,registry:&crate::ivp_surface_registry::Registry,manager:&crate::ivp_material_backend::Manager)->Result<(),&'static str>{
  let materials=crate::ivp_material_backend::ContactMaterials::from_registry(registry,manager)?;
  self.state.response.set_feature_materials(materials);Ok(())
 }
 /// Constructor binding for the backend surface identity used by code0 features.
 pub(crate) fn set_object_material(&mut self,object:u64,index:i32)->Result<(),&'static str>{self.state.response.set_object_material(object,index)}
 /// Public10094cf0: same identity is a no-op; a changed identity runs the
 /// native core/contact recheck instead of merely replacing coefficients.
 /// The containing compact owner stages this world and the borrowed cache together.
 pub(crate) fn prepare_material_assignment(&mut self,object:u64,index:i32)->Result<bool,&'static str>{
  let previous=self.state.response.object_material(object).ok_or("runtime material assignment requires a bound object identity")?;
  if previous==index{return Ok(false);}self.state.response.set_object_material(object,index)?;Ok(true)
 }
 /// Complete the recheck after the geometry owner has delivered any immediate
 /// per-member wake/admission callbacks. The owner stages the whole command.
 pub(crate) fn refresh_material_assignment_cached(&mut self,object:u64,cache:&mut crate::ivp_world_cache::WorldCache)->Result<(),&'static str>{
  let core=*self.state.contact_cores.get(&object).ok_or("unknown public material object")?;let mut next=self.clone();
  let fixed=next.bodies()[core].flags&2!=0;
  if !fixed{
   if next.bodies()[core].flags>>8>=8{next.state.response.physical_wake_now_in_candidate(core)?;}else{next.state.response.reset_material_unit_clocks(core)?;}
   next.state.effects(next.clock.now,next.clock.deadline)?;
   let b=&mut next.state.response.physical_bodies_mut()?[core];let old_flags=b.object_flags;b.object_flags=(b.object_flags&!255)|((b.flags>>8) as u32);if old_flags!=b.object_flags{next.state.object_notifications.push(core);}
  }else{
   let inverse_dt=next.state.dt.recip();let b=&mut next.state.response.physical_bodies_mut()?[core];
   for v in [&mut b.linear,&mut b.angular,&mut b.pending_linear,&mut b.pending_angular]{v[..3].fill(0.);}
   b.prediction.velocity=Vec3::ZERO;b.prediction.rotation_end=b.prediction.rotation_start;b.prediction.inverse_dt=inverse_dt;
  }
  next.state.refresh_context(next.clock.now)?;
  let contacts=next.state.response.material_recheck_contacts(object)?;
  for key in contacts{
   if fixed{next.state.response.mark_material_system_dirty(key)?;let ids=next.state.response.impact_objects(key)?;for id in ids{let other=next.state.contact_cores[&id];if other!=core&&next.bodies()[other].flags&2==0{next.state.response.physical_wake_in_candidate(other)?;}}}
   let objects=next.state.response.impact_objects(key)?;let mut cache_objects=[0;2];
   for side in 0..2{let id=next.state.contact_cores[&objects[side]];let b=next.bodies()[id];cache.update_core(id,b.prediction)?;let object=cache.contact_object(objects[side],id)?;cache.set_flags(object,b.object_flags)?;cache_objects[side]=object;}
   let clock=crate::ivp_world_cache::Context{time:next.clock.now,frame:next.clock.clock_serial};let leases=cache.retain_pair(cache_objects,clock)?;
   let frames=[cache.lease_frame(&leases.0)?,cache.lease_frame(&leases.1)?];let frames=std::array::from_fn::<_,2,_>(|side|{let f=frames[side];let r=f.pose.rotation.transpose();(objects[side],[r.x_axis,r.y_axis,r.z_axis],f.pose.position)});
   let result=cache.with_leases(leases,|_|{next.state.response.set_selected_physical_context(next.clock.now,&frames)?;next.state.response.rebuild_material_contact(key) });result?;
  }
  next.state.effects(next.clock.now,next.clock.deadline)?;*self=next;Ok(())
 }
 pub(crate) fn bind_shared_materials(&mut self,registry:&crate::ivp_material_backend::SharedSurfaceRegistry,table:&crate::ivp_material_backend::WorldMaterialTable)->Result<(),&'static str>{
  self.state.response.set_feature_materials(registry.contact_materials(table)?);Ok(())
 }
 pub(crate) fn bind_world_materials(&mut self,registry:&crate::ivp_surface_registry::Registry,table:&crate::ivp_material_backend::WorldMaterialTable)->Result<(),&'static str>{
  let materials=table.contact_materials(registry)?;self.state.response.set_feature_materials(materials);Ok(())
 }
 pub(crate) fn create_poly(&mut self,object_id:u64,parameters:crate::ivp_poly_constructor::Parameters,friction:f32)->Result<usize,&'static str>{
  let id=self.bodies().len();let created=crate::ivp_poly_constructor::construct_at(id,parameters,self.state.dt,crate::ivp_group_effects::Context{now:self.clock.now,next:self.clock.deadline})?;
  self.append_created(object_id,created.binding,created.object_offset,parameters.margin*f32::from_bits(0x3cd013a9),friction)
 }
 /// Empty public groups remain inactive until they have members and activate.
 pub(crate) fn register_joint_group(&mut self,id:usize,definition:crate::ivp_registered_physical::GroupDefinition)->Result<(),&'static str>{
  if self.state.joints.contains_key(&id)||self.state.metadata.contains_key(&id)||!definition.tolerance.is_finite()||definition.tolerance<0.||!definition.rescue_gravity.is_finite()||self.state.rescue_gravity.is_some_and(|g|g.to_array().map(f32::to_bits)!=definition.rescue_gravity.to_array().map(f32::to_bits)){return Err("invalid contact-world group creation");}
  let mut next=self.clone();let mut metadata=Members::default();for &object in &next.state.objects{metadata.register_object(object)?;}
  for c in &definition.constraints{if next.state.retired_constraints.contains(&c.id)||next.state.metadata.values().any(|m|m.order().entries().iter().any(|e|e.id==c.id))||c.objects[0]==c.objects[1]{return Err("invalid new group constraint");}
   let class=match c.joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>0,crate::ivp_joint_mixed::Joint::Ragdoll(_)=>3,crate::ivp_joint_mixed::Joint::Hinge(_)=>1};
   metadata.insert_constraint(crate::ivp_group_members::Constraint{entry:crate::ivp_constraint_order::Entry{id:c.id,class},objects:[c.objects[1],c.objects[0]]})?;
   for object in c.objects{next.state.response.physical_bodies_mut()?.get_mut(object).ok_or("missing new-group physical body")?.object_flags|=0x1000;next.state.object_joint_listeners.entry(object).or_default().insert(c.id);}
  }
  let members=metadata.objects().to_vec();let mut offsets=vec![];
  for object in &members{let offset=*definition.offsets.get(object).ok_or("missing new-group object offset")?;let b=next.bodies().get(*object).ok_or("missing new-group body")?;
   if offset.is_some()!=(b.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite()))||next.state.object_offsets.get(object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("invalid new-group object offset");}
   next.state.object_offsets.insert(*object,offset);offsets.push(offset);
  }
  let bindings=definition.constraints.iter().map(|c|crate::ivp_joint_mixed::Binding{bodies:c.objects.map(|object|members.iter().position(|&v|v==object).unwrap()),joint:c.joint}).collect();
  let solver=crate::ivp_group_complete::Group{bindings,tail:crate::ivp_joint_tail::State::new(members,definition.minimum,definition.tolerance)};
  next.state.joints.insert(id,ActiveGroup{constraint_ids:definition.constraints.iter().map(|c|c.id).collect(),solver,offsets,additional:definition.additional,maximum:[0.;4]});next.state.metadata.insert(id,metadata);next.state.rescue_gravity=Some(definition.rescue_gravity);
  next.activate_joint_group(id)?;*self=next;Ok(())
 }
 pub(crate) fn activate_joint_group(&mut self,id:usize)->Result<(),&'static str>{
  let metadata=self.state.metadata.get(&id).ok_or("unknown contact-world group activation")?;if metadata.active()||metadata.objects().is_empty(){return Ok(());}
  let mut next=self.clone();let metadata=next.state.metadata.get_mut(&id).unwrap();metadata.activate()?;next.state.response.register_physical_group_in_candidate(id,metadata.cores())?;
  next.state.effects(next.clock.now,next.clock.deadline)?;*self=next;Ok(())
 }
 /// Public ragdoll/hinge selection uses this owner's current physical state.
 pub(crate) fn ragdoll_definition(&self,id:usize,objects:[usize;2],parameters:crate::ivp_ragdoll_constructor::Parameters,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<crate::ivp_registered_physical::JointDefinition,&'static str>{
  if objects[0]==objects[1]{return Err("self-bound contact-world public joint");}
  let mut pair=vec![];for object in objects{let body=*self.bodies().get(object).ok_or("unknown contact-world public joint object")?;let offset=*offsets.get(&object).ok_or("missing contact-world public joint offset")?;
   if offset.is_some()!=(body.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite()))||self.state.object_offsets.get(&object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("invalid contact-world public joint offset");}
   pair.push(crate::ivp_group_complete::Member{body,object_offset:offset});
  }
  let joint=crate::ivp_ragdoll_constructor::construct(parameters,[pair[0],pair[1]])?;Ok(crate::ivp_registered_physical::JointDefinition{id,objects,joint})
 }
 pub(crate) fn insert_joint(&mut self,group:usize,c:crate::ivp_registered_physical::JointDefinition,offsets:&BTreeMap<usize,Option<[f32;3]>>)->Result<(),&'static str>{
  if !self.state.joints.contains_key(&group)||self.state.retired_constraints.contains(&c.id)||self.state.metadata.values().any(|m|m.order().entries().iter().any(|e|e.id==c.id))||c.objects[0]==c.objects[1]{return Err("invalid contact-world joint insertion");}
  let mut next=self.clone();let class=match c.joint{crate::ivp_joint_mixed::Joint::Fixed(_)=>0,crate::ivp_joint_mixed::Joint::Ragdoll(_)=>3,crate::ivp_joint_mixed::Joint::Hinge(_)=>1};
  let metadata=next.state.metadata.get_mut(&group).ok_or("missing contact-world group metadata")?;
  metadata.insert_constraint(crate::ivp_group_members::Constraint{entry:crate::ivp_constraint_order::Entry{id:c.id,class},objects:[c.objects[1],c.objects[0]]})?;
  if metadata.active(){next.state.response.rebind_physical_group_in_candidate(group,metadata.cores())?;}
  let members=metadata.objects().to_vec();let mut bound_offsets=vec![];
  for object in &members{let offset=*offsets.get(object).ok_or("missing inserted joint offset")?;let b=next.state.response.physical_bodies()?.get(*object).ok_or("missing inserted joint physical body")?;
   if offset.is_some()!=(b.object_flags&0x800==0)||offset.is_some_and(|v|v.iter().any(|x|!x.is_finite()))||next.state.object_offsets.get(object).is_some_and(|old|old.map(|v|v.map(f32::to_bits))!=offset.map(|v|v.map(f32::to_bits))){return Err("invalid inserted joint object offset");}
   next.state.object_offsets.insert(*object,offset);bound_offsets.push(offset);
  }
  let active=next.state.joints.get_mut(&group).unwrap();if !members.starts_with(&active.solver.tail.members){return Err("insertion changed retained contact-world member indices");}
  active.solver.bindings.push(crate::ivp_joint_mixed::Binding{bodies:c.objects.map(|object|members.iter().position(|&id|id==object).unwrap()),joint:c.joint});active.constraint_ids.push(c.id);active.solver.tail.members=members;active.offsets=bound_offsets;
  for object in c.objects{next.state.response.physical_bodies_mut()?[object].object_flags|=0x1000;next.state.object_joint_listeners.entry(object).or_default().insert(c.id);}
  next.state.effects(next.clock.now,next.clock.deadline)?;*self=next;Ok(())
 }
 /// A public constraint can die while the group and its core vector remain.
 pub(crate) fn destroy_joint(&mut self,group:usize,constraint:usize)->Result<(),&'static str>{
  let active=self.state.joints.get(&group).ok_or("unknown contact-world joint group")?;
  let slot=active.constraint_ids.iter().position(|&id|id==constraint).ok_or("unknown contact-world constraint")?;
  if active.constraint_ids.len()!=active.solver.bindings.len(){return Err("incoherent contact-world constraint identities");}
  let endpoints=active.solver.bindings[slot].bodies.map(|i|active.solver.tail.members[i]);
  let mut next=self.clone();for id in endpoints{next.wake(id)?;}
  next.state.metadata.get_mut(&group).ok_or("missing contact-world group metadata")?.remove_constraint(constraint)?;
  for id in endpoints{let listeners=next.state.object_joint_listeners.get_mut(&id).ok_or("missing contact-world joint listeners")?;if !listeners.remove(&constraint){return Err("missing contact-world constraint listener");}if listeners.is_empty(){next.state.response.physical_bodies_mut()?[id].object_flags&=!0x1000;}}
  let active=next.state.joints.get_mut(&group).unwrap();active.constraint_ids.remove(slot);active.solver.bindings.remove(slot);
  *self=next;Ok(())
 }
 /// Detachment reserves orphan wrapper identities and preserves contact graphs.
 pub(crate) fn detach_joint_group(&mut self,id:usize)->Result<(),&'static str>{
  if !self.state.joints.contains_key(&id){return Err("unknown contact-world joint group");}
  let mut next=self.clone();let metadata=next.state.metadata.get(&id).ok_or("missing contact-world group metadata")?;
  next.state.retired_constraints.extend(metadata.order().entries().iter().map(|e|e.id));
  let active=&next.state.joints[&id];if active.constraint_ids.len()!=active.solver.bindings.len(){return Err("incoherent detached contact-world identities");}
  for (&constraint,binding) in active.constraint_ids.iter().zip(&active.solver.bindings){let endpoints=binding.bodies.map(|i|active.solver.tail.members[i]);if next.state.detached_constraints.insert(constraint,endpoints).is_some(){return Err("duplicate detached contact-world wrapper");}}
  if metadata.active(){next.state.response.detach_physical_group_in_candidate(id)?;}
  next.state.metadata.remove(&id);next.state.joints.remove(&id);next.state.effects(next.clock.now,next.clock.deadline)?;
  *self=next;Ok(())
 }
 pub(crate) fn destroy_detached_joint(&mut self,id:usize)->Result<(),&'static str>{
  let endpoints=*self.state.detached_constraints.get(&id).ok_or("unknown detached contact-world wrapper")?;if !self.state.retired_constraints.contains(&id){return Err("unreserved detached contact-world wrapper");}
  let mut next=self.clone();for object in endpoints{next.wake(object)?;}
  for object in endpoints{let listeners=next.state.object_joint_listeners.get_mut(&object).ok_or("missing detached contact-world listeners")?;if !listeners.remove(&id){return Err("missing detached contact-world listener");}if listeners.is_empty(){next.state.response.physical_bodies_mut()?[object].object_flags&=!0x1000;}}
  next.state.detached_constraints.remove(&id);next.state.retired_constraints.remove(&id);*self=next;Ok(())
 }
 pub(crate) fn queue_recovery(&mut self,id:usize,a:Option<usize>,b:Option<usize>)->Result<bool,&'static str>{Ok(self.state.joints.get_mut(&id).ok_or("unknown contact-world joint group")?.solver.tail.queue(a,b))}
 pub(crate) fn resolve(&mut self,flags:u32,features:[Feature;2],hint:Option<usize>)->Result<Option<u64>,&'static str>{
  let mut next=self.clone();next.state.refresh_context(next.clock.now)?;let id=next.state.response.resolve(flags,features,hint)?;next.state.effects(next.clock.now,next.clock.deadline)?;*self=next;Ok(id)
 }
 #[cfg(test)]
 pub(crate) fn inspect_collision(&mut self,callback:impl FnOnce(&mut CollisionContext<'_>)->Result<(),&'static str>)->Result<(),&'static str>{
  let mut next=self.clone();callback(&mut CollisionContext{state:&mut next.state,now:next.clock.now,deadline:next.clock.deadline})?;*self=next;Ok(())
 }
 /// Stage a synchronous public callback against the canonical physical/queue
 /// owners. A geometry owner must stage its own metadata/cache alongside it.
 pub(crate) fn with_candidate_context(&mut self,run:impl FnOnce(&mut CollisionContext<'_>,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  let mut next=self.clone();let now=next.clock.now;let deadline=next.clock.deadline;let clock=crate::ivp_world_cache::Context{time:now,frame:next.clock.clock_serial};
  run(&mut CollisionContext{state:&mut next.state,now,deadline},clock,deadline,&mut next.clock.queue)?;*self=next;Ok(())
 }
 pub(crate) fn advance(&mut self,delta:f32)->Result<(),&'static str>{
  self.advance_with_prediction(delta,|_,_,_,_,_|Ok(()))
 }
 /// Runs discovery after canonical body prediction and before PSI reinsertion.
 /// World state commits only on success. Callers must stage their own metadata
 /// as well; the sphere owner does so in its paired advance operation.
 pub(crate) fn advance_with_prediction(&mut self,delta:f32,predicted:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_callbacks(delta,predicted,|_,_,_,_,_|Err("contact-world collision event discovery is not connected"))
 }
 /// Discovery callbacks observe the canonical bank inside the real dispatcher.
 /// External callback state must be staged by its owner, as with predictions.
 pub(crate) fn advance_with_callbacks(&mut self,delta:f32,
  predicted:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  mut event:impl FnMut(&mut [Body],usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_collision_context(delta,predicted,|context,id,clock,deadline,queue|event(context.bodies_mut()?,id,clock,deadline,queue))
 }
 /// Loans the actual candidate world to the collision callback. Admission and
 /// its registry/history effects remain in the same transaction as prediction.
 pub(crate) fn advance_with_collision_context(&mut self,delta:f32,
  mut predicted:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_discovery_context(delta,|b,order,_,clock,deadline,q|predicted(b,order,clock,deadline,q),event)
 }
 /// Activation/sleep notifications keep the actual registry effect traversal;
 /// discovery must not reconstruct it by sorting body IDs or prediction calls.
 pub(crate) fn advance_with_discovery_context(&mut self,delta:f32,
  predicted:impl FnMut(&[Body],&[usize],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_cache_context(delta,|_,_,_|Ok(()),predicted,event)
 }
 /// The cache owner observes actual contact refresh traversal BEFORE prediction
 /// changes anchors. Cached frames remain valid for discovery at this serial.
 pub(crate) fn advance_with_cache_context(&mut self,delta:f32,
  prepare:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context)->Result<(),&'static str>,
  mut predicted:impl FnMut(&[Body],&[usize],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_mutable_cache_context(delta,prepare,|context,order,notices,clock,deadline,queue|predicted(context.bodies()?,order,notices,clock,deadline,queue),event)
 }
 /// Compact discovery/recovery runs against the same canonical candidate as
 /// controllers and collision events. No persistent body mirror is lent out.
 /// Only metadata lists are copied before the mutable context is borrowed.
 /// The world/queue commit together on success; external owners must also stage
 /// their own metadata, just as for the read-only discovery adapter above.
 pub(crate) fn advance_with_mutable_cache_context(&mut self,delta:f32,
  prepare:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context)->Result<(),&'static str>,
  predicted:impl FnMut(&mut CollisionContext<'_>,&[usize],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_unit_context(delta,prepare,|_,_,_,_,_|Ok(()),predicted,event)
 }
 pub(crate) fn advance_with_unit_context(&mut self,delta:f32,
  prepare:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context)->Result<(),&'static str>,
  completed_unit:impl FnMut(&mut CollisionContext<'_>,&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  predicted:impl FnMut(&mut CollisionContext<'_>,&[usize],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  self.advance_with_lifecycle_context(delta,prepare,|_,_,_,_|Ok(()),completed_unit,predicted,event)
 }
 pub(crate) fn advance_with_lifecycle_context(&mut self,delta:f32,
  mut prepare:impl FnMut(&[Body],&[usize],crate::ivp_world_cache::Context)->Result<(),&'static str>,
  mut started:impl FnMut(&mut CollisionContext<'_>,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  mut completed_unit:impl FnMut(&mut CollisionContext<'_>,&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  mut predicted:impl FnMut(&mut CollisionContext<'_>,&[usize],&[usize],crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>,
  mut event:impl FnMut(&mut CollisionContext<'_>,usize,crate::ivp_world_cache::Context,f64,&mut crate::ivp_event_queue::Queue)->Result<(),&'static str>)->Result<(),&'static str>{
  let mut next=self.clone();next.state.prediction_calls.clear();#[cfg(any(test,debug_assertions))] next.state.calls.clear();let state=std::cell::RefCell::new(&mut next.state);
  next.clock.simulate_callbacks(delta,|queue,now,deadline,serial|{
   let mut state=state.borrow_mut();let first=state.prediction_calls.len();state.step(now,deadline,|b,ids|prepare(b,ids,crate::ivp_world_cache::Context{time:now,frame:serial}),|context,members|match members{None=>started(context,crate::ivp_world_cache::Context{time:now,frame:serial},deadline,queue),Some(ids)=>completed_unit(context,ids,crate::ivp_world_cache::Context{time:now,frame:serial},deadline,queue)})?;
   let notices=std::mem::take(&mut state.object_notifications);
   let order=state.prediction_calls[first..].to_vec();
   predicted(&mut CollisionContext{state:&mut state,now,deadline},&order,&notices,crate::ivp_world_cache::Context{time:now,frame:serial},deadline,queue)
  },|queue,id,now,serial,deadline|{
   let mut state=state.borrow_mut();for b in state.response.physical_bodies_mut()?{b.clock=now;}
   event(&mut CollisionContext{state:&mut state,now,deadline},id,crate::ivp_world_cache::Context{time:now,frame:serial},deadline,queue)
  },|_,_|{})?;
  drop(state);
  for b in next.state.response.physical_bodies_mut()?{b.clock=next.clock.now;}*self=next;Ok(())
 }
}
impl State{
 fn override_context_frames(&mut self,now:f64,objects:[u64;2],cached:[crate::ivp_motion::PreparedObjectCache;2])->Result<(),&'static str>{
  let bodies=self.response.physical_bodies()?;let mut frames=Vec::with_capacity(self.frames.len());
  for (&id,&frame) in &self.frames{let cache=if let Some(side)=objects.iter().position(|&object|object==id){cached[side]}else{bodies[self.contact_cores[&id]].prediction.prepare_object_cache(now,frame,0)};let r=cache.pose.rotation.transpose();frames.push((id,[r.x_axis,r.y_axis,r.z_axis],cache.pose.position));}
  self.response.set_physical_context(now,&frames)
 }
 fn refresh_context(&mut self,now:f64)->Result<(),&'static str>{
  let bodies=self.response.physical_bodies()?;let mut frames=Vec::with_capacity(self.frames.len());
  for (&id,&frame) in &self.frames{let b=bodies[self.contact_cores[&id]];let cache=b.prediction.prepare_object_cache(now,frame,0);let r=cache.pose.rotation.transpose();frames.push((id,[r.x_axis,r.y_axis,r.z_axis],cache.pose.position));}
  self.response.set_physical_context(now,&frames)
 }
 fn effects(&mut self,now:f64,deadline:f64)->Result<(),&'static str>{
  let (effects,released,features)=self.response.take_effects();self.released.extend(released);self.released_features.extend(features);if effects.iter().any(|e|matches!(e,Effect::ForcedSleep(_))){return Err("forced sleep needs explicit eligibility and geometry");}
  let queued=(0..self.response.physical_bodies()?.len()).map(|id|self.response.registry().queued(id)).collect::<Result<Vec<_>,_>>()?;
  let bodies=self.response.physical_bodies_mut()?;
  crate::ivp_group_effects::apply_scheduled(bodies,&self.objects,&effects,crate::ivp_group_effects::Context{now,next:deadline},self.dt)?;
  for e in effects{if let Effect::Activated(id)|Effect::Slept(id)=e{self.object_notifications.push(id);}match e{Effect::ResetClock(id)|Effect::Activated(id)=>{self.histories.get_mut(&id).ok_or("missing contact-world movement history")?.reset_clock(now)?;},Effect::Slept(_)=>{},_=>unreachable!()}}
  for b in bodies{b.flags=(b.flags&!4)|if queued[b.id]{4}else{0};}Ok(())
 }
 fn joint(&mut self,id:usize,cores:&[usize])->Result<(),&'static str>{
  let metadata=self.metadata.get_mut(&id).ok_or("missing contact-world group metadata")?;metadata.prepare_order();let order=metadata.order().entries();
  let g=self.joints.get_mut(&id).ok_or("missing contact-world numerical group")?;
  if order.len()!=g.constraint_ids.len()||order.len()!=g.solver.bindings.len(){return Err("contact-world joint order size mismatch");}
  let mut bindings=g.constraint_ids.iter().copied().zip(g.solver.bindings.iter().copied()).collect::<BTreeMap<_,_>>();
  let reordered=order.iter().map(|entry|bindings.remove(&entry.id).ok_or("missing contact-world joint binding")).collect::<Result<Vec<_>,_>>()?;
  if !bindings.is_empty(){return Err("contact-world joint binding outside group order");}g.constraint_ids=order.iter().map(|e|e.id).collect();g.solver.bindings=reordered;
  let bodies=self.response.physical_bodies_mut()?;
  let mut members=g.solver.tail.members.iter().zip(&g.offsets).map(|(&id,&offset)|crate::ivp_group_complete::Member{body:bodies[id],object_offset:offset}).collect::<Vec<_>>();
  if cores.len()!=members.iter().filter(|m|m.body.flags&2==0).count()||cores.iter().any(|id|!g.solver.tail.members.contains(id)||bodies[*id].flags&2!=0){return Err("contact-world group core mismatch");}
  let result=g.solver.run_with(&mut members,g.additional,crate::ivp_group_complete::Environment{dt:self.dt,inverse_dt:self.dt.recip(),rescue_dt:self.dt,gravity:self.rescue_gravity.unwrap_or(Vec3::ZERO).to_array()},|_,_,_,_,_,_|{},|_,_,_,_,_|{})?;g.maximum=result.error.maximum;
  for member in members{let b=member.body;let h=self.histories.get_mut(&b.id).ok_or("missing contact-world joint history")?;h.close_time=b.stamps[0];h.far_time=b.stamps[1];bodies[b.id]=b;}Ok(())
 }
 fn base(&mut self,cores:&[usize])->Result<(),&'static str>{
  let bodies=self.response.physical_bodies_mut()?;let mut forces=cores.iter().map(|&id|{let b=bodies[id];let p=self.properties[&id];(id,crate::ivp_base_controller::Body{core:core(b),flags:b.flags as u8,movement:(b.flags>>8) as u8,linear_rate:p.linear_rate,angular_rate:p.angular_rate})}).collect();
  crate::ivp_base_controller::Controller{gravity:self.gravity,alternate:self.alternate}.execute(&mut forces,cores,self.dt)?;for (id,f) in forces{write_core(&mut bodies[id],f.core);}Ok(())
 }
 fn step(&mut self,now:f64,deadline:f64,mut prepare:impl FnMut(&[Body],&[usize])->Result<(),&'static str>,mut lifecycle:impl FnMut(&mut CollisionContext<'_>,Option<&[usize]>)->Result<(),&'static str>)->Result<(),&'static str>{
  self.prediction_motion.clear();
  self.response.set_physical_time(now)?;lifecycle(&mut CollisionContext{state:self,now,deadline},None)?;self.response.begin_physical_psi()?;self.effects(now,deadline)?;self.refresh_context(now)?;let mut processed=vec![];
  loop{
   let histories=&mut self.histories;let counts=&mut self.impact_counts;
   let work=self.response.next_physical_unit(&mut self.poll,|bodies,cores,flags|{
    let mut values=cores.iter().map(|&id|bodies[id]).collect::<Vec<_>>();let result=crate::ivp_unit_prepare::prepare(&mut values,flags,now)?;
    for b in values{if let Some(count)=counts.get_mut(&b.id){*count=0;}if result.reset_movement_clock{histories.get_mut(&b.id).ok_or("missing contact-world preparation history")?.reset_clock(now)?;}bodies[b.id]=b;}processed.extend(cores.iter().rev().copied());Ok(())
   })?;let Some(work)=work else{break;};
   for (key,cores) in work.controllers(){#[cfg(any(test,debug_assertions))] self.calls.push(*key);match *key{ControllerKey::Base=>self.base(cores)?,ControllerKey::Joint(id)=>self.joint(id,cores)?,ControllerKey::Contact{system,phase}=>{if phase==crate::ivp_unit_registry::ContactPhase::Refresh{let ids=self.response.refresh_cache_cores(system)?;prepare(self.response.physical_bodies()?,&ids)?;}self.response.execute_in_candidate(system,phase)?;}}}
   //100ff090 can reset movement clocks during a contact controller.
   // Publish those effects before100eace0 reads the movement history;
   // otherwise its stale clocks can put a still-active unit to sleep.
   self.effects(now,deadline)?;
   let histories=&mut self.histories;let properties=&self.properties;
   let members=work.cores().to_vec();
   let polled=self.response.poll_physical_unit(work,|id,b|{let h=histories.get_mut(&id).ok_or("missing contact-world polled history")?;let result=movement::classify(h,input(*b,properties[&id]),now,&settings())?;b.flags=(b.flags&0xff)|((result as u16)<<8);b.stamps=[h.close_time,h.far_time];Ok(result)})?;self.effects(now,deadline)?;
   lifecycle(&mut CollisionContext{state:self,now,deadline},Some(&members))?;
   self.response.finish_polled_physical_unit(polled)?;self.effects(now,deadline)?;
  }
  self.effects(now,deadline)?;
  for id in processed.into_iter().rev(){self.prediction_calls.push(id);let b=&mut self.response.physical_bodies_mut()?[id];let p=self.properties[&id];let result=b.prediction.advance(crate::ivp_motion::MotionStep{time:now,dt:self.dt,velocity:Vec3::new(b.linear[0],b.linear[1],b.linear[2]),angular:Vec3::new(b.angular[0],b.angular[1],b.angular[2]),inertia:p.inertia,inverse_inertia:Vec3::new(b.inverse[0],b.inverse[1],b.inverse[2]),limits:p.limits,angular_limit_exact:p.angular_limit_exact,gyro:p.gyro});b.linear[..3].copy_from_slice(&b.prediction.velocity.to_array());b.angular[..3].copy_from_slice(&result.angular.to_array());
   self.prediction_motion.insert(id,crate::ivp_motion::PredictionMotion{time:now,dt:self.dt,delta:result.delta,rotation:b.current.rotation,inverse_dt:b.prediction.inverse_dt});
   if !b.prediction.start.is_finite()||!b.prediction.velocity.is_finite()||!result.angular.is_finite()||b.prediction.rotation_end.iter().any(|v|!v.is_finite()){return Err("contact-world prediction overflow");}
  }Ok(())
 }
}

#[cfg(test)]
pub(crate) fn continuation_metadata(w:&ContactWorld)->serde_json::Value{serde_json::json!({
 "histories":w.state.histories.values().map(|h|serde_json::json!({"close_time":h.close_time.to_bits(),"far_time":h.far_time.to_bits(),"close_position":h.close_position.to_array().map(f32::to_bits),"far_position":h.far_position.to_array().map(f32::to_bits),"close_quat":h.close_quat.map(f32::to_bits),"far_quat":h.far_quat.map(f32::to_bits)})).collect::<Vec<_>>(),
 "poll":{"remaining":w.state.poll.remaining,"seed":w.state.poll.seed},
 "clock":{"now":w.clock.now.to_bits(),"next":w.clock.deadline.to_bits()},
 "impact_counts":w.state.impact_counts.values().copied().collect::<Vec<_>>(),
 "unit_flags":w.bodies().iter().map(|b|w.state.response.registry().unit_flags(b.id).unwrap().flags&0x3fff).collect::<Vec<_>>()})}
#[cfg(test)]
pub(crate) fn snapshot(w:&ContactWorld)->serde_json::Value{serde_json::json!({"object_notifications":w.state.object_notifications,"impact_cache":w.state.response.impact_snapshot(),"spheres":w.state.spheres,"elasticities":w.state.elasticities,"impact_counts":w.state.impact_counts,"physical":w.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),"contacts":w.state.response.world_snapshot(),"body_owners":w.state.response.body_owners_snapshot(),"properties":format!("{:?}",w.state.properties),"objects":format!("{:?}",w.state.objects),"contact_cores":w.state.contact_cores,"frames":w.state.frames.iter().map(|(id,f)|serde_json::json!({"id":id,"shift":f.shift.map(|v|v.to_array().map(f32::to_bits)),"rotation":f.rotation.map(|v|v.map(f32::to_bits))})).collect::<Vec<_>>(),"metadata":w.state.response.diagnostics(),"joints":format!("{:?}",w.state.joints),"groups":format!("{:?}",w.state.metadata),"listeners":format!("{:?}",w.state.object_joint_listeners),"retired":w.state.retired_constraints,"detached":w.state.detached_constraints,"offsets":format!("{:?}",w.state.object_offsets),"rescue_gravity":format!("{:?}",w.state.rescue_gravity),"clock":format!("{:?}",w.clock),"histories":format!("{:?}",w.state.histories),"poll":format!("{:?}",w.state.poll),"predictions":w.state.prediction_calls,"calls":format!("{:?}",w.state.calls),"released":w.state.released,"features":format!("{:?}",w.state.released_features)})}

#[cfg(test)]
pub(crate) fn verify_rollback_controls(world:&ContactWorld){

 let p=crate::ivp_sphere_constructor::Parameters{radius:16.1,position:[64.,0.,128.],angles:[0.;3],mass:15.,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:false,collisions:false};
 let mut bad=world.clone();let before=snapshot(&bad);for (id,parameters,friction) in [(0,p,0.8),(1,p,0.8),(10000,crate::ivp_sphere_constructor::Parameters{radius:-1.,..p},0.8),(10000,p,f32::NAN)]{assert!(bad.create_sphere(id,parameters,friction).is_err());assert_eq!(snapshot(&bad),before);}
 let mut bad=world.clone();let id=bad.bodies().len();bad.state.metadata.get_mut(&0).unwrap().register_object(Object{id,core:id,fixed:false}).unwrap();let before=snapshot(&bad);assert!(bad.create_sphere(10000,p,0.8).is_err());assert_eq!(snapshot(&bad),before,"late group definition failure leaked body, energy, work or registry ownership");
 let mut born=world.clone();let before=born.contacts().snapshot(1);born.create_sphere(10000,p,0.8).unwrap();let after=born.contacts().snapshot(1);assert_eq!(before["state"],after["state"]);assert_eq!(before["geometry"],after["geometry"],"body creation rebuilt existing contact geometry/history");
 let poly=crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{center:Vec3::new(0.05,-0.02,0.01),unit_inertia:Vec3::new(0.01,0.02,0.03),radius:0.4,factor:64},margin:1.,center:Some([1.25,-2.,0.5]),position:[64.,0.,128.],angles:[11.,-7.,3.],mass:15.,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:false,collisions:false};
 let mut bad=world.clone();let before=snapshot(&bad);for (id,parameters,friction) in [(0,poly,0.8),(1,poly,0.8),(10000,crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{unit_inertia:Vec3::ZERO,..poly.surface},..poly},0.8),(10000,poly,f32::NAN)]{assert!(bad.create_poly(id,parameters,friction).is_err());assert_eq!(snapshot(&bad),before);}
 let mut bad=world.clone();let id=bad.bodies().len();bad.state.metadata.get_mut(&0).unwrap().register_object(Object{id,core:id,fixed:false}).unwrap();let before=snapshot(&bad);assert!(bad.create_poly(10000,poly,0.8).is_err());assert_eq!(snapshot(&bad),before,"late poly definition failure leaked a body or offset");
 let mut born=world.clone();let before=born.contacts().snapshot(1);let id=born.create_poly(10000,poly,0.8).unwrap();assert!(born.object_offsets()[&id].is_some()&&born.state.frames[&10000].shift.is_some());let after=born.contacts().snapshot(1);assert_eq!(before["state"],after["state"]);assert_eq!(before["geometry"],after["geometry"]);
 use crate::ivp_joint_tail::Command;
 let mut bad=world.clone();let before=snapshot(&bad);let good=Command::AngularPush{body:0,delta:[0.01,0.02,0.03]};let huge=Command::AngularPush{body:0,delta:[f32::MAX;3]};
 for commands in [vec![good,Command::AngularPush{body:999,delta:[0.;3]}],vec![good,Command::PointImpulse{body:0,point:[f32::NAN;3],impulse:[0.;3]}],vec![good,huge,huge]]{assert!(bad.apply_impulses(&commands).is_err());assert_eq!(snapshot(&bad),before,"failed impulse batch leaked body, queue or contact state");}
 assert!(bad.set_velocity(0,Some(Vec3::splat(f32::NAN)),None).is_err());assert_eq!(snapshot(&bad),before,"failed public velocity leaked wake effects");
 assert!(bad.wake(999).is_err());assert_eq!(snapshot(&bad),before);bad.apply_impulses(&[]).unwrap();assert_eq!(snapshot(&bad),before);
 let new=crate::ivp_registered_physical::JointDefinition{id:99,objects:[1,0],joint:world.state.joints[&0].solver.bindings[0].joint};let offsets=world.state.object_offsets.clone();
 for c in [crate::ivp_registered_physical::JointDefinition{objects:[0,999],..new},crate::ivp_registered_physical::JointDefinition{objects:[0,0],..new},crate::ivp_registered_physical::JointDefinition{id:0,..new}]{let mut bad=world.clone();let before=snapshot(&bad);assert!(bad.insert_joint(0,c,&offsets).is_err());assert_eq!(snapshot(&bad),before);}
 let mut bad=world.clone();let mut missing=offsets.clone();missing.remove(&0);let before=snapshot(&bad);assert!(bad.insert_joint(0,new,&missing).is_err());assert_eq!(snapshot(&bad),before,"late offset failure leaked registry rebind or metadata");
 let mut bad=world.clone();bad.state.response.detach_physical_group_in_candidate(0).unwrap();let before=snapshot(&bad);assert!(bad.insert_joint(0,new,&offsets).is_err());assert_eq!(snapshot(&bad),before,"late insertion rebind failure leaked metadata");
 let mut bad=world.clone();bad.state.retired_constraints.insert(99);let before=snapshot(&bad);assert!(bad.insert_joint(0,new,&offsets).is_err());assert_eq!(snapshot(&bad),before);
 let group_definition=crate::ivp_registered_physical::GroupDefinition{constraints:vec![new],offsets:offsets.clone(),additional:0,minimum:3,tolerance:3.,rescue_gravity:world.state.rescue_gravity.unwrap_or(Vec3::ZERO)};
 let mut bad=world.clone();let before=snapshot(&bad);assert!(bad.register_joint_group(0,group_definition.clone()).is_err());assert!(bad.activate_joint_group(999).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();let mut invalid=group_definition.clone();invalid.rescue_gravity.x+=1.;let before=snapshot(&bad);assert!(bad.register_joint_group(1,invalid).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();let mut missing=group_definition.clone();missing.offsets.remove(&0);let before=snapshot(&bad);assert!(bad.register_joint_group(1,missing).is_err());assert_eq!(snapshot(&bad),before,"new-group offset failure leaked listeners");
 let mut bad=world.clone();bad.state.response.register_physical_group_in_candidate(1,&[0,1]).unwrap();let before=snapshot(&bad);assert!(bad.register_joint_group(1,group_definition.clone()).is_err());assert_eq!(snapshot(&bad),before,"late new-group registry failure leaked metadata or physical state");
 let mut bad=world.clone();let mut empty=group_definition.clone();empty.constraints.clear();bad.register_joint_group(1,empty).unwrap();assert!(!bad.group(1).unwrap().active());let before=snapshot(&bad);bad.activate_joint_group(1).unwrap();assert_eq!(snapshot(&bad),before);bad.insert_joint(1,new,&offsets).unwrap();assert!(!bad.group(1).unwrap().active());bad.state.response.register_physical_group_in_candidate(1,&[0,1]).unwrap();let before=snapshot(&bad);assert!(bad.activate_joint_group(1).is_err());assert_eq!(snapshot(&bad),before,"late activation failure leaked active flag");
 let mut bad=world.clone();let before=snapshot(&bad);assert!(bad.destroy_joint(99,0).is_err());assert!(bad.destroy_joint(0,999).is_err());assert!(bad.detach_joint_group(99).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();bad.state.object_joint_listeners.get_mut(&0).unwrap().remove(&0);let before=snapshot(&bad);assert!(bad.destroy_joint(0,0).is_err());assert_eq!(snapshot(&bad),before,"late listener failure leaked endpoint wake or numerical state");
 let mut bad=world.clone();bad.destroy_joint(0,0).unwrap();let before=snapshot(&bad);assert!(bad.destroy_joint(0,0).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();bad.state.response.detach_physical_group_in_candidate(0).unwrap();let before=snapshot(&bad);assert!(bad.detach_joint_group(0).is_err());assert_eq!(snapshot(&bad),before,"late registry detachment failure leaked reserved identities or metadata");
 let mut bad=world.clone();let before=snapshot(&bad);assert!(bad.destroy_detached_joint(999).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();bad.detach_joint_group(0).unwrap();let ends=bad.state.detached_constraints[&0];bad.state.object_joint_listeners.get_mut(&ends[1]).unwrap().remove(&0);let before=snapshot(&bad);assert!(bad.destroy_detached_joint(0).is_err());assert_eq!(snapshot(&bad),before,"late detached listener failure leaked wake/ownership prefix");
 let mut bad=world.clone();bad.detach_joint_group(0).unwrap();bad.destroy_detached_joint(0).unwrap();assert!(!bad.state.retired_constraints.contains(&0)&&!bad.state.detached_constraints.contains_key(&0));let before=snapshot(&bad);assert!(bad.destroy_detached_joint(0).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();let retained=bad.state.joints[&0].constraint_ids.clone();bad.detach_joint_group(0).unwrap();assert!(retained.iter().all(|id|bad.state.retired_constraints.contains(id)));let before=snapshot(&bad);assert!(bad.detach_joint_group(0).is_err());assert_eq!(snapshot(&bad),before);
 let mut bad=world.clone();bad.state.joints.get_mut(&0).unwrap().solver.bindings[0].bodies[1]=usize::MAX;let before=snapshot(&bad);assert!(bad.advance(1./60.).is_err());assert_eq!(snapshot(&bad),before,"late joint failure leaked candidate physics/contact state");
 let mut bad=world.clone();bad.clock.queue.insert(99,0.001).unwrap();let before=snapshot(&bad);assert!(bad.advance(1./60.).is_err());assert_eq!(snapshot(&bad),before,"unported event dispatch leaked candidate physics/contact state");
 let mut bad=world.clone();let feature=crate::ivp_managed_response::Feature{material_code:0,endpoint:crate::ivp_create::Endpoint{object:1,feature:crate::ivp_create::Feature{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]};let before=snapshot(&bad);assert!(bad.resolve(0xc0000,[feature.clone(),feature],Some(0)).is_err());assert_eq!(snapshot(&bad),before,"invalid admission leaked refreshed cache context");

}

#[cfg(test)]
mod compact_wake_notification_tests {
 use super::*;
 use crate::ivp_surface_admission::contacts::ownership::{Ownership,controllers::Controller};
 #[test]
 fn immediate_member_wake_delivers_compact_alias_exit_with_published_body_flags(){
  let mut world=ContactWorld::empty_native(1./60.,123,Vec3::ZERO,Vec3::ZERO).unwrap();
  for id in 0..2{world.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:1.,position:[id as f32*200.,0.,0.],angles:[0.;3],mass:10.,inertia_scale:1.,damping:0.,angular_damping:0.,rotational_limit:0.,fixed:false,collisions:false},0.8).unwrap();}
  let mut compact=Ownership::new(2,8,8,0.).unwrap();compact.bind_objects([0,1],[0,1]).unwrap();let mut c=Controller::new(0,0.1,true).unwrap();c.track_cores([(1,1)]).unwrap();c.listen(2,7).unwrap();compact.controllers.insert(c).unwrap();
  compact.change_controller_contact(0,0,[0,1],true,world.state.response.physical_bodies_mut().unwrap(),|_,_,_|Ok(())).unwrap();compact.change_controller_contact(0,0,[0,1],false,world.state.response.physical_bodies_mut().unwrap(),|_,_,_|Ok(())).unwrap();
  assert!(compact.controllers.get(0).unwrap().aliased(1));assert_ne!(world.bodies()[1].object_flags&0x1000,0);assert_eq!(world.bodies()[1].object_flags&0xff,8);let mut calls=vec![];
  let mut context=CollisionContext{state:&mut world.state,now:0.,deadline:1./60.};
  assert_eq!(context.wake_impact_core_with_compact(1,&mut compact,|id,c,e,bodies|{assert_eq!(id,0);assert_eq!((e.channel,e.listener,e.subject,e.add),(4,7,1,false));assert!(!c.aliased(1));assert_eq!(bodies[1].object_flags&0x1000,0);assert_eq!(bodies[1].object_flags&0xff,1);calls.push(e);Ok(())}).unwrap(),vec![1]);
  assert!(context.wake_impact_core_with_compact(1,&mut compact,|_,_,_,_|panic!("already awake core repeated alias exit")).unwrap().is_empty());
  assert_eq!(calls.len(),1);assert!(!compact.controllers.get(0).unwrap().aliased(1));assert_eq!(world.bodies()[1].object_flags&0xff,1);assert_eq!(world.bodies()[0].object_flags&0xff,8);
 }
}

#[cfg(test)]
#[path="ivp_native_compact_wake_tests.rs"]
mod native_compact_wake_tests;

#[cfg(test)]
#[path="ivp_native_unit_wake_tests.rs"]
mod native_unit_wake_tests;

#[cfg(test)]
#[path="ivp_native_compact_neighbour_tests.rs"]
mod native_compact_neighbour_tests;

#[cfg(test)]
mod mutable_discovery_tests{
 use super::*;
 use std::cell::Cell;
 fn world()->ContactWorld{
  let mut w=ContactWorld::empty_native(1./60.,123,Vec3::ZERO,Vec3::ZERO).unwrap();
  for id in 0..2{w.create_sphere(id+1,crate::ivp_sphere_constructor::Parameters{radius:1.,position:[id as f32*2000.,0.,128.],angles:[0.;3],mass:10.,inertia_scale:1.,damping:0.,angular_damping:0.,rotational_limit:0.1,fixed:false,collisions:false},0.8).unwrap();}
  w.set_velocity(0,Some(Vec3::ZERO),None).unwrap();w
 }
 fn snapshot(w:&ContactWorld)->serde_json::Value{serde_json::json!({
  "bodies":w.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),
  "registry":format!("{:?}",w.registry()),"clock":format!("{:?}",w.clock),"histories":format!("{:?}",w.state.histories),
  "contacts":w.contacts().diagnostics(),"owners":w.contacts().body_owners_snapshot(),"notifications":w.state.object_notifications,
  "prediction_calls":w.state.prediction_calls,"prediction_motion":format!("{:?}",w.state.prediction_motion),"released":w.state.released,
 })}
 #[test]
 fn mutable_discovery_and_collision_events_share_the_committed_candidate_bank(){
  let mut w=world();let pointer=Cell::new(0usize);let frame=Cell::new(0u32);let calls=Cell::new(0usize);
  w.advance_with_mutable_cache_context(0.002,|_,_,_|Ok(()),|c,order,notices,clock,deadline,queue|{
   assert_eq!(order,&[0]);assert_eq!(notices,&[0]);assert_eq!(c.current_time(),clock.time);assert!(deadline>clock.time);
   assert_eq!(c.prediction_motion(0)?.time.to_bits(),clock.time.to_bits());
   pointer.set(c.bodies()?.as_ptr() as usize);frame.set(clock.frame);
   c.bodies_mut()?[0].pending_linear[0]=0.25;
   queue.insert(7,0.001)?;Ok(())
  },|c,id,clock,_,_|{
   assert_eq!(id,7);assert_eq!(c.current_time(),clock.time);assert!(clock.frame>frame.get());
   assert!(c.prediction_motion(0).is_err(),"collision reused another time's prediction delta");
   assert_eq!(c.bodies()?.as_ptr() as usize,pointer.get());assert_eq!(c.bodies()?[0].pending_linear[0],0.25);
   c.queue_wake(1)?;c.bodies_mut()?[0].pending_angular[2]=0.125;calls.set(calls.get()+1);Ok(())
  }).unwrap();
  assert_eq!(calls.get(),1);assert_eq!(w.bodies().as_ptr() as usize,pointer.get());
  assert_eq!(w.bodies()[0].pending_linear[0],0.25);assert_eq!(w.bodies()[0].pending_angular[2],0.125);
  assert_ne!(w.bodies()[1].flags&4,0);assert_eq!(w.clock.now,0.002f32 as f64);assert!(w.clock.queue.scheduled(7).is_none());
 }
 #[test]
 fn mutable_discovery_or_event_failure_rolls_back_bodies_registry_and_queue(){
  for fail_discovery in [true,false]{
   let mut w=world();let before=snapshot(&w);let event_called=Cell::new(false);
   let result=w.advance_with_mutable_cache_context(0.002,|_,_,_|Ok(()),|c,_,_,_,_,queue|{
    c.queue_wake(1)?;c.bodies_mut()?[0].pending_linear[0]=0.25;queue.insert(7,0.001)?;
    if fail_discovery{Err("discovery failure")}else{Ok(())}
   },|c,id,_,_,queue|{
    assert_eq!(id,7);event_called.set(true);c.bodies_mut()?[0].pending_angular[2]=0.125;queue.insert(8,0.0015)?;Err("event failure")
   });
   assert_eq!(result,Err(if fail_discovery{"discovery failure"}else{"event failure"}));
   assert_eq!(event_called.get(),!fail_discovery);assert_eq!(snapshot(&w),before,"failed discovery/event leaked a partial candidate");
  }
 }
}
