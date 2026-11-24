//! Native compiled-box registration/discovery/wake comparisons. Both isolated
//! admission controls and complete source spatial-discovery paths are retained.
use super::*;
use crate::ivp_surface_admission::contacts::{Entry,ownership::Ownership};
use crate::ivp_minimizer::owner::Contact;
use crate::ivp_world_cache::{WorldCache,Context as CacheClock};
use serde_json::{Value,json};
use crate::ivp_surface_admission::contacts::discovery::{Discovery,Object as DiscoveryObject,Lifecycle};
#[derive(Clone,Copy,PartialEq,Eq)]
enum Cases{Startup,Epochs,Velocity,Impact,Dispatch,Prediction,Far,Collision,OwnedCollision,OwnedImpactWake,OwnedSleep,OwnedRewake,OwnedRest,OwnedMaterial,OwnedLiveMaterial,OwnedLiveRegistry,OwnedObjectMaterial,OwnedMaterialAssign}
#[test]
fn transactional_compact_owner_matches_native_live_surface_registry_updates(){check(true,Cases::OwnedLiveRegistry);}
#[test]
fn transactional_compact_owner_matches_native_object_default_material_updates(){check(true,Cases::OwnedObjectMaterial);}
#[test]
fn transactional_compact_owner_matches_native_runtime_material_assignments(){check(true,Cases::OwnedMaterialAssign);}
struct Constructor<'a>{owner:&'a mut Ownership,queue:&'a mut crate::ivp_event_queue::Queue,clock:CacheClock,dt:f32,deadline:f64,radius:f32,gravity:[f32;3]}
impl Constructor<'_>{
 fn initialize(&mut self,id:usize,entry:&mut Entry<'_>,convexes:[&l4d2_formats::phy::Convex;2],budgets:[crate::ivp_hull_budget::Budget;2],properties:[crate::ivp_minimizer::owner::Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Option<crate::ivp_surface_admission::contacts::discovery::FarRegistration>,&'static str>{
  let timing=crate::ivp_minimizer::owner::Schedule{deadline:self.deadline,dt:self.dt,delay:0,config:crate::ivp_distance_schedule::Config::bare_native(),face_epsilon:f32::from_bits(975594773),properties,budgets:std::array::from_fn(|i|crate::ivp_distance_schedule::Budget{time:budgets[i].time,flags:bodies[i].object_flags,rates:budgets[i].rates,accumulated:budgets[i].accumulated}),allow_far:false};
  self.owner.bind_objects(entry.key.slots,bodies.map(|b|b.id))?;
  self.owner.with_queue(self.queue,|owner|{
   let (_,result)=entry.initialize_registered(&mut owner.graph,id,[false;2],cache,self.clock,bodies,convexes,self.gravity,&mut owner.queue,&mut owner.counter,timing,|entry,graph,queue|{
    if queue.scheduled(id).is_some(){queue.cancel(id)?;}graph.make_inactive(id)?;entry.contact.flags=graph.flags(id)?;Ok(())
   })?;
   Ok(match result{crate::ivp_surface_admission::contacts::Registration::Scheduled(out)=>out.far,crate::ivp_surface_admission::contacts::Registration::InvalidGeometry=>None})
  })
 }
}
impl Lifecycle for Constructor<'_>{
 fn created(&mut self,entry:&mut Entry<'_>,convexes:[&l4d2_formats::phy::Convex;2],input:crate::ivp_surface_admission::contacts::discovery::CreationInput,bodies:&mut[Body;2],cache:&mut WorldCache)->Result<crate::ivp_surface_admission::contacts::discovery::Created,&'static str>{
  let id=self.owner.graph.register(entry.key.slots)?;let far=self.initialize(id,entry,convexes,input.budgets,input.properties,bodies,cache)?;Ok(crate::ivp_surface_admission::contacts::discovery::Created{id,far,descendants:vec![]})
 }
 fn reactivated(&mut self,id:usize,entry:&mut Entry<'_>,convexes:[&l4d2_formats::phy::Convex;2],budgets:[crate::ivp_hull_budget::Budget;2],motion:[crate::ivp_minimizer::owner::Properties;2],bodies:&mut[Body;2],cache:&mut WorldCache)->Result<Option<crate::ivp_surface_admission::contacts::discovery::FarRegistration>,&'static str>{self.initialize(id,entry,convexes,budgets,motion,bodies,cache)}
 fn retired(&mut self,id:usize,_:&mut Entry<'_>,_:&[crate::ivp_surface_admission::contacts::recursion::construction::Binding])->Result<(),&'static str>{self.owner.with_queue(self.queue,|owner|owner.retire(id))}
}

fn f(v:&Value)->f32{f32::from_bits(v.as_u64().unwrap() as u32)}
fn v(v:&Value)->Vec3{Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
fn bank(v:&Value)->Value{json!(v.as_array().unwrap().iter().enumerate().map(|(i,p)|{let mut b=p[0].clone();let mut other=p[1].clone();b["id"]=json!(i);other["id"]=json!(i);assert_eq!(b,other);b}).collect::<Vec<_>>())}

#[test]
fn native_compiled_box_contacts_merge_wake_units_and_notify_after_admission(){
 check(false,Cases::Startup);
}
#[test]
fn native_compiled_box_wakes_discover_contacts_through_the_spatial_tree(){
 check(true,Cases::Startup);
}
#[test]
fn native_late_compiled_box_registration_and_wakes_preserve_nonzero_environment_clocks(){check(true,Cases::Epochs);}
#[test]
fn native_compact_wakes_preserve_queued_public_linear_and_angular_velocity(){check(true,Cases::Velocity);}
#[test]
fn native_compact_impact_staging_response_and_prediction_rebuild_match(){check(true,Cases::Impact);}
#[test]
fn native_compact_timed_callback_gates_and_retry_scheduling_match(){check(true,Cases::Dispatch);}
#[test]
fn native_anisotropic_compact_prediction_uses_original_gyro_deltas_and_actual_ticks(){check(true,Cases::Prediction);}
#[test]
fn native_separating_compact_contacts_share_the_spatial_hull_world(){check(true,Cases::Far);}
#[test]
fn native_compact_collision_trajectories_use_natural_world_dispatch(){check(true,Cases::Collision);}
#[test]
fn transactional_compact_owner_matches_native_dispatch_and_rolls_back_every_mutable_bank(){check(true,Cases::OwnedCollision);}
#[test]
fn transactional_compact_owner_wakes_sleeping_native_targets_during_impact(){check(true,Cases::OwnedImpactWake);}
#[test]
fn transactional_compact_owner_matches_twelve_native_simulation_calls(){check(true,Cases::OwnedSleep);}
#[test]
fn transactional_compact_owner_matches_native_public_rewake(){check(true,Cases::OwnedRewake);}
#[test]
fn transactional_compact_owner_matches_native_gravity_stacks(){check(true,Cases::OwnedRest);}
#[test]
fn transactional_compact_owner_matches_native_triangle_material_stacks(){check(true,Cases::OwnedMaterial);}
#[test]
fn transactional_compact_owner_matches_native_live_world_material_remaps(){check(true,Cases::OwnedLiveMaterial);}
fn check(discover:bool,cases:Cases){
 let material_systems:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-compact-material-systems.json")).unwrap();
 let system_bank=|w:&crate::ivp_registered_contacts::ContactWorld|json!((1..=3).map(|object|w.contacts().material_system_snapshot(object)).collect::<Vec<_>>());
 let rest=matches!(cases,Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign);let materials=matches!(cases,Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign);let dispatch=cases==Cases::Dispatch;let impact=matches!(cases,Cases::Impact|Cases::Dispatch);let warm=cases!=Cases::Startup;let motion=matches!(cases,Cases::Velocity|Cases::Impact|Cases::Dispatch|Cases::Prediction|Cases::Far|Cases::Collision|Cases::OwnedCollision|Cases::OwnedImpactWake|Cases::OwnedSleep|Cases::OwnedRewake|Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign);
 let data:Value=serde_json::from_str(match cases{Cases::OwnedLiveMaterial=>include_str!("../tools/fixtures/native-ivp-compact-live-material.json"),Cases::OwnedMaterialAssign=>include_str!("../tools/fixtures/native-ivp-compact-material-assign.json"),Cases::OwnedObjectMaterial=>include_str!("../tools/fixtures/native-ivp-compact-object-material.json"),Cases::OwnedLiveRegistry=>include_str!("../tools/fixtures/native-ivp-compact-live-registry.json"),Cases::OwnedMaterial=>include_str!("../tools/fixtures/native-ivp-compact-material-stack.json"),Cases::OwnedRest=>include_str!("../tools/fixtures/native-ivp-compact-rest.json"),Cases::OwnedRewake=>include_str!("../tools/fixtures/native-ivp-compact-rewake.json"),Cases::OwnedSleep=>include_str!("../tools/fixtures/native-ivp-compact-sleep.json"),Cases::OwnedImpactWake=>include_str!("../tools/fixtures/native-ivp-compact-impact-wake.json"),Cases::OwnedCollision=>include_str!("../tools/fixtures/native-ivp-compact-collision.json"),Cases::Collision=>include_str!("../tools/fixtures/native-ivp-compact-collision.json"),Cases::Far=>include_str!("../tools/fixtures/native-ivp-compact-far-world.json"),Cases::Prediction=>include_str!("../tools/fixtures/native-ivp-compact-prediction.json"),Cases::Dispatch=>include_str!("../tools/fixtures/native-ivp-compact-postimpact.json"),Cases::Impact=>include_str!("../tools/fixtures/native-ivp-compact-impact.json"),Cases::Velocity=>include_str!("../tools/fixtures/native-ivp-compact-wake-motion.json"),Cases::Epochs=>include_str!("../tools/fixtures/native-ivp-compact-wake-epochs.json"),Cases::Startup=>include_str!("../tools/fixtures/native-ivp-compact-wake-neighbour.json")}).unwrap();
 // Explicit diagnostic override exercises the pending public compound target;
 // ordinary committed fixtures and their assertions remain the default suite.
 let compound_input=(cases==Cases::OwnedMaterialAssign).then(||std::env::var_os("L4D2R_COMPOUND_WORLD_INPUT")).flatten();
 let data=if let Some(path)=&compound_input{let text=std::fs::read_to_string(path).unwrap();let start=text.find("{\"material_texts\"").expect("native compound JSON header");serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>().next().unwrap().unwrap()}else{data};
 let compound=compound_input.is_some();
 let hex=data["shape"].as_str().unwrap();let bytes=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();
 let mut serialized=(bytes.len() as u32).to_le_bytes().to_vec();serialized.extend(bytes);
 let solids=l4d2_formats::phy::parse_collision(&serialized,1).unwrap();if compound{assert!(solids[0].convexes.len()>1);}else{assert_eq!(solids[0].convexes.len(),1);}let mut merged=0;let mut branches=[0usize;3];let mut original_delta_distinctions=0usize;let mut collisions=0usize;let mut target_wakes=0usize;
 let shared_material_table=crate::ivp_material_backend::WorldMaterialTable::default();
 for case in data["cases"].as_array().unwrap(){
  let mut material_solid=solids[0].clone();
  if materials{let codes=case["triangle_codes"].as_array().unwrap();assert_eq!(codes.len(),material_solid.convexes[0].triangle_headers.len());for (header,code) in material_solid.convexes[0].triangle_headers.iter_mut().zip(codes){header.set_material_index(code.as_u64().unwrap() as i32);}}
  let convex=&material_solid.convexes[0];
  let dt=f(&case["dt"]);let seed=case["poll_seed"].as_u64().unwrap_or(123) as u32;
  let gravity=if case["gravity_input"].is_null(){Vec3::ZERO}else{let v=v(&case["gravity_input"]);let unit=f32::from_bits(0x3cd013a9);Vec3::new(v.x*unit,-(v.z*unit),v.y*unit)};
  // Public force gravity belongs to the base controller. The environment's
  // separate recovery vector retains its default even after SetGravity.
  let rescue_gravity=if rest{[0.,9.83f32,0.]}else{[0.;3]};
  if rest{assert_eq!(json!(rescue_gravity.map(f32::to_bits)),case["gravity"]);}
  let mut world=ContactWorld::empty_native(dt,seed,gravity,Vec3::ZERO).unwrap();
  let mut shared_registry=None;
  if materials{
   let mut registry=crate::ivp_surface_registry::Registry::default();
   for (index,text) in data["material_texts"].as_array().unwrap().iter().enumerate(){let text=text.as_str().unwrap();let bytes=(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect::<Vec<_>>();registry.parse_file(&format!("material-stack-{index}"),&String::from_utf8(bytes).unwrap()).unwrap();}
   let mut manager=crate::ivp_material_backend::Manager{map:[0;128]};let input=data["material_table"].as_array().unwrap().iter().map(|n|n.as_i64().unwrap() as i32).collect::<Vec<_>>();if matches!(cases,Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){shared_material_table.set(&input,128).unwrap();let shared=crate::ivp_material_backend::SharedSurfaceRegistry::new(registry);world.bind_shared_materials(&shared,&shared_material_table).unwrap();shared_registry=Some(shared);}else if cases==Cases::OwnedLiveMaterial{shared_material_table.set(&input,128).unwrap();world.bind_world_materials(&registry,&shared_material_table).unwrap();}else{manager.set_world_table(&input,128).unwrap();world.set_feature_materials(&registry,&manager).unwrap();}
  }
  let mut profile=crate::ivp_distance_schedule::Config::bare_native();if rest{profile.set_force_gravity(gravity).unwrap();}
  if warm{let prefix=f(&case["prefix"]);if prefix>0.{world.advance(prefix).unwrap();}assert_eq!(world.clock.now.to_bits(),case["now"].as_u64().unwrap());assert_eq!(world.clock.deadline.to_bits(),case["deadline"].as_u64().unwrap());assert_eq!(world.clock.clock_serial as u64,case["frame"].as_u64().unwrap());assert_eq!(world.clock.queue.epoch.to_bits(),case["epoch"].as_u64().unwrap());
   let config=profile;assert_eq!(json!(config.targets.map(f32::to_bits).to_vec()),case["config"]["targets"]);assert_eq!(json!(config.slow_gate.to_bits()),case["config"]["slow_gate"]);assert_eq!(json!(config.time_scale.to_bits()),case["config"]["time_scale"]);assert_eq!(json!(config.gap.to_bits()),case["config"]["gap"]);assert_eq!(case["config"]["face_epsilon"],json!(975594773u32));}
  for id in 0..3{world.create_poly(id as u64+1,crate::ivp_poly_constructor::Parameters{
   surface:crate::ivp_poly_constructor::Surface{center:v(&data["surface"]["center"]),unit_inertia:v(&data["surface"]["inertia"]),radius:f(&data["surface"]["radius"]),factor:data["surface"]["factor"].as_u64().unwrap() as u8},
   margin:0.,center:None,position:v(&case["positions"][id]).to_array(),angles:[0.;3],mass:5.+5.*id as f32,inertia_scale:1.,damping:0.,angular_damping:0.,rotational_limit:0.1,fixed:case["fixed"][id].as_bool().unwrap_or(false),collisions:true},0.8).unwrap();}
  if matches!(cases,Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){for object in 1..=3{world.set_object_material(object,1).unwrap();}}
  if motion{for id in 0..3{let input=&case["velocity_inputs"][id];let linear=(!input["linear"].is_null()).then(||v(&input["linear"]));let angular=(!input["angular"].is_null()).then(||v(&input["angular"]));if linear.is_some()||angular.is_some(){world.set_velocity(id,linear,angular).unwrap();}}}
  if impact||matches!(cases,Cases::Collision|Cases::OwnedCollision|Cases::OwnedImpactWake|Cases::OwnedSleep|Cases::OwnedRewake|Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){for object in 1..=3{world.configure_object_elasticity(object,0.2).unwrap();}}
  assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&case["before"]),"case{} initial physical bank",case["id"]);
  let mut cache=WorldCache::new(8).unwrap();
  for id in 0..3{cache.register_core(world.bodies()[id].prediction);}
  cache.register_object(0,world.bodies()[0].object_flags,world.state.frames[&1]).unwrap();
  for id in 0..3{cache.register_object(id,world.bodies()[id].object_flags,world.state.frames[&(id as u64+1)]).unwrap();}
  let clock=CacheClock{time:world.clock.now,frame:world.clock.clock_serial};let mut compact=Ownership::new(4,8,8,world.clock.queue.epoch).unwrap();let mut entries:BTreeMap<[usize;2],Entry<'_>>=BTreeMap::new();let mut notifications=vec![];
  let mut discovery=Discovery::new((0..3).map(|core|DiscoveryObject{id:core+1,core,solid:&material_solid,margin:0.,core_radius:f(&data["surface"]["radius"]),core_angular_radius:crate::ivp_bounds::Compact{center:v(&data["surface"]["center"]),radius:f(&data["surface"]["radius"]),factor:data["surface"]["factor"].as_u64().unwrap() as u8}.at(v(&data["surface"]["center"])).unwrap().angular_radius,cached_motion:[0.;2]}).collect(),&[1,2,3],world.bodies(),dt).unwrap();let deadline=world.clock.deadline;
  if warm{for id in 0..3{let b=discovery.object_budget(id+1).unwrap();assert_eq!(json!({"time":b.time.to_bits(),"rates":b.rates.map(f32::to_bits),"accumulated":b.accumulated.map(f32::to_bits)}),case["budgets_before"][id]);}}
  let actual_queue=&mut world.clock.queue;
  let mut admit=|physical:&mut CollisionContext<'_>,owner:&mut Ownership,waking:usize|->Result<bool,&'static str>{
   let mut created=false;
   if discover{
    discovery.notify_wake(waking+1,clock,[0.;2],physical.bodies_mut()?,&mut cache,&mut Constructor{owner,queue:actual_queue,clock,dt,deadline,radius:f(&data["surface"]["radius"]),gravity:rescue_gravity})?;
    for link in owner.graph.object_links(waking+1)?.to_vec(){
     let objects=owner.graph.objects(link.pair)?;let other=objects[link.side^1]-1;
     if owner.graph.flags(link.pair)?&0x3000!=0||physical.bodies()?[other].flags&2!=0||physical.core_has_contact_system(other){continue;}
     let (entry,convexes,keys)=discovery.entry_mut(link.pair)?;let bodies=entry.key.slots.map(|id|physical.bodies().unwrap()[id-1]);
     entry.contact.refresh_geometry(&mut cache,clock,&bodies,convexes.map(|c|c.topology.as_deref().unwrap()),convexes.map(|c|c.triangle_headers.as_slice()))?;owner.graph.set_flags(link.pair,entry.contact.flags)?;
     if entry.contact.flags&0xc000==0&&entry.contact.query.geometry.ok_or("missing discovered wake distance")?.distance<f32::from_bits(1021967909){created|=entry.admit_wake_neighbour(physical,keys,convexes,&mut cache,clock,waking,other)?;}
    }
   }else{
   // Candidate provider is authored independently of the observed native link
   // lists. This isolates wake/feature/admission ordering from broadphase work.
   for other in 0..3{
    if other==waking||physical.bodies()?[other].flags&2!=0||physical.core_has_contact_system(other){continue;}
    let objects=[waking.min(other)+1,waking.max(other)+1];
    owner.bind_objects(objects,objects.map(|id|id-1))?;
    // Ordinary constructor links the mindist before its first query. Physical
    // admission's native gate rejects an unlinked constructor packet.
    let entry=entries.entry(objects).or_insert_with(||Entry{key:crate::ivp_surface_admission::Pair{slots:objects,ledges:[0,0]},contact:Contact::new(objects,crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS|0xc0000,0.).unwrap(),recursive:None});
    let bodies=objects.map(|id|physical.bodies().unwrap()[id-1]);
    entry.contact.refresh_geometry(&mut cache,clock,&bodies,[convex.topology.as_deref().unwrap();2],[convex.triangle_headers.as_slice();2])?;
    if entry.contact.flags&0xc000==0&&entry.contact.query.geometry.ok_or("missing compact wake distance")?.distance<f32::from_bits(1021967909){
     created|=entry.admit_wake_neighbour(physical,[1,1],[convex;2],&mut cache,clock,waking,other)?;
    }
   }
   }
   notifications.push(json!({"id":waking,"flags":physical.bodies()?.iter().map(|b|b.object_flags).collect::<Vec<_>>()}));Ok(created)
  };
  let mut context=CollisionContext{state:&mut world.state,now:clock.time,deadline};let start=case["start"].as_u64().unwrap() as usize;
  if case["whole"].as_bool().unwrap(){context.wake_impact_unit_with_admission(start,&mut compact,&mut admit,|_,_,_,_|Ok(())).unwrap();}
  else{let (_,created)=context.wake_impact_member_with_admission(start,&mut compact,&mut admit,|_,_,_,_|Ok(())).unwrap();assert_eq!(usize::from(created),case["outcome"].as_u64().unwrap() as usize);}
  if impact{for core in 0..2{context.wake_impact_unit_with_admission(core,&mut compact,&mut admit,|_,_,_,_|Ok(())).unwrap();}}
  assert_eq!(json!(notifications),case["notifications"],"case{} wake notification order/flags",case["id"]);
  if discover{
   let mut actual=vec![];
   for object in 1..=3{let mut links=vec![];for link in compact.graph.object_links(object).unwrap(){let (entry,_,_)=discovery.entry_mut(link.pair).unwrap();links.push(json!({"objects":entry.key.slots.map(|id|id-1),"flags":entry.contact.flags&0x3fffffff,"kinds":entry.contact.query.kinds,"query_frame":entry.contact.query.frame}));}actual.push(links);}
   let mut expected=case["links_after"].clone();for list in expected.as_array_mut().unwrap(){for link in list.as_array_mut().unwrap(){link["flags"]=json!(link["flags"].as_u64().unwrap()&0x3fffffff);}}
   //1010ce30 preserves uninitialized upper two flag bits in its constructor.
   // Compare all defined flag bits, endpoint order, features and query serials.
   assert_eq!(json!(actual),expected,"case{} discovered object-link order/features",case["id"]);
  }
  assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&case["after"]),"case{} final physical bank",case["id"]);
  let registry=world.registry();
  for id in 0..3{assert_eq!(json!(registry.unit_cores(id).unwrap()),case["units_after"][id]["members"],"case{} unit{id} member order",case["id"]);assert_eq!(json!(registry.unit_flags(id).unwrap().flags),case["units_after"][id]["state"]);}
  merged+=usize::from(registry.unit_cores(start).unwrap().len()>1);
  if impact{
   let pair=compact.graph.object_links(1).unwrap().iter().find_map(|link|{let objects=compact.graph.objects(link.pair).unwrap();(objects.contains(&2)).then_some(link.pair)}).unwrap();
   assert_eq!(json!(discovery.entry_mut(pair).unwrap().0.key.slots.map(|id|id-1)),case["impact_objects"]);
   let mut run_impact=true;
   if dispatch{
    use crate::ivp_surface_admission::contacts::TimedEvent;
    let mode=case["event_mode"].as_u64().unwrap();
    // The callback must reject entries the real dispatcher has not popped.
    // A separate diagnostic timer must survive queue borrowing unchanged.
    compact.queue.insert(9999,3.).unwrap();
    let diagnostic=compact.queue.snapshot();
    if world.clock.queue.scheduled(pair).is_none(){world.clock.queue.insert(pair,0.25).unwrap();}
    let queued=world.clock.queue.snapshot();let before=crate::ivp_registered_contacts::snapshot(&world);
    let mut context=CollisionContext{state:&mut world.state,now:clock.time,deadline};
    assert!(discovery.dispatch_ordinary_event(pair,&mut compact,&mut world.clock.queue,clock,deadline,context.bodies_mut().unwrap(),&mut cache,[0.;3],crate::ivp_distance_schedule::Config::bare_native(),f32::from_bits(975594773)).is_err());
    assert_eq!(compact.queue.snapshot(),diagnostic);assert_eq!(world.clock.queue.snapshot(),queued);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);
    world.clock.queue.cancel(pair).unwrap();
    let (entry,_,_)=discovery.entry_mut(pair).unwrap();
    if mode==1{entry.contact.flags=(entry.contact.flags&!15)|1;}
    if mode==2{entry.contact.flags|=0x4000;}
    compact.graph.set_flags(pair,entry.contact.flags).unwrap();
    assert_eq!(entry.contact.flags&0x3fffffff,case["event_input_flags"].as_u64().unwrap() as u32&0x3fffffff);
    let mut config=crate::ivp_distance_schedule::Config::bare_native();
    if mode==3{config.targets[((entry.contact.flags>>22)&255) as usize]=0.;}
    assert_eq!(config.target(entry.contact.flags).unwrap().to_bits(),case["event_target"].as_u64().unwrap() as u32);
    let mut context=CollisionContext{state:&mut world.state,now:clock.time,deadline};
    let out=discovery.dispatch_ordinary_event(pair,&mut compact,&mut world.clock.queue,clock,deadline,context.bodies_mut().unwrap(),&mut cache,[0.;3],config,f32::from_bits(975594773)).unwrap_or_else(|e|panic!("case{} timed world callback: {e}",case["id"]));
    assert_eq!(compact.queue.snapshot(),diagnostic);compact.queue.cancel(9999).unwrap();
    let (entry,_,_)=discovery.entry_mut(pair).unwrap();
    run_impact=matches!(out,TimedEvent::Impact);
    branches[match out{TimedEvent::Impact=>0,TimedEvent::Retry(_)=>1,TimedEvent::InvalidGeometry=>2}]+=1;
    if !run_impact{
     assert_eq!(entry.contact.flags&0x3fffffff,case["event_output_flags"].as_u64().unwrap() as u32&0x3fffffff,"case{} retry/invalid flags",case["id"]);
     assert_eq!(world.clock.queue.scheduled(pair).map(|(_,key)|key.to_bits()),case["event_key"].as_u64().map(|v|v as u32),"case{} retry/invalid timer",case["id"]);
     assert!(compact.graph.active().contains(&pair));
    }
   }
   if run_impact{
   if dispatch{compact.begin_collision_revision();}
   let (entry,convexes,keys)=discovery.entry_mut(pair).unwrap();
   let mut context=CollisionContext{state:&mut world.state,now:clock.time,deadline};let admitted=entry.admit_impact(&mut context,keys,convexes,&mut cache,clock).unwrap();
   assert!(context.has_impact_elasticities(admitted.key).unwrap());context.primary_impact(admitted.key).unwrap();
   let mut wave=crate::ivp_impact_wave::Wave::new_cached(&mut context,admitted,&mut cache,clock).unwrap();
   while wave.step_cached(&mut context,&mut cache,clock).unwrap().is_some(){wave.continue_iteration().unwrap();}
   let changed=wave.rebuild(&mut context).unwrap();
   if dispatch{
    let motion=changed.iter().map(|&core|(core,context.prediction_motion(core).unwrap())).collect::<Vec<_>>();
    discovery.rebuild_bounds(&motion,clock,deadline,context.bodies_mut().unwrap(),&mut cache,&mut Constructor{owner:&mut compact,queue:&mut world.clock.queue,clock,dt,deadline,radius:f(&data["surface"]["radius"]),gravity:rescue_gravity}).unwrap_or_else(|e|panic!("case{} post-impact bounds: {e}",case["id"]));
    compact.with_queue(&mut world.clock.queue,|owner|discovery.recheck_impact_pairs(&changed,owner,clock,deadline,context.bodies_mut().unwrap(),&mut cache,[0.;3],crate::ivp_distance_schedule::Config::bare_native(),f32::from_bits(975594773))).unwrap_or_else(|e|panic!("case{} post-impact recheck: {e}",case["id"]));
    let (entry,_,_)=discovery.entry_mut(pair).unwrap();
    assert_eq!(entry.contact.flags&0x3fffffff,case["event_output_flags"].as_u64().unwrap() as u32&0x3fffffff,"case{} post-impact flags",case["id"]);
    assert_eq!(world.clock.queue.scheduled(pair).map(|(_,key)|key.to_bits()),case["event_key"].as_u64().map(|v|v as u32),"case{} post-impact timer",case["id"]);
   }
   }
   assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&case["impact_after"]),"case{} compact event physical bank",case["id"]);
   for id in 0..3{assert_eq!(json!(world.registry().unit_cores(id).unwrap()),case["impact_units"][id]["members"]);assert_eq!(json!(world.registry().unit_flags(id).unwrap().flags),case["impact_units"][id]["state"]);}
  }
  if matches!(cases,Cases::OwnedCollision|Cases::OwnedImpactWake|Cases::OwnedSleep|Cases::OwnedRewake|Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){
   use crate::ivp_surface_admission::contacts::discovery::owner::{Owner,Phase};
   let dormant_target=world.bodies()[1].object_flags&255==8;
   let mut coupled=Owner::from_parts(discovery,compact,cache,&world,rescue_gravity,profile,f32::from_bits(975594773)).unwrap();
   // The dormant-target control reaches impact with actor1 still sleeping.
   let fail_case=if cases==Cases::OwnedImpactWake{8}else{4};
   if !rest&&case["id"].as_u64()==Some(fail_case){assert!(case["events"].as_array().unwrap().iter().any(|e|e["kind"]==2));for fail in [Phase::Predicted,Phase::Timed,Phase::Impact,Phase::Woken,Phase::Rebuilt]{
    let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
    assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==fail{reached=true;Err("injected staged-owner failure")}else{Ok(())}),Err("injected staged-owner failure"));
    assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before,"physical candidate leaked after {fail:?}");assert_eq!(coupled.snapshot(),metadata,"contact/cache/hull candidate leaked after {fail:?}");
   }}
   let mut trace=vec![];let mut observed_banks=vec![];
   let diagnostic_case=std::env::var("L4D2R_TRACE_REST_CASE").ok().and_then(|s|s.parse::<u64>().ok());
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_contact_pipeline::TANGENT_AUDIT.with(|a|*a.borrow_mut()=Some(vec![]));crate::ivp_registered_contacts::PRIMARY_AUDIT.with(|a|*a.borrow_mut()=Some(vec![]));crate::ivp_movement::AUDIT.with(|a|*a.borrow_mut()=Some(vec![]));crate::ivp_batch_normal::BATCH_AUDIT.with(|a|*a.borrow_mut()=Some(vec![]));crate::ivp_surface_admission::contacts::discovery::owner::OWNER_AUDIT.with(|a|*a.borrow_mut()=Some(vec![]));}
   let steps=case["prediction_steps"].as_u64().unwrap_or(1);
   let mut stack_rollback_gates=0;
   for iteration in 0..steps{
    if cases==Cases::OwnedMaterialAssign{for update in case["material_assignments"].as_array().unwrap(){if update["iteration"].as_u64()==Some(iteration){
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["before"]),"case{} before material assignment",case["id"]);
     let expected_systems=material_systems["cases"][case["id"].as_u64().unwrap() as usize]["updates"].as_array().unwrap().iter().find(|u|u["iteration"].as_u64()==Some(iteration)).unwrap();
     if !compound{assert_eq!(system_bank(&world),expected_systems["before_systems"],"case{} iteration{iteration} before material systems",case["id"]);}
     let cache_before=(case["id"]==5&&iteration==20&&std::env::var_os("L4D2R_TRACE_MATERIAL_CACHE").is_some()).then(||coupled.snapshot());
     let noop=(iteration==16).then(||(crate::ivp_registered_contacts::snapshot(&world),coupled.snapshot()));
     coupled.assign_material(&mut world,update["actor"].as_u64().unwrap()+1,update["index"].as_i64().unwrap() as i32).unwrap();
     if !compound{assert_eq!(system_bank(&world),expected_systems["after_systems"],"case{} iteration{iteration} after material systems",case["id"]);}
     if let Some((physical,owner))=noop{assert_eq!(crate::ivp_registered_contacts::snapshot(&world),physical);assert_eq!(coupled.snapshot(),owner);}
     if let Some(before)=cache_before{std::fs::write(std::env::var_os("L4D2R_TRACE_MATERIAL_CACHE").unwrap(),serde_json::to_vec(&json!({"before":before,"after":coupled.snapshot(),"clock":format!("{:?}",world.clock())})).unwrap()).unwrap();}

     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["after"]),"case{} after material assignment",case["id"]);
     if case["id"]==68&&iteration==20{let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
      assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==Phase::Predicted{reached=true;Err("injected post-material queued-wake failure")}else{Ok(())}),Err("injected post-material queued-wake failure"));
      assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
     }

    }}}

    if matches!(cases,Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial){for update in case["registry_updates"].as_array().unwrap(){if update["iteration"].as_u64()==Some(iteration){
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["before"]),"case{} before registry update",case["id"]);
     let text=update["text"].as_str().unwrap();let bytes=(0..text.len()).step_by(2).map(|i|u8::from_str_radix(&text[i..i+2],16).unwrap()).collect::<Vec<_>>();shared_registry.as_ref().unwrap().parse_file(update["file"].as_str().unwrap(),&String::from_utf8(bytes).unwrap()).unwrap();
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["after"]),"case{} after registry update",case["id"]);
     if case["id"].as_u64()==Some(1)&&iteration==4{let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
      assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==Phase::Predicted{reached=true;Err("injected post-registry-update failure")}else{Ok(())}),Err("injected post-registry-update failure"));
      assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
     }

    }}}

    if cases==Cases::OwnedLiveMaterial{for update in case["remap_updates"].as_array().unwrap(){if update["iteration"].as_u64()==Some(iteration){
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["before"]),"case{} before remap",case["id"]);
     let input=update["input"].as_array().unwrap().iter().map(|v|v.as_i64().unwrap() as i32).collect::<Vec<_>>();shared_material_table.set(&input,update["count"].as_i64().unwrap() as i32).unwrap();
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&update["after"]),"case{} after remap",case["id"]);
     if case["id"].as_u64()==Some(1)&&iteration==4{let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
      assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==Phase::Predicted{reached=true;Err("injected post-remap failure")}else{Ok(())}),Err("injected post-remap failure"));
      assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
     }
    }}}

    if cases==Cases::OwnedMaterial&&case["id"].as_u64()==Some(15){
     let native:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-compact-material-case15-banks.json")).unwrap();let gate=&native["rollback"];
     if iteration==gate["iteration"].as_u64().unwrap(){let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
      assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==Phase::Inactive&&e.clock.time.to_bits()==gate["time"].as_u64().unwrap(){reached=true;Err("injected inactive-query completion failure")}else{Ok(())}),Err("injected inactive-query completion failure"));
      assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
     }
    }

    // Falling stacks first impact after several public calls. Exercise all
    // five rollback gates in that actual call, retaining its accepted prefix.
    if rest&&case["id"].as_u64()==Some(fail_case){
     let impact=case["events"].as_array().unwrap().iter().find(|e|e["kind"]==2).unwrap();
     let impact_time=f64::from_bits(impact["time"].as_u64().unwrap());let now=world.clock().now;
     if now<=impact_time&&impact_time<now+f(&case["step"]) as f64{for fail in [Phase::Predicted,Phase::Timed,Phase::Impact,Phase::Woken,Phase::Rebuilt]{
      let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
      assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==fail{reached=true;Err("injected staged-stack failure")}else{Ok(())}),Err("injected staged-stack failure"));
      assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before,"stack physical state leaked after {fail:?}");assert_eq!(coupled.snapshot(),metadata,"stack owner state leaked after {fail:?}");
      stack_rollback_gates+=1;
     }}
    }
    if cases==Cases::OwnedRewake&&case["rewake"]["iteration"].as_u64()==Some(iteration){
     let action=&case["rewake"];let actor=action["actor"].as_u64().unwrap() as usize;
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&action["before"]),"case{} before public rewake",case["id"]);
     world.set_velocity(actor,(!action["linear"].is_null()).then(||v(&action["linear"])),(!action["angular"].is_null()).then(||v(&action["angular"]))).unwrap();
     assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&action["after"]),"case{} after public rewake",case["id"]);
    }
    // The same actor that slept in case20 is explicitly spun awake at call13.
    // A rejected prediction must preserve the accepted queued public command,
    // but roll back all wake/discovery/cache effects of the attempted tick.
    if cases==Cases::OwnedRewake&&case["id"].as_u64()==Some(20)&&iteration==12{
     let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut reached=false;
     assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|if e.phase==Phase::Predicted{reached=true;Err("injected public-rewake failure")}else{Ok(())}),Err("injected public-rewake failure"));
     assert!(reached);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
    }
    // Case20's tenth public call is the first sleep transition that formerly
    // returned an unsupported error. Fail after its fourth/final PSI, then
    // require both owner banks unchanged before the native-matched replay.
    if cases==Cases::OwnedSleep&&case["id"].as_u64()==Some(20)&&iteration==9{
     let before=crate::ivp_registered_contacts::snapshot(&world);let metadata=coupled.snapshot();let mut predicted=0;
     assert_eq!(coupled.advance_observed(&mut world,f(&case["step"]),|e|{if e.phase==Phase::Predicted{predicted+=1;if predicted==4{return Err("injected post-sleep failure");}}Ok(())}),Err("injected post-sleep failure"));
     assert_eq!(predicted,4);assert_eq!(crate::ivp_registered_contacts::snapshot(&world),before);assert_eq!(coupled.snapshot(),metadata);
    }
    coupled.advance_observed(&mut world,f(&case["step"]),|e|{
    let kind=match e.phase{Phase::Prediction=>0,Phase::Timed=>1,Phase::Impact=>{collisions+=1;2},Phase::Rebuilt if rest&&((cases==Cases::OwnedRest&&matches!(case["id"].as_u64(),Some(0|1)))||((cases==Cases::OwnedMaterial&&case["id"].as_u64()==Some(15))||((cases==Cases::OwnedLiveMaterial&&case["id"].as_u64()==Some(55))||(cases==Cases::OwnedMaterialAssign&&case["id"].as_u64()==Some(5))))||case["id"].as_u64()==diagnostic_case)=>3,_=>return Ok(())};
    if (cases==Cases::OwnedRewake&&case["id"].as_u64()==Some(9)&&kind!=0)||(rest&&((cases==Cases::OwnedRest&&matches!(case["id"].as_u64(),Some(0|1)))||((cases==Cases::OwnedMaterial&&case["id"].as_u64()==Some(15))||((cases==Cases::OwnedLiveMaterial&&case["id"].as_u64()==Some(55))||(cases==Cases::OwnedMaterialAssign&&case["id"].as_u64()==Some(5))))||case["id"].as_u64()==diagnostic_case)){let mut row=json!({"kind":kind,"time":e.clock.time.to_bits(),"bodies":e.bodies.iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()});
     if rest{let mut contacts=e.contacts.contact_state_snapshot();for c in contacts.as_array_mut().unwrap(){for object in c["objects"].as_array_mut().unwrap(){*object=json!(object.as_u64().unwrap()-1);}}contacts.as_array_mut().unwrap().sort_by_key(|r|r.to_string());row["contacts"]=contacts;}observed_banks.push(row);}
    if kind!=3{trace.push(json!({"kind":kind,"time":e.clock.time.to_bits(),"frame":e.clock.frame,"flags":e.flags&0x3fffffff,"objects":e.objects.map(|o|o.map(|id|id-1)).unwrap_or([999;2])}));}Ok(())
   }).unwrap_or_else(|e|panic!("case{} iteration{iteration} transactional compact owner: {e}",case["id"]));}
   if rest&&case["id"].as_u64()==Some(fail_case){assert_eq!(stack_rollback_gates,5,"stack rollback gates must all be reached");}
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_contact_pipeline::TANGENT_AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_FRICTION"){std::fs::write(path,serde_json::to_vec(&rows).unwrap()).unwrap();}}});}
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_registered_contacts::PRIMARY_AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_PRIMARY"){std::fs::write(path,serde_json::to_vec(&rows).unwrap()).unwrap();}}});}
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_batch_normal::BATCH_AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_NORMAL"){std::fs::write(path,serde_json::to_vec(&rows).unwrap()).unwrap();}}});}
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_surface_admission::contacts::discovery::owner::OWNER_AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_OWNER"){std::fs::write(path,serde_json::to_vec(&rows).unwrap()).unwrap();}}});}
   if rest&&case["id"].as_u64()==Some(diagnostic_case.unwrap_or(0)){crate::ivp_movement::AUDIT.with(|a|{if let Some(rows)=a.borrow_mut().take(){if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_MOVEMENT"){std::fs::write(path,serde_json::to_vec(&rows).unwrap()).unwrap();}}});}
   if rest&&case["id"].as_u64()==diagnostic_case{if let Some(path)=std::env::var_os("L4D2R_TRACE_REST_BANKS"){std::fs::write(path,serde_json::to_vec(&observed_banks).unwrap()).unwrap();}}
   if !observed_banks.is_empty()&&(!rest||(cases==Cases::OwnedRest&&matches!(case["id"].as_u64(),Some(0|1)))||((cases==Cases::OwnedMaterial&&case["id"].as_u64()==Some(15))||((cases==Cases::OwnedLiveMaterial&&case["id"].as_u64()==Some(55))||(cases==Cases::OwnedMaterialAssign&&case["id"].as_u64()==Some(5))))){let native:Value=serde_json::from_str(if cases==Cases::OwnedMaterialAssign{include_str!("../tools/fixtures/native-ivp-compact-material-assign-case5-banks.json")}else if cases==Cases::OwnedLiveMaterial{include_str!("../tools/fixtures/native-ivp-compact-live-material-case55-banks.json")}else if cases==Cases::OwnedMaterial{include_str!("../tools/fixtures/native-ivp-compact-material-case15-banks.json")}else if rest{if case["id"].as_u64()==Some(1){include_str!("../tools/fixtures/native-ivp-compact-rest-case1-banks.json")}else{include_str!("../tools/fixtures/native-ivp-compact-rest-banks.json")}}else{include_str!("../tools/fixtures/native-ivp-compact-rewake-banks.json")}).unwrap();assert_eq!(case["id"],native["case"]);
    for (index,(actual,expected)) in observed_banks.iter().zip(native["rows"].as_array().unwrap()).enumerate(){
     assert_eq!(actual["kind"],expected["kind"],"case{} canonical entry{index} kind",case["id"]);assert_eq!(actual["time"],expected["time"],"case{} canonical entry{index} time",case["id"]);
     if rest{assert_eq!(actual["contacts"],expected["contacts"],"case{} canonical entry{index} persistent contacts",case["id"]);}
     for actor in 0..3{for (key,value) in actual["bodies"][actor].as_object().unwrap(){assert_eq!(value,&expected["bodies"][actor][key],"case{} canonical entry{index} actor{actor} field{key}",case["id"]);}}
    }assert_eq!(observed_banks.len(),native["rows"].as_array().unwrap().len());
   }
   let mut expected=case["events"].clone();for e in expected.as_array_mut().unwrap(){e["flags"]=json!(e["flags"].as_u64().unwrap()&0x3fffffff);}for (index,(actual,native)) in trace.iter().zip(expected.as_array().unwrap()).enumerate(){assert_eq!(actual,native,"case{} native callback{index}",case["id"]);}assert_eq!(trace.len(),expected.as_array().unwrap().len(),"case{} native callback count",case["id"]);
   if cases==Cases::OwnedImpactWake&&dormant_target&&world.bodies()[1].object_flags&255!=8{target_wakes+=1;}
   (discovery,compact,cache)=coupled.into_parts();
   assert_eq!(world.clock.now.to_bits(),case["prediction_now"].as_u64().unwrap());assert_eq!(world.clock.deadline.to_bits(),case["prediction_deadline"].as_u64().unwrap());assert_eq!(world.clock.clock_serial as u64,case["prediction_frame"].as_u64().unwrap());
   if cases==Cases::OwnedMaterialAssign&&!compound{assert_eq!(system_bank(&world),material_systems["cases"][case["id"].as_u64().unwrap() as usize]["post_systems"],"case{} final material systems",case["id"]);}
   assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&case["predicted_after"]),"case{} staged owner final physical bank",case["id"]);
   for id in 0..3{assert_eq!(json!(world.registry().unit_cores(id).unwrap()),case["predicted_units"][id]["members"]);assert_eq!(json!(world.registry().unit_flags(id).unwrap().flags&0xff),case["predicted_units"][id]["state"]);assert_eq!(discovery.motion_snapshot(id+1),case["post_motion"][id]);}
  }
  if matches!(cases,Cases::Prediction|Cases::Far|Cases::Collision){
   let owners=std::cell::RefCell::new((&mut discovery,&mut compact,&mut cache));let event_trace=std::cell::RefCell::new(Vec::<Value>::new());
   world.advance_with_mutable_cache_context(f(&case["step"]),|bodies,ids,clock|{
    let mut owners=owners.borrow_mut();let (_,_,cache)=&mut *owners;
    for &core in ids{cache.update_core(core,bodies[core].prediction)?;cache.set_flags(core+1,bodies[core].object_flags)?;cache.access(core+1,clock)?;}Ok(())
   },|context,order,notices,clock,deadline,queue|{
    let mut owners=owners.borrow_mut();let (discovery,compact,cache)=&mut *owners;
    if cases==Cases::Collision{event_trace.borrow_mut().push(json!({"kind":0,"time":clock.time.to_bits(),"frame":clock.frame,"flags":0,"objects":[999,999]}));}
    for &core in notices{
     if context.bodies()?[core].object_flags&255==8{return Err("isolated prediction unexpectedly slept");}
     let motion=discovery.cached_motion(core+1)?;
     discovery.notify_wake(core+1,clock,motion,context.bodies_mut()?,cache,&mut Constructor{owner:compact,queue,clock,dt,deadline,radius:f(&data["surface"]["radius"]),gravity:rescue_gravity})?;
    }
    let motion=order.iter().map(|&id|context.prediction_motion(id).map(|m|(id,m))).collect::<Result<Vec<_>,_>>()?;
    for &(id,m) in &motion{let angular=Vec3::from_array(context.bodies()?[id].angular[..3].try_into().unwrap());if m.delta.map(f32::to_bits)!=crate::ivp_motion::sine_delta(angular,m.dt).map(f32::to_bits){original_delta_distinctions+=1;}}
    discovery.predict_bounds(&motion,clock,deadline,context.bodies_mut()?,cache,&mut Constructor{owner:compact,queue,clock,dt,deadline,radius:f(&data["surface"]["radius"]),gravity:rescue_gravity})?;
    if matches!(cases,Cases::Far|Cases::Collision){discovery.refresh_schedule(compact,queue,clock,deadline,context.bodies_mut()?,cache,[0.;3],crate::ivp_distance_schedule::Config::bare_native(),f32::from_bits(975594773))?;}Ok(())
   },|context,id,clock,deadline,queue|{
    if cases!=Cases::Collision{return Err("separating prediction unexpectedly collided");}
    let mut owners=owners.borrow_mut();let (discovery,compact,cache)=&mut *owners;
    event_trace.borrow_mut().push(json!({"kind":1,"time":clock.time.to_bits(),"frame":clock.frame,"flags":compact.graph.flags(id)?&0x3fffffff,"objects":compact.graph.objects(id)?.map(|id|id-1)}));
    match discovery.dispatch_ordinary_event(id,compact,queue,clock,deadline,context.bodies_mut()?,cache,[0.;3],crate::ivp_distance_schedule::Config::bare_native(),f32::from_bits(975594773))?{
     crate::ivp_surface_admission::contacts::TimedEvent::Impact=>{
      event_trace.borrow_mut().push(json!({"kind":2,"time":clock.time.to_bits(),"frame":clock.frame,"flags":compact.graph.flags(id)?&0x3fffffff,"objects":compact.graph.objects(id)?.map(|id|id-1)}));
      compact.begin_collision_revision();let (entry,convexes,keys)=discovery.entry_mut(id)?;
      let admitted=entry.admit_impact(context,keys,convexes,cache,clock)?;context.primary_impact(admitted.key)?;
      let mut wave=crate::ivp_impact_wave::Wave::new_cached(context,admitted,cache,clock)?;
      while wave.step_cached(context,cache,clock)?.is_some(){wave.continue_iteration()?;}
      let changed=wave.rebuild(context)?;let motion=changed.iter().map(|&id|context.prediction_motion(id).map(|m|(id,m))).collect::<Result<Vec<_>,_>>()?;
      discovery.rebuild_bounds(&motion,clock,deadline,context.bodies_mut()?,cache,&mut Constructor{owner:compact,queue,clock,dt,deadline,radius:f(&data["surface"]["radius"]),gravity:rescue_gravity})?;
      compact.with_queue(queue,|owner|discovery.recheck_impact_pairs(&changed,owner,clock,deadline,context.bodies_mut()?,cache,[0.;3],crate::ivp_distance_schedule::Config::bare_native(),f32::from_bits(975594773)))?;
      collisions+=1;Ok(())
     },_=>Ok(())
    }
   }).unwrap_or_else(|e|panic!("case{} natural prediction: {e}",case["id"]));
   drop(owners);
   if cases==Cases::Collision{let mut expected=case["events"].clone();for event in expected.as_array_mut().unwrap(){event["flags"]=json!(event["flags"].as_u64().unwrap()&0x3fffffff);}assert_eq!(json!(*event_trace.borrow()),expected,"case{} native PSI/timed/impact trace",case["id"]);}
   assert_eq!(world.clock.now.to_bits(),case["prediction_now"].as_u64().unwrap());assert_eq!(world.clock.deadline.to_bits(),case["prediction_deadline"].as_u64().unwrap());assert_eq!(world.clock.clock_serial as u64,case["prediction_frame"].as_u64().unwrap());
   assert_eq!(json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),bank(&case["predicted_after"]),"case{} natural prediction body bank",case["id"]);
   // units3 records only the native state byte, not the classification word.
   for id in 0..3{assert_eq!(json!(world.registry().unit_cores(id).unwrap()),case["predicted_units"][id]["members"]);assert_eq!(json!(world.registry().unit_flags(id).unwrap().flags&0xff),case["predicted_units"][id]["state"]);assert_eq!(discovery.motion_snapshot(id+1),case["post_motion"][id],"case{} actor{id} natural prediction motion/budget",case["id"]);}
  }
  if matches!(cases,Cases::Far|Cases::Collision|Cases::OwnedCollision|Cases::OwnedImpactWake|Cases::OwnedSleep|Cases::OwnedRewake|Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){for object in 1..=3{
   let mut expected=case["post_far"][object-1].as_array().unwrap().clone();for r in &mut expected{r["flags"]=json!(r["flags"].as_u64().unwrap()&0x3fffffff);}expected.sort_by_key(|r|r["objects"].to_string());
   let mut actual=discovery.far_snapshot(object);for r in actual.as_array_mut().unwrap(){for id in r["objects"].as_array_mut().unwrap(){*id=json!(id.as_u64().unwrap()-1);}}
   assert_eq!(actual,json!(expected),"case{} object{object} shared-world far listeners",case["id"]);
  }}
  assert!(compact.queue.first().is_none(),"diagnostic queue retained a world event");
  if dispatch{for id in 1..=3{assert_eq!(discovery.motion_snapshot(id),case["post_motion"][id-1],"case{} actor{id} post-impact motion/budget",case["id"]);}}
  let (slots,_,_)=cache.metadata();assert!(slots.iter().all(|s|s.references==0),"wake leaked cache references");
 }
 if dispatch{assert_eq!(branches,[64,320,128]);}
 if matches!(cases,Cases::Collision|Cases::OwnedCollision|Cases::OwnedImpactWake|Cases::OwnedSleep|Cases::OwnedRewake|Cases::OwnedRest|Cases::OwnedMaterial|Cases::OwnedLiveMaterial|Cases::OwnedLiveRegistry|Cases::OwnedObjectMaterial|Cases::OwnedMaterialAssign){assert_eq!(collisions,if cases==Cases::OwnedMaterialAssign{1810}else if cases==Cases::OwnedObjectMaterial{1627}else if cases==Cases::OwnedLiveRegistry{1680}else if cases==Cases::OwnedLiveMaterial{1635}else if cases==Cases::OwnedMaterial{1936}else if rest{1236}else if cases==Cases::OwnedRewake{204}else if cases==Cases::OwnedSleep{160}else{60});println!("naturally dispatched compact impacts: {collisions}");}
 if cases==Cases::OwnedImpactWake{assert_eq!(target_wakes,18);println!("dormant compact targets activated: {target_wakes}");}
 if cases==Cases::Prediction{assert!(original_delta_distinctions>0,"prediction controls did not distinguish original gyro deltas");println!("original gyro delta differs from post-gyro reconstruction in {original_delta_distinctions} predictions");}
 assert_eq!(data["cases"].as_array().unwrap().len(),match cases{Cases::OwnedMaterialAssign=>96,Cases::OwnedObjectMaterial=>96,Cases::OwnedLiveRegistry=>96,Cases::OwnedLiveMaterial=>96,Cases::OwnedMaterial=>96,Cases::OwnedRest=>96,Cases::OwnedRewake=>96,Cases::OwnedSleep=>96,Cases::OwnedImpactWake=>96,Cases::OwnedCollision=>96,Cases::Collision=>96,Cases::Far=>96,Cases::Prediction=>384,Cases::Dispatch=>512,Cases::Impact=>128,Cases::Velocity=>384,Cases::Epochs=>96,Cases::Startup=>24});assert_eq!(merged,match cases{Cases::OwnedMaterialAssign=>0,Cases::OwnedObjectMaterial=>0,Cases::OwnedLiveRegistry=>0,Cases::OwnedLiveMaterial=>0,Cases::OwnedMaterial=>0,Cases::OwnedRest=>0,Cases::OwnedRewake=>0,Cases::OwnedSleep=>0,Cases::OwnedImpactWake=>0,Cases::OwnedCollision=>0,Cases::Collision=>0,Cases::Far=>0,Cases::Prediction=>0,Cases::Dispatch=>512,Cases::Impact=>128,Cases::Velocity=>192,Cases::Epochs=>48,Cases::Startup=>12});
}
