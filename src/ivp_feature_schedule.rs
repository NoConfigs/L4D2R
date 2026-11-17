//!1010b840 queue ownership around source feature-time dispatch. The caller
//! supplies geometry/body inputs, not a recorded candidate/event timeline.
use crate::{ivp_distance_schedule as schedule,ivp_event_queue::Queue};
#[derive(Clone,Copy)]
pub(crate) struct Request{pub flags:u32,pub now:f64,pub deadline:f64,pub distance:f32,pub target:f32,pub bound:f32,pub total_bound:f32}
pub(crate) struct Candidate{pub kind:u32,pub time:Option<f64>}
pub(crate) struct Outcome{pub flags:u32,pub counter:u32,pub kind:u32,pub candidate:Option<f64>,pub handle:Option<u16>,pub far:Option<([f32;2],f32)>}
pub(crate) fn run(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,feature:impl FnOnce(Request)->Result<Candidate,&'static str>)->Result<Outcome,&'static str>{
    let mut next=queue.clone();if next.scheduled(owner).is_some(){next.cancel(owner)?;}
    let mut decision=schedule::prepare(&input)?;let mut far=None;
    let candidate=match decision.action{
        schedule::Action::Skip=>Candidate{kind:0,time:None},
        schedule::Action::Hull{keys,linear_delta}=>{far=Some((keys,linear_delta));Candidate{kind:0,time:None}},
        schedule::Action::Feature{closing_bound,total_bound}=>feature(Request{flags:decision.flags,now:input.now,deadline:input.deadline,distance:input.distance,target:input.target,bound:closing_bound,total_bound})?,
    };
    let offset=schedule::complete(&input,&mut decision,candidate.kind,candidate.time,delay,config.time_scale,next.epoch)?;
    let handle=offset.map(|key|next.insert(owner,key)).transpose()?;*queue=next;
    Ok(Outcome{flags:decision.flags,counter:decision.counter,kind:candidate.kind,candidate:candidate.time,handle,far})
}
/// Complete vertex/face timing and queue admission. The scheduling decision
/// owns bound/target/time; stale copies in the geometry packet cannot override it.
pub(crate) fn vertex_face(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::Input<'_>)->Result<Outcome,&'static str>{
    run(queue,owner,input,delay,config,|r|{
        geometry.now=r.now;geometry.deadline=r.deadline;geometry.bound=r.bound;geometry.distance=r.distance;geometry.target=r.target;
        let result=crate::ivp_feature_handler::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})
    })
}

pub(crate) fn point_pair(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::point_pair::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.total_bound=r.total_bound;let result=crate::ivp_feature_handler::point_pair::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}
/// The scene supplies the independent adjacent-plane epsilon. The scheduling
/// profile supplies the escape gap and the total/directional motion bounds.
pub(crate) fn point_edge(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::point_edge::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.total_bound=r.total_bound;geometry.gap=config.gap;let result=crate::ivp_feature_handler::point_edge::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}
pub(crate) fn edge_edge(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::edge_edge::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.gap=config.gap;let result=crate::ivp_feature_handler::edge_edge::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}

/// Sphere first, compact point second. Bounds/time/target and escape settings
/// come from the scheduling profile rather than geometry or expected events.
pub(crate) fn sphere_point(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::sphere_point::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.total_bound=r.total_bound;geometry.gap=config.gap;let result=crate::ivp_feature_handler::sphere_point::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}

pub(crate) fn sphere_edge(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::sphere_edge::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.total_bound=r.total_bound;geometry.gap=config.gap;let result=crate::ivp_feature_handler::sphere_edge::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}
pub(crate) fn sphere_face(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::sphere_face::Input<'_>)->Result<Outcome,&'static str>{
 run(queue,owner,input,delay,config,|r|{geometry.features.now=r.now;geometry.features.deadline=r.deadline;geometry.features.bound=r.bound;geometry.features.distance=r.distance;geometry.features.target=r.target;geometry.gap=config.gap;let result=crate::ivp_feature_handler::sphere_face::run(geometry)?;Ok(Candidate{kind:result.kind,time:result.candidate})})
}

/// Native selected-kind dispatch inside the same transactional queue owner.
/// Contact geometry owns features/properties; scheduling owns dynamic settings.
pub(crate) fn feature(queue:&mut Queue,owner:usize,input:schedule::Input,delay:u8,config:schedule::Config,mut geometry:crate::ivp_feature_handler::dispatch::Input<'_>)->Result<Outcome,&'static str>{
 let normal=input.normal;
 run(queue,owner,input,delay,config,|r|{geometry.flags=r.flags;geometry.now=r.now;geometry.deadline=r.deadline;geometry.bound=r.bound;geometry.total_bound=r.total_bound;geometry.distance=r.distance;geometry.target=r.target;geometry.normal=normal;geometry.gap=config.gap;let result=crate::ivp_feature_handler::dispatch::run(geometry)?.outcome;Ok(Candidate{kind:result.kind,time:result.candidate})})
}
