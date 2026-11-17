//! Initialized1018def0 dispatch plus10116b70/10116980/10116ac0 role rules.
//! Features and body properties are live inputs; no expected route or event is read.
use super::*;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u8)]
pub(crate) enum Kind{Point=0,Edge=1,Face=2,Sphere=3}
#[derive(Clone,Copy)]
pub(crate) enum Feature<'a>{Point(&'a CompactTopology,CompactEdgeId),Edge(&'a CompactTopology,CompactEdgeId),Face(&'a CompactTopology,CompactEdgeId),Sphere}
impl Feature<'_>{
 pub(crate) fn kind(self)->Kind{match self{Self::Point(..)=>Kind::Point,Self::Edge(..)=>Kind::Edge,Self::Face(..)=>Kind::Face,Self::Sphere=>Kind::Sphere}}
 fn edge(self)->Option<CompactEdgeId>{match self{Self::Point(_,e)|Self::Edge(_,e)|Self::Face(_,e)=>Some(e),Self::Sphere=>None}}
}
impl<'a> Feature<'a>{fn topology(self)->Option<&'a CompactTopology>{match self{Self::Point(h,_)|Self::Edge(h,_)|Self::Face(h,_)=>Some(h),Self::Sphere=>None}}}
#[derive(Clone,Copy)]
pub(crate) struct Side<'a>{pub feature:Feature<'a>,pub body:Body,pub radius:f32,pub linear_speed:f32,pub angular_radius:f32}
pub(crate) struct Input<'a>{pub sides:[Side<'a>;2],pub flags:u32,pub now:f64,pub deadline:f64,pub bound:f32,pub total_bound:f32,pub margin:f32,pub distance:f32,pub target:f32,pub normal:Vec3,pub gap:f32,pub face_epsilon:f32}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct Route{pub wrapper:u32,pub handler:u32,pub order:[usize;2]}
pub(crate) fn route(kinds:[Kind;2],flags:u32)->Result<Route,&'static str>{
 use Kind::*;
 let selected=((flags>>8)&3) as usize;if selected>1{return Err("invalid native preferred endpoint");}let order=[selected,1-selected];
 if kinds==[Sphere,Sphere]{return Ok(Route{wrapper:0x116ac0,handler:0x1157a0,order:[0,1]});}
 if kinds[order[0]]==Sphere{
    if selected!=0{return Err("native mixed wrapper requires sphere in physical endpoint zero");}
    return Ok(Route{wrapper:0x116980,handler:match kinds[1]{Point=>0x115370,Edge=>0x116340,Face=>0x114660,Sphere=>unreachable!()},order:[0,1]});
 }
 if kinds.contains(&Sphere){return Err("native table rejects selected compact/sphere combination");}
 let handler=match order.map(|i|kinds[i]){[Point,Point]=>0x114dc0,[Point,Edge]=>0x115910,[Point,Face]=>0x1141b0,[Edge,Edge]=>0x1147e0,_=>return Err("native dispatch rejects selected compact feature combination")};
 Ok(Route{wrapper:0x116b70,handler,order})
}
pub(crate) struct Routed{pub route:Route,pub outcome:Outcome}
pub(crate) fn run(i:Input<'_>)->Result<Routed,&'static str>{
 let route=route(i.sides.map(|s|s.feature.kind()),i.flags)?;
 if ![i.gap,i.face_epsilon].iter().all(|v|v.is_finite()&&*v>=0.){return Err("invalid native feature settings");}
 let s=route.order.map(|j|i.sides[j]);
 let outcome=if route.handler==0x1157a0{
    let bodies=s.map(|s|s.body);let candidate=crate::ivp_time::sphere_candidate(crate::ivp_time::SphereCandidate{now:i.now,deadline:i.deadline,margin:i.margin,target:i.target,gap:i.gap,bound:i.bound,initial:bodies.map(|b|b.cached),inactive:bodies.map(|b|(b.flags as u8 as i8)>7)},|side,at|bodies[side].prediction.sample(at,bodies[side].frame));
    Outcome{kind:if candidate.is_some(){16}else{0},candidate,searches:Vec::new()}
 }else{
    // Mixed handlers never read a compact feature for the sphere. Their shared
    // low-level packet uses the other topology in that unused slot; the typed
    // public input does not require a fictitious sphere ledge or edge.
    let other=s[1].feature.topology().ok_or("missing compact timing feature")?;
    let geometry=super::Input{hulls:s.map(|v|v.feature.topology().unwrap_or(other)),edges:s.map(|v|v.feature.edge().unwrap_or(CompactEdgeId{triangle:0,edge:0})),bodies:s.map(|v|v.body),now:i.now,deadline:i.deadline,bound:i.bound,margin:i.margin,distance:i.distance,target:i.target};
    match route.handler{
      0x114dc0=>point_pair::run_with_gap(point_pair::Input{features:geometry,normal:i.normal,total_bound:i.total_bound,radii:s.map(|v|v.radius),linear_speeds:s.map(|v|v.linear_speed)},i.gap)?,
      0x115910=>point_edge::run(point_edge::Input{features:geometry,total_bound:i.total_bound,linear_speeds:s.map(|v|v.linear_speed),edge_angular_bound:s[1].angular_radius,gap:i.gap,face_epsilon:i.face_epsilon})?,
      0x1141b0=>super::run_with_settings(geometry,i.gap,i.face_epsilon)?,
      0x1147e0=>edge_edge::run(edge_edge::Input{features:geometry,normal:i.normal,gap:i.gap,face_epsilon:i.face_epsilon})?,
      0x115370=>sphere_point::run(sphere_point::Input{features:geometry,normal:i.normal,total_bound:i.total_bound,linear_speeds:s.map(|v|v.linear_speed),gap:i.gap})?,
      0x116340=>sphere_edge::run(sphere_edge::Input{features:geometry,total_bound:i.total_bound,gap:i.gap,face_epsilon:i.face_epsilon})?,
      0x114660=>sphere_face::run(sphere_face::Input{features:geometry,gap:i.gap})?,
      _=>return Err("unimplemented native feature handler"),
    }
 };Ok(Routed{route,outcome})
}
#[cfg(test)]
#[path="ivp_feature_dispatch_tests.rs"]
mod tests;
