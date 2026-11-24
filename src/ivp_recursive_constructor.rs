//! Native recursive-parent expansion with real child constructors and teardown.
//! The encompassing physical/discovery owner stages this candidate as a unit.
use super::*;
use crate::{ivp_joint_apply::Body as Physical,ivp_world_cache::{WorldCache,Context as Clock},ivp_minimizer::owner::Schedule};
#[derive(Clone,Debug)]
pub(crate) struct Binding{pub path:Vec<Pair>,pub id:usize}
pub(crate) struct Context<'a>{
 pub cache:&'a mut WorldCache,pub bodies:&'a mut[Physical;2],pub clock:Clock,pub timing:Schedule,pub gravity:[f32;3],
 pub margins:[f32;2],pub motion:[[f32;2];2],pub root:Counter,pub limit:i32,pub fallback:f32,
}
struct Builder<'a,'b>{context:&'b mut Context<'a>,prefix:Vec<Pair>,bindings:Vec<Binding>}
impl Builder<'_,'_>{
 fn retire<'a>(&mut self,owner:&mut ownership::Ownership,key:&Pair,entry:&mut Entry<'a>)->Result<(),&'static str>{
  self.prefix.push(*key);
  let result=(||{let id=self.bindings.iter().find(|b|b.path==self.prefix).ok_or("retiring recursive child lacks binding")?.id;retire_at(owner,id,entry,&mut self.prefix,&self.bindings)?;self.bindings.retain(|b|!b.path.starts_with(&self.prefix));Ok(())})();
  self.prefix.pop();result
 }
 fn input(&mut self,objects:[usize;2])->Result<Input,&'static str>{
  let c=&mut self.context;
  for side in 0..2{c.cache.update_core(c.bodies[side].id,c.bodies[side].prediction)?;c.cache.set_flags(objects[side],c.bodies[side].object_flags)?;}
  let leases=c.cache.retain_pair(objects,c.clock)?;let frames=[c.cache.lease_frame(&leases.0)?,c.cache.lease_frame(&leases.1)?];c.cache.with_leases(leases,|release|{release();Ok(())})?;
  let policy=c.timing.properties.map(|p|p.radius);let allowances=crate::ivp_hull_policy::Policy::default().allowances(c.timing.dt,std::array::from_fn(|i|crate::ivp_hull_policy::Body{radius:policy[i],linear:c.motion[i][0],angular:c.motion[i][1]}));
  Ok(Input{bodies:std::array::from_fn(|i|super::super::Body{prediction:c.bodies[i].prediction,prepared_pose:frames[i].pose,core_radius:policy[i],margin:c.margins[i]}),at:c.clock.time,margin:allowances[0]+allowances[1],provided:[None;2],restricted:[None;2]})
 }
 fn expand<'a>(&mut self,owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>)->Result<(),&'static str>{
  let c=&self.context;let (root,limit,fallback,motion,flags)=(c.root.clone(),c.limit,c.fallback,c.motion,c.bodies.map(|b|b.object_flags));
  //10121950 checks the count before asking the provider for caches/margins.
  // This unused packet keeps the rejected-count branch free of cache access.
  let input=if entry.expansion_plan(&root,limit,fallback)?.is_some(){self.input(entry.key.slots)?}else{let c=&self.context;Input{bodies:std::array::from_fn(|i|super::super::Body{prediction:c.bodies[i].prediction,prepared_pose:c.bodies[i].current,core_radius:c.timing.properties[i].radius,margin:c.margins[i]}),at:c.clock.time,margin:0.,provided:[None;2],restricted:[None;2]}};
  entry.expand_registered(owner,id,input,&root,limit,fallback,flags,motion,0.,|owner,key,child,convexes|self.initialize(owner,key,child,convexes),|_,_,_|Err("new recursive constructor tried to retire an existing child"))?;Ok(())
 }
 fn initialize<'a>(&mut self,owner:&mut ownership::Ownership,key:&Pair,entry:&mut Entry<'a>,convexes:[&Convex;2])->Result<(),&'static str>{
  let id=owner.graph.register(entry.key.slots)?;self.prefix.push(*key);self.bindings.push(Binding{path:self.prefix.clone(),id});
  let result=(||{
   let c=&mut self.context;owner.bind_objects(entry.key.slots,c.bodies.map(|b|b.id))?;let mut timing=c.timing;
   timing.budgets=std::array::from_fn(|i|{let b=owner.hulls[entry.key.slots[i]].budget;crate::ivp_distance_schedule::Budget{time:b.time,flags:c.bodies[i].object_flags,rates:b.rates,accumulated:b.accumulated}});
   let (_,registration)=entry.initialize_registered(&mut owner.graph,id,[false;2],c.cache,c.clock,c.bodies,convexes,c.gravity,&mut owner.queue,&mut owner.counter,timing,|_,_,_|Ok(()))?;
   match registration{
    Registration::Scheduled(out)=>{if let Some((keys,delta))=out.far{owner.track_far(id,keys,delta)?;}Ok(())},
    Registration::InvalidGeometry=>{if entry.recursive.is_some(){self.expand(owner,id,entry)}else{if owner.queue.scheduled(id).is_some(){owner.queue.cancel(id)?;}owner.graph.make_inactive(id)?;entry.contact.flags=owner.graph.flags(id)?;Ok(())}}
   }
  })();self.prefix.pop();result
 }
}
pub(crate) fn expand_new_tree<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,context:&mut Context<'_>)->Result<Vec<Binding>,&'static str>{
 if !entry.recursive.as_ref().ok_or("recursive constructor requires a parent")?.descendants.entries().is_empty(){return Err("new recursive constructor already has descendants");}
 let mut builder=Builder{context,prefix:vec![entry.key],bindings:vec![]};builder.expand(owner,id,entry)?;Ok(builder.bindings)
}
///1010cc80/1010bdf0 first queries the parent. Only the invalid-geometry
/// virtual slot expands it; a valid parent keeps its own event/far decision.
pub(crate) fn initialize_new_tree<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,convexes:[&Convex;2],context:&mut Context<'_>)->Result<Vec<Binding>,&'static str>{
 if !entry.recursive.as_ref().ok_or("recursive initialization requires a parent")?.descendants.entries().is_empty(){return Err("recursive initialization already has descendants");}
 owner.bind_objects(entry.key.slots,context.bodies.map(|b|b.id))?;
 let mut timing=context.timing;
 timing.budgets=std::array::from_fn(|i|{let b=owner.hulls[entry.key.slots[i]].budget;crate::ivp_distance_schedule::Budget{time:b.time,flags:context.bodies[i].object_flags,rates:b.rates,accumulated:b.accumulated}});
 let (_,out)=entry.initialize_registered(&mut owner.graph,id,[false;2],context.cache,context.clock,context.bodies,convexes,context.gravity,&mut owner.queue,&mut owner.counter,timing,|_,_,_|Ok(()))?;
 match out{
  Registration::Scheduled(out)=>{if let Some((keys,delta))=out.far{owner.track_far(id,keys,delta)?;}Ok(vec![])},
  Registration::InvalidGeometry=>expand_new_tree(owner,id,entry,context),
 }
}
///10121510 retains the stored split and reconciles the existing child vector.
/// It does not query, detach or rearm the parent;10121600 owns those phases.
pub(crate) fn update_tree<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,bindings:&mut Vec<Binding>,context:&mut Context<'_>,margin:f32)->Result<Option<Changes>,&'static str>{
 if owner.graph.objects(id)?!=entry.key.slots||owner.graph.flags(id)?!=entry.contact.flags||((entry.contact.flags>>18)&15)!=4||owner.far(id).is_none(){return Err("recursive update requires registered recursive far parent");}
 if !margin.is_finite()||margin<0.{return Err("invalid recursive update search margin");}
 let r=entry.recursive.as_ref().ok_or("recursive update requires a parent")?;
 let global=context.root.load(Ordering::Relaxed);let count=if global>0{global}else{r.count.load(Ordering::Relaxed)};
 if count>context.limit{return Ok(None);}
 let split=r.split.filter(|&s|s<2).ok_or("recursive update has no stored split")?;
 let mut builder=Builder{context,prefix:vec![entry.key],bindings:bindings.clone()};let mut input=builder.input(entry.key.slots)?;input.margin=margin;
 let mut provided=[None;2];let mut restricted=[None;2];provided[1-split]=Some(entry.key.ledges[1-split]);restricted[split]=Some(entry.key.ledges[split]);
 let root=builder.context.root.clone();let builder=std::cell::RefCell::new(&mut builder);let owner=std::cell::RefCell::new(owner);
 let out=entry.apply_plan(input,&root,Plan{split,provided,restricted},|key,child,convexes|builder.borrow_mut().initialize(&mut owner.borrow_mut(),key,child,convexes),|key,child|builder.borrow_mut().retire(&mut owner.borrow_mut(),key,child))?;
 *bindings=builder.borrow().bindings.clone();Ok(Some(out))
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum ParentCallback{Active,Tracked}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum FarRecheck{Rearmed,Activated}
///1010c8a0 normal far math also applies to a phase5 recursive parent.
/// Reactivation removes both old listeners before the ordinary base query,
/// whose invalid virtual branch can build a fresh recursive subtree.
pub(crate) fn recheck_parent<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,convexes:[&Convex;2],bindings:&mut Vec<Binding>,context:&mut Context<'_>,gap:f32)->Result<FarRecheck,&'static str>{
 if entry.recursive.as_ref().is_none_or(|r|!r.descendants.entries().is_empty())||!bindings.is_empty()||((entry.contact.flags>>18)&15)!=5||owner.graph.objects(id)?!=entry.key.slots||owner.graph.flags(id)?!=entry.contact.flags{return Err("recursive far recheck requires a detached childless phase5 parent");}
 let record=*owner.far(id).ok_or("recursive far parent lacks hull ownership")?;let g=entry.contact.query.geometry.ok_or("recursive far parent lacks geometry")?;
 let input=crate::ivp_far_distance::Input{flags:entry.contact.flags,now:context.clock.time,dt:context.timing.dt,gap,geometry:crate::ivp_far_distance::Geometry{projection:g.projection,linear_delta:record.linear_delta,distance:g.distance,normal:g.normal},cores:std::array::from_fn(|i|{let p=context.bodies[i].prediction;crate::ivp_far_distance::Core{time:p.time,start:p.start,velocity:p.velocity,linear:context.motion[i][0],angular:context.motion[i][1]}}),budgets:record.objects.map(|o|owner.hulls[o].budget)};
 match owner.recheck(id,&input,crate::ivp_far_distance::Config::default())?{
  crate::ivp_far_distance::Action::Rearm{geometry,..}=>{entry.contact.query.geometry=Some(crate::ivp_minimizer::Geometry{projection:geometry.projection,distance:geometry.distance,normal:geometry.normal});Ok(FarRecheck::Rearmed)},
  crate::ivp_far_distance::Action::Activate{pending:false}=>{*bindings=initialize_new_tree(owner,id,entry,convexes,context)?;Ok(FarRecheck::Activated)},
  crate::ivp_far_distance::Action::Activate{pending:true}=>Err("pending recursive far reactivation requires its distinct lifecycle"),
 }
}
/// Actual10121600 query, descendant collapse/update and paired hull rearm.
pub(crate) fn refresh_parent<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,convexes:[&Convex;2],bindings:&mut Vec<Binding>,context:&mut Context<'_>,threshold:f32)->Result<ParentCallback,&'static str>{
 if !threshold.is_finite()||threshold<0.||entry.recursive.is_none()||owner.graph.objects(id)?!=entry.key.slots||owner.graph.flags(id)?!=entry.contact.flags||((entry.contact.flags>>18)&15)!=4||owner.far(id).is_none(){return Err("recursive parent callback requires registered recursive hull ownership");}
 let [a,b]=convexes.map(|c|c.topology.as_deref().ok_or("recursive parent query lacks topology"));
 entry.contact.refresh_geometry_zero_budget(context.cache,context.clock,context.bodies,[a?,b?],convexes.map(|c|c.triangle_headers.as_slice()))?;
 owner.graph.set_flags(id,entry.contact.flags)?;
 if entry.contact.flags&0xc000==0&&entry.contact.query.geometry.ok_or("valid recursive parent lacks geometry")?.distance>threshold{
  clear_children(owner,entry,&mut vec![entry.key],bindings)?;bindings.clear();owner.release_far(id)?;owner.graph.activate(id,[false;2])?;
  entry.contact.flags=owner.graph.flags(id)?&!0x3000;owner.graph.set_flags(id,entry.contact.flags)?;return Ok(ParentCallback::Active);
 }
 let allowances=crate::ivp_hull_policy::Policy::default().allowances(context.timing.dt,std::array::from_fn(|i|crate::ivp_hull_policy::Body{radius:context.timing.properties[i].radius,linear:context.motion[i][0],angular:context.motion[i][1]}));
 update_tree(owner,id,entry,bindings,context,allowances[0]+allowances[1])?;
 let keys=std::array::from_fn(|i|owner.hulls[entry.key.slots[i]].budget.motion_at(context.clock.time)+allowances[i]);owner.rearm_recursive_far(id,keys)?;Ok(ParentCallback::Tracked)
}
fn clear_children<'a>(owner:&mut ownership::Ownership,entry:&mut Entry<'a>,prefix:&mut Vec<Pair>,bindings:&[Binding])->Result<(),&'static str>{
 if let Some(r)=entry.recursive.as_mut(){r.descendants.clear(|key,child|{prefix.push(*key);let result=(||{let id=bindings.iter().find(|b|b.path==*prefix).ok_or("retiring recursive child has no ownership identity")?.id;retire_at(owner,id,child,prefix,bindings)})();prefix.pop();result})?;}Ok(())
}
fn retire_at<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,prefix:&mut Vec<Pair>,bindings:&[Binding])->Result<(),&'static str>{
 clear_children(owner,entry,prefix,bindings)?;owner.retire(id)
}
pub(crate) fn retire_tree<'a>(owner:&mut ownership::Ownership,id:usize,entry:&mut Entry<'a>,bindings:&[Binding])->Result<(),&'static str>{
 retire_at(owner,id,entry,&mut vec![entry.key],bindings)
}

#[cfg(test)]
#[path="ivp_recursive_constructor_tests.rs"]
mod tests;
