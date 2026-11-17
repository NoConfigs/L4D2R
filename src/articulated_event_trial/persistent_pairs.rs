//! Source-owned normal compact pair membership and prediction scheduling.
//! Physical response remains the controlled trial contact implementation.
use super::*;
use crate::ivp_trial_bounds::{Hooks,DistanceAction,DistanceEdit,DistanceEvent,Request,World};
use crate::ivp_hull_budget::Budget;
use crate::ivp_distance_schedule as schedule;
use crate::ivp_far_distance as far;
use crate::ivp_feature_handler as feature;
pub(super) struct Context<'a>{
 pub cache:&'a mut WorldCache,pub context:ContextClock,pub trees:&'a [std::sync::Arc<l4d2_formats::phy::CompactTree>;2],pub radii:[f32;2],pub moving:&'a [Convex],pub floor:&'a [Convex],
 pub admissions:&'a mut std::collections::BTreeMap<usize,SurfaceAdmission>,pub records:&'a mut Vec<DetailedPair>,pub graph:&'a mut crate::ivp_mindist_graph::Graph,
 pub queue:&'a mut crate::ivp_event_queue::Queue,pub motions:[PairMotion;2],pub epoch:f64,pub deadline:f64,pub dt:f32,pub counter:&'a mut u32,pub events:Vec<serde_json::Value>,
}
type ContextClock=crate::ivp_world_cache::Context;
impl Context<'_>{
 fn index(&self,id:usize)->Result<usize,&'static str>{self.records.iter().position(|p|p.id==id).ok_or("missing source detailed pair")}
 fn snapshot(&self,index:usize)->Result<serde_json::Value,&'static str>{
  let p=&self.records[index];let g=p.geometry;
  Ok(serde_json::json!({"id":p.id,"slots":p.surface.slots,"ledges":p.surface.ledges,"flags":self.graph.flags(p.id)?,"frame":p.query.frame,"kinds":p.query.kinds,
   "vertex":[p.query.vertex.triangle,p.query.vertex.edge],"face":[p.query.face.triangle,p.query.face.edge],"projection":g.projection,"linear_delta":g.linear_delta,"distance":g.distance,"normal":g.normal.to_array(),"far":p.is_far,
   "queue":p.queued.map(|q|{let(epoch,offset)=self.queue.scheduled(p.id).expect("queued pair lost its native handle");serde_json::json!({"epoch":epoch,"offset_bits":offset.to_bits(),"kind":q.kind,"time":epoch+offset as f64})})}))
 }
 fn refresh(&mut self,index:usize)->Result<bool,&'static str>{
  let p=&mut self.records[index];if p.query.frame==self.context.frame{return Ok(false);}
  let slots=p.surface.slots;let leases=self.cache.retain_pair(slots,self.context)?;
  let frames=[self.cache.lease_frame(&leases.0)?,self.cache.lease_frame(&leases.1)?];
  let result=p.query.refresh(self.context.frame,p.hulls[0].as_ref(),p.hulls[1].as_ref(),frames,0.);
  let changed=self.cache.with_leases(leases,|release|{release();result})?;
  let g=p.query.geometry.ok_or("refreshed detailed pair lacks geometry")?;
  p.geometry.projection=g.projection;p.geometry.distance=g.distance;p.geometry.normal=g.normal;
  let flags=self.graph.flags(p.id)?;self.graph.set_flags(p.id,flags&!0xc000)?;Ok(changed)
 }
 fn allow_far(&self,slots:[usize;2])->Result<bool,&'static str>{let a=self.cache.object_inputs(slots[0])?.2 as u8;let b=self.cache.object_inputs(slots[1])?.2 as u8;Ok((a|b)<0x21)}
 /// None leaves the pair active. Some transfers it to its two hull listeners.
 fn schedule(&mut self,index:usize,phase:&str,allow_far:bool,delay:u8,budgets:[Budget;2])->Result<Option<[f32;2]>,&'static str>{
  let p=&self.records[index];if p.is_far{return Err("attempted active scheduling of a far pair");}
  if p.query.kinds!=[0,2]{return Err("pair scheduler supports only normal vertex/face features");}
  let id=p.id;let slots=p.surface.slots;let geometry=p.geometry;let before=self.snapshot(index)?;let flags=self.graph.flags(id)?;
  let body=[self.cache.object_inputs(slots[0])?,self.cache.object_inputs(slots[1])?];let config=schedule::Config::default();
  let input=schedule::Input{flags,counter:*self.counter,now:self.context.time,deadline:self.deadline,dt:self.dt,distance:geometry.distance,target:config.target(flags)?,normal:geometry.normal,
   cores:slots.map(|object|{let m=self.motions[object];schedule::Core{motion:crate::ivp_bounds::Motion{axis:m.axis,angular_bound:m.bound,velocity:m.velocity},linear:m.linear}}),
   budgets:std::array::from_fn(|k|schedule::Budget{time:budgets[k].time,flags:body[k].2,rates:budgets[k].rates,accumulated:budgets[k].accumulated}),allow_far,slow_gate:config.slow_gate};
  if self.records[index].queued.take().is_some(){self.queue.cancel(id)?;}let mut decision=schedule::prepare(&input)?;*self.counter=decision.counter;self.graph.set_flags(id,decision.flags)?;
  let(mut kind,mut candidate,mut searches)=(0,None,0);let mut far_keys=None;let mut feature_input_audit=None;
  match decision.action{
   schedule::Action::Hull{keys,linear_delta}=>{self.graph.unlink(id)?;let p=&mut self.records[index];p.is_far=true;p.geometry.linear_delta=linear_delta;far_keys=Some(keys);},
   schedule::Action::Skip=>{},
   schedule::Action::Feature{closing_bound,..}=>{
    let leases=self.cache.retain_pair(slots,self.context)?;let cached=[self.cache.lease_frame(&leases.0)?.pose,self.cache.lease_frame(&leases.1)?.pose];
    let p=&self.records[index];let feature_input=feature::Input{hulls:[p.hulls[0].as_ref(),p.hulls[1].as_ref()],edges:[p.query.vertex,p.query.face],
     bodies:std::array::from_fn(|k|feature::Body{prediction:body[k].0,frame:body[k].1,flags:body[k].2,cached:cached[k],angular_speed:self.motions[slots[k]].speed,inverse_diameter:0.5/self.radii[slots[k]]}),
     now:input.now,deadline:input.deadline,bound:closing_bound,margin:0.,distance:input.distance,target:input.target};
    if std::env::var_os("L4D2R_PHY_TRACE_PAIR_INPUTS").is_some(){feature_input_audit=Some(feature::describe(&feature_input));}
    let outcome=feature::run(feature_input);
    let outcome=self.cache.with_leases(leases,|release|{release();outcome})?;kind=outcome.kind;candidate=outcome.candidate;searches=outcome.searches.len();
    let offset=schedule::complete(&input,&mut decision,kind,candidate,delay,config.time_scale,self.epoch)?;self.graph.set_flags(id,decision.flags)?;
    if let Some(offset)=offset{self.queue.insert(id,offset)?;self.records[index].queued=Some(PairQueue{kind});}
   }
  }
  let mut audit=serde_json::json!({"kind":"pair_schedule","phase":phase,"time":self.context.time,"epoch":self.epoch,"deadline":self.deadline,"id":id,"allow_far":allow_far,"delay":delay,"counter_before":input.counter,"counter_after":*self.counter,"before":before,"after":self.snapshot(index)?,"feature_kind":kind,"candidate":candidate,"searches":searches,"far_keys":far_keys});if let Some(input)=feature_input_audit{audit["feature_input"]=input;}self.events.push(audit);
  Ok(far_keys)
 }
 fn destroy(&mut self,index:usize)->Result<Option<DistanceEdit>,&'static str>{let p=self.records.remove(index);if p.queued.is_some(){self.queue.cancel(p.id)?;}self.graph.remove(p.id)?;Ok(p.is_far.then_some(DistanceEdit::Clear{pair:p.id}))}
 pub(super) fn refresh_active(&mut self)->Result<(),&'static str>{
  for id in self.graph.active().to_vec(){let index=self.index(id)?;let before=self.snapshot(index)?;let changed=self.refresh(index)?;self.events.push(serde_json::json!({"kind":"pair_refresh","time":self.context.time,"id":id,"changed":changed,"before":before,"after":self.snapshot(index)?}));}Ok(())
 }
 pub(super) fn predict_active(&mut self,world:&mut World,reaction:bool)->Result<(),&'static str>{
  for id in self.graph.active().to_vec(){let index=self.index(id)?;if reaction{self.refresh(index)?;}let slots=self.records[index].surface.slots;let budgets=[world.object_budget(slots[0])?,world.object_budget(slots[1])?];
   if let Some(keys)=self.schedule(index,if reaction{"reaction"}else{"post_prediction"},!reaction,if reaction{2}else{1},budgets)?{world.register_distance(id,slots,keys)?;}
  }Ok(())
 }
 /// Consume a source-owned event at the current cache clock. Physical
 /// response remains the controlled trial's contact implementation.
 pub(super) fn dispatch_queued(&mut self,world:&mut World,id:usize)->Result<bool,&'static str>{
  let index=self.index(id)?;if self.queue.pop()?!=Some((id,self.context.time)){return Err("pair callback lost native list-head ownership");}
  let queued=self.records[index].queued.take().ok_or("queued pair lost event ownership")?;
  let before=self.snapshot(index)?;self.refresh(index)?;
  let action=schedule::callback(self.graph.flags(id)?,self.records[index].geometry.distance,schedule::Config::default())?;
  let physical=match action{
   schedule::Callback::Physical=>true,
   schedule::Callback::InvalidGeometry=>false,
   schedule::Callback::Reschedule{delay}=>{
    let slots=self.records[index].surface.slots;let budgets=[world.object_budget(slots[0])?,world.object_budget(slots[1])?];
    if self.schedule(index,"queued_callback",false,delay,budgets)?.is_some(){return Err("queued callback unexpectedly transferred to far tracking");}false
   }
  };
  if queued.kind==33{
   // A directional queue callback is the common feature event. Observe the
   // already prepared cache without another access, retain or clock change.
   let p=&self.records[index];let object=self.cache.object_frame(p.surface.slots[0])?;
   let vertex=Vec3::from_array(p.hulls[0].vertex(p.query.vertex).ok_or("feature audit lost its vertex")?);
   let point=object.pose.transform_point(vertex);
   self.events.push(serde_json::json!({"kind":"feature","time":self.context.time-self.epoch,"absolute_time":self.context.time,"pair":id,"cached_point_ivp":point.to_array(),"owner":"compact_pair_queue"}));
  }
  self.events.push(serde_json::json!({"kind":"pair_queue_dispatch","id":id,"time":self.context.time,"event_kind":queued.kind,"physical":physical,"before":before,"after":self.snapshot(index)?}));Ok(physical)
 }
 pub(super) fn audit(&mut self,requests:Vec<Request>){for request in requests{match request{
  Request::Dispatch{object,before,after}=>{let budget=|b:Budget|serde_json::json!({"time":b.time,"rates":b.rates,"accumulated":b.accumulated,"predicted":b.predicted,"threshold":b.threshold});self.events.push(serde_json::json!({"kind":"bounds_dispatch","object":object,"time":before.time,"before":budget(before),"after":budget(after)}));},
  Request::Distance{pair,objects}=>self.events.push(serde_json::json!({"kind":"distance_pair_due","time":self.context.time,"pair":pair,"objects":objects,"processed_inside_dispatch":true})),
  Request::Geometry{..}|Request::Removed{..}=>{},
 }}}
}
impl Hooks for Context<'_>{
 fn geometry(&mut self,owner:usize,objects:[usize;2],margin:f32,budgets:[Budget;2])->Result<Vec<DistanceEdit>,&'static str>{
  for object in objects{self.cache.access(object,self.context)?;}
  let bodies=objects.map(|object|crate::ivp_surface_admission::Body{prediction:self.cache.object_inputs(object).unwrap().0,prepared_pose:self.cache.object_frame(object).unwrap().pose,core_radius:self.radii[object],margin:0.});
  let trees=objects.map(|object|self.trees[object].as_ref());let (lists,queries)=crate::ivp_surface_admission::select(trees,bodies,self.context.time,margin,[None;2],[None;2])?;
  let(created,removed,retained)={let a=self.admissions.entry(owner).or_insert_with(||SurfaceAdmission{objects,pairs:vec![]});if a.objects!=objects{return Err("broad pair changed identity without destruction");}
   let(c,r)=crate::ivp_surface_admission::reconcile(objects,&lists,&mut a.pairs)?;(c,r,pairs_snapshot(&a.pairs))};
  let mut edits=vec![];for surface in &removed{let index=self.records.iter().position(|p|p.owner==owner&&p.surface==*surface).ok_or("surface removal lost its detailed owner")?;if let Some(edit)=self.destroy(index)?{edits.push(edit);}}
  let mut constructed=vec![];
  for surface in &created{
   let id=self.graph.register(objects)?;self.graph.activate(id,[false;2])?;
   let hulls=objects.map(|object|{let side=if object==objects[0]{0}else{1};let tree=&self.trees[object];let key=surface.ledges[side];let convex=if let Some(&i)=tree.leaf_convexes.get(&key){if object==0{&self.moving[i]}else{&self.floor[i]}}else{tree.coarse_convexes.get(&key).expect("admitted coarse ledge missing")};convex.topology.as_ref().expect("admitted compact topology missing").clone()});
   self.records.push(DetailedPair{id,owner,surface:*surface,query:crate::ivp_minimizer::Query::compact_pair(),hulls,geometry:far::Geometry{projection:0.,linear_delta:0.,distance:0.,normal:Vec3::ZERO},is_far:false,queued:None});let index=self.records.len()-1;self.refresh(index)?;
   let p=&self.records[index];constructed.push(serde_json::json!({"id":id,"slots":objects,"ledges":surface.ledges,"pre_schedule":true,"flags":self.graph.flags(id)?,"frame":p.query.frame,"kinds":p.query.kinds,"vertex":[p.query.vertex.triangle,p.query.vertex.edge],"face":[p.query.face.triangle,p.query.face.edge],"geometry":p.query.geometry.map(|g|serde_json::json!({"distance_bits":g.distance.to_bits(),"projection_bits":g.projection.to_bits(),"normal_bits":g.normal.to_array().map(f32::to_bits)})),"error":null}));
   if let Some(keys)=self.schedule(index,"constructor",self.allow_far(objects)?,0,budgets)?{edits.push(DistanceEdit::Register{pair:id,objects,keys});}
  }
  let queries=queries.map(|q|q.map(|q|serde_json::json!({"point_bits":q.point.map(f32::to_bits),"radius_bits":q.radius.to_bits(),"restricted":q.restricted})));
  self.events.push(serde_json::json!({"kind":"bounds_pair_query","time":self.context.time,"pair":owner,"objects":objects,"margin_bits":margin.to_bits(),"queries":queries,"ledge_lists":lists,"surface_pairs":retained,"created":pairs_snapshot(&created),"removed":pairs_snapshot(&removed),"constructed":constructed,"compact_admission_implemented":true,"minimizer_graph_implemented":false,"normal_pair_scheduling_implemented":true}));Ok(edits)
 }
 fn removed(&mut self,owner:usize,_objects:[usize;2])->Result<Vec<DistanceEdit>,&'static str>{
  let mut edits=vec![];if let Some(a)=self.admissions.remove(&owner){for surface in a.pairs.into_iter().rev(){let index=self.records.iter().position(|p|p.owner==owner&&p.surface==surface).ok_or("broad destruction lost detailed owner")?;if let Some(edit)=self.destroy(index)?{edits.push(edit);}}}
  self.events.push(serde_json::json!({"kind":"bounds_pair_removed","time":self.context.time,"pair":owner}));Ok(edits)
 }
 fn distance(&mut self,event:DistanceEvent)->Result<DistanceAction,&'static str>{match event{
  DistanceEvent::Rebase{pair,object,motion,linear,..}=>{let index=self.index(pair)?;if !self.records[index].is_far{return Err("rebase reached an active detailed pair");}self.records[index].geometry.rebase(motion,linear);self.events.push(serde_json::json!({"kind":"pair_far_rebase","id":pair,"object":object,"time":self.context.time,"motion_bits":motion.to_bits(),"linear_bits":linear.to_bits(),"state":self.snapshot(index)?}));Ok(DistanceAction::Keep)},
  DistanceEvent::Recheck{pair,objects,gap,budgets,..}=>{
   let index=self.index(pair)?;let p=&self.records[index];if !p.is_far||p.surface.slots!=objects{return Err("far callback lost pair membership");}let before=self.snapshot(index)?;
   let predictions=[self.cache.object_inputs(objects[0])?.0,self.cache.object_inputs(objects[1])?.0];
   let input=far::Input{flags:self.graph.flags(pair)?,now:self.context.time,dt:self.dt,gap,geometry:p.geometry,cores:std::array::from_fn(|k|{let m=self.motions[objects[k]];let p=predictions[k];far::Core{time:p.time,start:p.start,velocity:p.velocity,linear:m.linear,angular:m.bound}}),budgets};
   let action=far::update(&input,far::Config::default())?;let result=match action{
    far::Action::Rearm{geometry,keys,first}=>{let p=&mut self.records[index];p.geometry=geometry;p.query.geometry=Some(crate::ivp_minimizer::Geometry{distance:geometry.distance,projection:geometry.projection,normal:geometry.normal});DistanceAction::Rearm{keys,first}},
    far::Action::Activate{pending}=>{if pending{return Err("pending detailed activation is not yet connected");}self.records[index].is_far=false;self.graph.activate(pair,[false;2])?;self.refresh(index)?;
     let replacement=self.schedule(index,"far_activation",self.allow_far(objects)?,0,budgets)?;DistanceAction::Activate{replacement}
    }
   };
   self.events.push(serde_json::json!({"kind":"pair_far_callback","id":pair,"time":self.context.time,"gap":gap,"before":before,"after":self.snapshot(index)?,"rearmed":matches!(action,far::Action::Rearm{..})}));Ok(result)
  }
 }}
}
