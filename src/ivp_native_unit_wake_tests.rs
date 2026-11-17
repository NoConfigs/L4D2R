use super::*;
use crate::ivp_surface_admission::contacts::ownership::{Ownership,controllers::Controller};
use crate::ivp_joint_apply::tests::snapshot;
#[test]
fn native_whole_unit_wakes_preserve_per_member_callback_timing_and_mixed_entries(){
 let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unit-wake-notify.json")).unwrap();let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);let mut warm=0;
 for c in d["cases"].as_array().unwrap(){
  let mut world=ContactWorld::empty_native(f(&c["dt"]),123,Vec3::ZERO,Vec3::ZERO).unwrap();
  for (id,p) in c["descriptions"].as_array().unwrap().iter().enumerate(){world.create_sphere(id as u64+1,crate::ivp_sphere_constructor::Parameters{radius:f(&c["radius"]),position:std::array::from_fn(|i|f(&p["position"][i])),angles:std::array::from_fn(|i|f(&c["angles"][i])),mass:f(&p["mass"]),inertia_scale:1.,damping:0.,angular_damping:0.,rotational_limit:0.1,fixed:false,collisions:false},0.8).unwrap();}
  let prefix=f(&c["prefix"]);if prefix>0.{world.advance(prefix).unwrap();warm+=1;}
  let group_order=c["group_order"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();world.state.response.register_physical_group_in_candidate(77,&group_order).unwrap();let now=world.clock.now;let deadline=world.clock.deadline;world.state.effects(now,deadline).unwrap();
  let mut compact=Ownership::new(4,8,8,0.).unwrap();for id in 1..4{compact.bind_objects([0,id],[0,id]).unwrap();}let mut controller=Controller::new(0,0.1,true).unwrap();controller.track_cores([(1,1),(2,2),(3,3)]).unwrap();for channel in 0..4{for id in 0..3{controller.listen(channel,id).unwrap();}}compact.controllers.insert(controller).unwrap();
  let peer=c["peer"].as_u64().unwrap() as usize;
  for pair in 0..3{let object=1+pair;let objects=if pair%2==0{[0,object]}else{[object,0]};compact.change_controller_contact(0,pair,objects,true,world.state.response.physical_bodies_mut().unwrap(),|_,_,_|Ok(())).unwrap();compact.change_controller_contact(0,pair,objects,false,world.state.response.physical_bodies_mut().unwrap(),|_,_,_|Ok(())).unwrap();}
  if c["prewake"]==true{let now=world.clock.now;let deadline=world.clock.deadline;CollisionContext{state:&mut world.state,now,deadline}.wake_impact_core_with_compact(peer,&mut compact,|_,_,_,_|Ok(())).unwrap();}
  assert_eq!(serde_json::json!(world.registry().unit_cores(peer).unwrap()),c["unit_members"]);assert_eq!(world.registry().unit_flags(peer).unwrap().flags as u8,8);
  assert_eq!(serde_json::json!(world.bodies().iter().copied().map(snapshot).collect::<Vec<_>>()),c["before"],"case{} before",c["id"]);assert_eq!(world.clock.now.to_bits(),c["now"].as_u64().unwrap());assert_eq!(world.clock.deadline.to_bits(),c["deadline"].as_u64().unwrap());
  let now=world.clock.now;let deadline=world.clock.deadline;let mut context=CollisionContext{state:&mut world.state,now,deadline};let mut events=vec![];
  let expected=c["unit_members"].as_array().unwrap().iter().rev().map(|v|v.as_u64().unwrap() as usize).filter(|&core|!(c["prewake"]==true&&core==peer)).collect::<Vec<_>>();
  assert_eq!(context.wake_impact_unit_with_compact(peer,&mut compact,|_,state,e,bodies|{let core=e.subject;let mut event=vec![e.channel,e.listener,core,usize::from(e.add),state.count(),state.peer_count(core) as usize,state.core_count(core) as usize,usize::from(state.aliased(core)),bodies[core].object_flags as usize];event.extend(bodies.iter().map(|b|b.object_flags as usize));events.push(event);Ok(())}).unwrap(),expected);
  assert!(context.wake_impact_unit_with_compact(peer,&mut compact,|_,_,_,_|panic!("repeated whole-unit alias exit")).unwrap().is_empty());
  assert_eq!(serde_json::json!(events),c["events"],"case{} events",c["id"]);assert_eq!(serde_json::json!(world.bodies().iter().copied().map(snapshot).collect::<Vec<_>>()),c["after"],"case{} after",c["id"]);
  assert_eq!(world.registry().snapshot()["active"][0]["cores"],c["unit_members"]);assert_eq!(world.registry().unit_flags(peer).unwrap().flags as u8,1);
  for core in 1..4{assert_eq!(compact.controllers.get(0).unwrap().aliased(core),c["aliases"][core-1].as_bool().unwrap());}
 }
 assert_eq!(warm,48);assert_eq!(d["cases"].as_array().unwrap().len(),96);
}
