//! Interactive runtime consumer of the recovered owner. Zero gravity and an
//! authored sphere contact isolate integration; map collision is not connected.
use bevy::prelude::*;
use crate::ivp_registered_contacts::ContactWorld;
pub(crate) const DT:f64=1./60.;
pub(crate) struct Scene{pub world:ContactWorld,pub steps:u64,pub joints:bool,accumulator:f64}
impl Scene{
 pub(crate) fn new()->Result<Self,&'static str>{
  let mut world=ContactWorld::empty_native(DT as f32,1,Vec3::new(0.,-0.,0.),Vec3::ZERO)?;
  for i in 0..2{world.create_sphere(i+1,crate::ivp_sphere_constructor::Parameters{radius:16.1,position:[32.*i as f32,0.,0.],angles:[0.;3],mass:10.,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:i==0,collisions:false},0.8)?;}
  world.create_poly(3,crate::ivp_poly_constructor::Parameters{surface:crate::ivp_poly_constructor::Surface{center:Vec3::from_array([1028658086,3159364525,3167753129].map(f32::from_bits)),unit_inertia:Vec3::from_array([1001756665,1008365033,1006681523].map(f32::from_bits)),radius:f32::from_bits(1045777259),factor:239},margin:0.,center:None,position:[64.,0.,0.],angles:[0.;3],mass:15.,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:false,collisions:false},0.8)?;
  world.set_velocity(1,Some(Vec3::new(-3.,0.,0.)),Some(Vec3::new(0.,10.,5.)))?;
  world.set_velocity(2,Some(Vec3::new(2.,0.,0.)),Some(Vec3::new(0.,20.,0.)))?;
  use crate::ivp_create::{Endpoint,Feature as Compact};use crate::ivp_managed_response::Feature;
  let features=std::array::from_fn(|i|Feature{endpoint:Endpoint{object:i as u64+1,feature:Compact{kind:3,edge:0,opposite:0,triangle:0,ledge:0,vertex:0,vertices:[Vec3::ZERO;3]}},vertices:[Vec3::ZERO;3],canonical:[Vec3::ZERO;3]});world.resolve(0xc0000,features,Some(1))?;
  let mut frames=[[[0.;4];3];2];for frame in &mut frames{for (i,row) in frame.iter_mut().enumerate(){row[i]=1.;}}frames[0][0][3]=-16.;frames[1][0][3]=16.;
  let p=crate::ivp_ragdoll_constructor::Parameters{frames,axes:[crate::ivp_ragdoll_constructor::Limit{min:-20.,max:20.,velocity:0.,torque:0.};3],force_limit:0.,torque_limit:0.,mass_scale:[1.;2],angular_only:false,clockwise:true};
  let offsets=world.object_offsets().clone();let constraints=(0..2).map(|i|world.ragdoll_definition(i,[i+1,i],p,&offsets)).collect::<Result<Vec<_>,_>>()?;
  world.register_joint_group(0,crate::ivp_registered_physical::GroupDefinition{constraints,offsets,additional:0,minimum:3,tolerance:3.,rescue_gravity:Vec3::ZERO})?;
  Ok(Self{world,steps:0,joints:true,accumulator:0.})
 }
 pub(crate) fn frame(&mut self,delta:f64,paused:bool)->Result<usize,&'static str>{
  if !delta.is_finite()||delta<0.{return Err("invalid runtime frame delta");}if paused{return Ok(0);}
  self.accumulator+=delta;let mut count=0;while self.accumulator+1e-12>=DT&&count<8{self.world.advance(DT as f32)?;self.world.take_releases();self.accumulator=(self.accumulator-DT).max(0.);self.steps+=1;count+=1;}Ok(count)
 }
 pub(crate) fn kick(&mut self)->Result<(),&'static str>{
  let point=(self.world.bodies()[1].current.position+Vec3::new(0.1,0.,0.05)).to_array();self.world.apply_impulses(&[crate::ivp_joint_tail::Command::PointImpulse{body:1,point,impulse:[0.,-8.,1.]}])
 }
 pub(crate) fn release(&mut self)->Result<(),&'static str>{
  if !self.joints{return Ok(());}let mut world=self.world.clone();world.detach_joint_group(0)?;for id in 0..2{world.destroy_detached_joint(id)?;}self.world=world;self.joints=false;Ok(())
 }
 pub(crate) fn poses(&self)->Vec<(Vec3,Quat)>{
  let basis=Mat3::from_diagonal(Vec3::new(1.,-1.,-1.));self.world.bodies().iter().map(|b|{let p=b.prediction.sample_physical_core(self.world.clock().now,(b.flags>>8) as u8,b.current).pose;(basis*p.position,Quat::from_mat3(&(basis*p.rotation*basis)).normalize())}).collect()
 }
}
#[cfg(test)]mod tests{
 use super::*;
 fn snapshot(s:&Scene)->serde_json::Value{serde_json::json!({"bodies":s.world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>(),"contacts":s.world.contacts().world_snapshot(),"clock":format!("{:?}",s.world.clock()),"steps":s.steps})}
 #[test]fn frame_rate_pause_and_backlog_preserve_runtime_physics(){let mut a=Scene::new().unwrap();let mut b=Scene::new().unwrap();for _ in 0..144{a.frame(1./144.,false).unwrap();}for _ in 0..60{b.frame(DT,false).unwrap();}assert_eq!(a.steps,60);assert_eq!(snapshot(&a),snapshot(&b));let before=snapshot(&a);a.frame(10.,true).unwrap();assert_eq!(snapshot(&a),before);let mut c=Scene::new().unwrap();c.frame(1.,false).unwrap();while c.steps<60{c.frame(0.,false).unwrap();}assert_eq!(snapshot(&b),snapshot(&c));}
 #[test]fn runtime_controls_keep_numeric_scene_finite_and_resettable(){let mut scene=Scene::new().unwrap();scene.frame(DT,false).unwrap();let before=snapshot(&scene);scene.kick().unwrap();assert_ne!(snapshot(&scene),before);for _ in 0..120{scene.frame(DT,false).unwrap();}scene.release().unwrap();assert!(scene.world.joint_group(0).is_none());scene.kick().unwrap();for _ in 0..120{scene.frame(DT,false).unwrap();}assert!(scene.poses().iter().all(|(p,q)|p.is_finite()&&q.is_finite()));assert_eq!(snapshot(&Scene::new().unwrap()),snapshot(&Scene::new().unwrap()));}
}
