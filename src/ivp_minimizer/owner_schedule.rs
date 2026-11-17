//! Compact contact -> native scheduling gates -> typed timing -> queue entry.
//! Pair discovery, global graph membership and far-list edits remain with callers.
use super::*;
use crate::{ivp_distance_schedule as schedule,ivp_feature_handler as timing,ivp_event_queue::Queue};
#[derive(Clone,Copy)]
pub(crate) struct Properties{pub radius:f32,pub inverse_diameter:f32,pub angular_speed:f32,pub angular_bound:f32,pub sweep_axis:Vec3}
#[derive(Clone,Copy)]
pub(crate) struct Schedule{pub deadline:f64,pub dt:f32,pub delay:u8,pub config:schedule::Config,pub face_epsilon:f32,pub properties:[Properties;2],pub budgets:[schedule::Budget;2],pub allow_far:bool}
impl Contact{
 pub(crate) fn schedule(&mut self,cache:&mut WorldCache,clock:Context,bodies:&[Body;2],hulls:[&CompactTopology;2],queue:&mut Queue,owner:usize,counter:&mut u32,i:Schedule)->Result<schedule::queue::Outcome,&'static str>{
    if self.query.frame!=clock.frame||self.query.retirement_requested()||self.flags&0xc000!=0{return Err("compact scheduling requires current ordinary geometry");}
    let geometry=self.query.geometry.ok_or("compact scheduling requires defined geometry")?;
    let edges=[self.query.vertex,self.query.face];let features=std::array::from_fn::<_,2,_>(|j|match self.query.kinds[j]{0=>Ok(timing::dispatch::Feature::Point(hulls[j],edges[j])),1=>Ok(timing::dispatch::Feature::Edge(hulls[j],edges[j])),2=>Ok(timing::dispatch::Feature::Face(hulls[j],edges[j])),_=>Err("compact owner has unsupported feature kind")});let features=[features[0]?,features[1]?];
    for j in 0..2{
      if cache.object_core(self.objects[j])?!=bodies[j].id{return Err("compact scheduling cache/core binding differs");}
      cache.update_core(bodies[j].id,bodies[j].prediction)?;cache.set_flags(self.objects[j],bodies[j].object_flags)?;
    }
    let linear=bodies.map(|b|crate::ivp_bounds::speed(Vec3::from_array(b.linear[..3].try_into().unwrap())));
    let input=schedule::Input{flags:self.flags,counter:*counter,now:clock.time,deadline:i.deadline,dt:i.dt,distance:geometry.distance,target:i.config.target(self.flags)?,normal:geometry.normal,cores:std::array::from_fn(|j|schedule::Core{motion:crate::ivp_bounds::Motion{axis:i.properties[j].sweep_axis,angular_bound:i.properties[j].angular_bound,velocity:Vec3::from_array(bodies[j].linear[..3].try_into().unwrap())},linear:linear[j]}),budgets:i.budgets,allow_far:i.allow_far,slow_gate:i.config.slow_gate};
    let result=schedule::queue::run(queue,owner,input,i.delay,i.config,|request|{
      let route=timing::dispatch::route(features.map(|f|f.kind()),request.flags)?;
      let descriptors=[cache.object_inputs(self.objects[0])?,cache.object_inputs(self.objects[1])?];
      // Native gates return before retaining caches. Compact wrappers retain
      // their preferred endpoint first, and release after the handler returns.
      let leases=cache.retain_pair(route.order.map(|j|self.objects[j]),clock)?;
      let frames=cache.lease_frame(&leases.0).and_then(|a|cache.lease_frame(&leases.1).map(|b|{let ordered=[a,b];route.order.map(|j|ordered[j])}));
      cache.with_leases(leases,|release|{
        let frames=frames?;
        let result=timing::dispatch::run(timing::dispatch::Input{sides:std::array::from_fn(|j|timing::dispatch::Side{feature:features[j],body:timing::Body{prediction:descriptors[j].0,frame:descriptors[j].1,flags:descriptors[j].2,cached:frames[j].pose,angular_speed:i.properties[j].angular_speed,inverse_diameter:i.properties[j].inverse_diameter},radius:i.properties[j].radius,linear_speed:linear[j],angular_radius:i.properties[j].angular_bound}),flags:request.flags,now:request.now,deadline:request.deadline,bound:request.bound,total_bound:request.total_bound,margin:self.margin,distance:request.distance,target:request.target,normal:geometry.normal,gap:i.config.gap,face_epsilon:i.face_epsilon});
        release();let result=result?.outcome;Ok(schedule::queue::Candidate{kind:result.kind,time:result.candidate})
      })
    })?;
    self.flags=result.flags;*counter=result.counter;Ok(result)
 }
}
#[cfg(test)]
#[path="owner_schedule_tests.rs"]
mod tests;
