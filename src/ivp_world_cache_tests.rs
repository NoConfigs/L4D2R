//! Recorded native query inputs drive callers; pool/frames are never reseeded.
use crate::ivp_world_cache::*;
use crate::ivp_cache_lifecycle_tests::{controls,pose_crc,state_crc};
use serde_json::Value;
use std::collections::VecDeque;

#[test]
fn populated_native_time_resets_preserve_cache_payloads_and_select_unit_members(){
 use bevy::prelude::Vec3;
 use crate::ivp_motion::{Prediction,ObjectFrame};
 use crate::ivp_hull_budget::Budget;
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-body-time.json")).unwrap();
 let float=|v:&Value|f32::from_bits(v.as_u64().unwrap() as u32);
 let vector=|v:&Value|Vec3::new(float(&v[0]),float(&v[1]),float(&v[2]));
 let quat=|v:&Value|std::array::from_fn(|i|float(&v[i]));
 let double=|v:&Value|f64::from_bits(v.as_u64().unwrap());
 let payload=|cache:&WorldCache,id:usize|{
  let(slots,objects,_)=cache.metadata();let slot=objects[id].cache.unwrap();let p=cache.get(slot).unwrap();
  let r=p.pose.rotation.transpose();let rows=[r.x_axis,r.y_axis,r.z_axis].into_iter().flat_map(|v|v.to_array().map(f32::to_bits)).collect::<Vec<_>>();
  serde_json::json!({"stamp":p.frame_stamp,"references":slots[slot].references,"owner_valid":slots[slot].owner==Some(id),"core_position_bits":p.core_position.to_array().map(f32::to_bits),"position_bits":p.pose.position.to_array().map(f32::to_bits),"quaternion_bits":p.quaternion.map(f32::to_bits),"rotation_bits":rows})
 };
 let mut world=crate::ivp_event_world::World::new(1000).unwrap();let dt=float(&data["frame_dt_bits"]);world.set_timestep(dt).unwrap();
 // Derive the environment clock from the authored setup/step inputs. Body
 // predictor and prior cache-generation packets below are local-operation
 // inputs; no native post-reset payload or expected motion seeds the registry.
 world.simulate(dt,|_,_,_|Ok(()),|_,_,_|Ok(()),|_,_|{}).unwrap();
 let mut rebased=0;let mut untouched=0;
 for (index,case) in data["cases"].as_array().unwrap().iter().enumerate(){
  for _ in 0..data["step_counts"][index].as_u64().unwrap(){world.simulate(dt,|_,_,_|Ok(()),|_,_,_|Ok(()),|_,_|{}).unwrap();}
  let before=&case["before"];let after=&case["after_reset"];let queried=&case["after_access"];
  assert_eq!(world.now.to_bits(),before["now"].as_f64().unwrap().to_bits());assert_eq!(world.start.to_bits(),before["start"].as_f64().unwrap().to_bits());assert_eq!(world.clock_serial as u64,before["serial"].as_u64().unwrap());
  let mut cache=WorldCache::new(8).unwrap();let mut budgets=vec![];
  for b in before["bodies"].as_array().unwrap(){
   let p=Prediction{start:vector(&b["prediction_start_bits"]),velocity:vector(&b["prediction_velocity_bits"]),rotation_start:quat(&b["q0_bits"]),rotation_end:quat(&b["q1_bits"]),time:double(&b["prediction_time_bits"]),inverse_dt:float(&b["inverse_dt_bits"])};
   let id=cache.register_core(p);assert_eq!(id,b["id"].as_u64().unwrap() as usize);
   let frame=ObjectFrame{shift:Some(vector(&b["shift_bits"])),rotation:(!b["object_quaternion_bits"].is_null()).then(||quat(&b["object_quaternion_bits"]))};
   assert_eq!(cache.register_object(id,b["flags"].as_u64().unwrap() as u32,frame).unwrap(),id);
   cache.access(id,Context{time:world.now,frame:b["cache"]["stamp"].as_u64().unwrap() as u32}).unwrap();
   assert_eq!(payload(&cache,id),b["cache"],"initial local cache input case{index} object{id}");
   budgets.push(Budget{time:double(&b["hull_time_bits"]),rates:std::array::from_fn(|i|float(&b["hull_rate_bits"][i])),accumulated:std::array::from_fn(|i|float(&b["hull_accumulated_bits"][i])),predicted:float(&b["hull_predicted_bits"]),threshold:float(&b["hull_threshold_bits"])});
  }
  let mut selected=vec![];
  for unit in before["units"].as_array().unwrap(){
   assert!(unit["controller_reset_rvas"].as_array().unwrap().iter().all(|r|r.as_u64().unwrap()==0x7e0c0));
   selected.extend(unit["cores"].as_array().unwrap().iter().rev().map(|id|id.as_u64().unwrap() as usize));
  }
  let old_serial=world.clock_serial;
  world.reset_time_with(|clock,shift|{
   assert_eq!(clock.clock_serial,old_serial);assert_eq!(clock.queue.epoch,0.);
   let objects=cache.rebase_core_times(&selected,shift).unwrap();
   for object in objects{budgets[object].rebase_time(shift);}
  }).unwrap();
  assert_eq!(world.now.to_bits(),after["now"].as_f64().unwrap().to_bits());assert_eq!(world.clock_serial as u64,after["serial"].as_u64().unwrap());
  for id in 0..3{
   let expected=&after["bodies"][id];let(input,_,_)=cache.object_inputs(id).unwrap();
   assert_eq!(input.time.to_bits(),expected["prediction_time_bits"].as_u64().unwrap());assert_eq!(budgets[id].time.to_bits(),expected["hull_time_bits"].as_u64().unwrap());
   assert_eq!(input.start.to_array().map(f32::to_bits),std::array::from_fn(|i|expected["prediction_start_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(input.velocity.to_array().map(f32::to_bits),std::array::from_fn(|i|expected["prediction_velocity_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(input.rotation_start.map(f32::to_bits),std::array::from_fn(|i|expected["q0_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(input.rotation_end.map(f32::to_bits),std::array::from_fn(|i|expected["q1_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(input.inverse_dt.to_bits(),expected["inverse_dt_bits"].as_u64().unwrap() as u32);
   assert_eq!(budgets[id].rates.map(f32::to_bits),std::array::from_fn(|i|expected["hull_rate_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(budgets[id].accumulated.map(f32::to_bits),std::array::from_fn(|i|expected["hull_accumulated_bits"][i].as_u64().unwrap() as u32));
   assert_eq!(budgets[id].predicted.to_bits(),expected["hull_predicted_bits"].as_u64().unwrap() as u32);
   assert_eq!(budgets[id].threshold.to_bits(),expected["hull_threshold_bits"].as_u64().unwrap() as u32);
   assert_eq!(payload(&cache,id),expected["cache"]);
   cache.access(id,Context{time:world.now,frame:world.clock_serial}).unwrap();
   assert_eq!(payload(&cache,id),queried["bodies"][id]["cache"],"post-reset lookup case{index} object{id}");
   if selected.contains(&id){rebased+=1;}else{untouched+=1;}
  }
 }
 assert_eq!((rebased,untouched),(6,6));
}
#[test]
fn current_object_inputs_do_not_replace_a_retained_same_frame_pose(){
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-object-cache.json")).unwrap();
 let (mut prediction,_,time,stamp)=controls(&data["cases"][0]["before"]);
 let frame=crate::ivp_motion::ObjectFrame{shift:Some(bevy::prelude::Vec3::new(0.2,0.3,0.4)),rotation:None};
 let mut cache=WorldCache::new(8).unwrap();let core=cache.register_core(prediction);let object=cache.register_object(core,0x101,frame).unwrap();
 let lease=cache.retain(object,Context{time,frame:stamp}).unwrap();let crc=pose_crc(&cache.lease_frame(&lease).unwrap());let metadata=cache.metadata().2;
 prediction.start.x+=1.;cache.update_core(core,prediction).unwrap();
 let (current,object_frame,flags)=cache.object_inputs(object).unwrap();assert_eq!(current.start,prediction.start);assert_eq!(object_frame.shift,frame.shift);assert_eq!(flags,0x101);
 assert_eq!(pose_crc(&cache.lease_frame(&lease).unwrap()),crc);assert_eq!(cache.metadata().2,metadata);
 cache.set_flags(object,0x901).unwrap();assert!(cache.object_inputs(object).unwrap().1.shift.is_none());
 assert_eq!(pose_crc(&cache.lease_frame(&lease).unwrap()),crc);assert!(cache.object_inputs(99).is_err());cache.release(lease).unwrap();
}
#[test]
fn registered_cache_replays_native_reference_lifetimes_without_metadata_reseeding(){
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-lifecycle.json")).unwrap();
 let(mut preparations,mut retains,mut releases,mut allocations,mut borrows)=(0,0,0,0,0);let mut retained_reads=0;
 for profile in data["profiles"].as_array().unwrap(){
  let inputs=profile["inputs"].as_array().unwrap();let events=profile["events"].as_array().unwrap();let native=profile["frames"].as_array().unwrap();
  let mut cache=WorldCache::new(256).unwrap();let mut core_ids=[0;2];
  for target in 0..2{let e=events.iter().find(|e|e["target"]==target).unwrap();let input=&inputs[e["input_id"].as_u64().unwrap() as usize];let(p,f,_,_)=controls(input);core_ids[target]=cache.register_core(p);assert_eq!(cache.register_object(core_ids[target],e["flags"][target].as_u64().unwrap() as u32,f).unwrap(),target);}
  let mut leases:[Vec<Lease>;2]=std::array::from_fn(|_|vec![]);let mut pending=VecDeque::new();let mut expected_stamps=vec![0u32;256];
  for e in events{
   let target=e["target"].as_u64().unwrap() as usize;let p=&inputs[e["input_id"].as_u64().unwrap() as usize];let(pred,_,time,frame)=controls(p);let context=Context{time,frame};
   cache.update_core(core_ids[target],pred).unwrap();for i in 0..2{cache.set_flags(i,e["flags"][i].as_u64().unwrap() as u32).unwrap();}
   let kind=e["kind"].as_u64().unwrap();
   match kind{
    0=>{let(s,o,c)=cache.metadata();assert_eq!(state_crc(s,o,c,frame),e["state_crc32"].as_u64().unwrap());cache.access_observed(target,context,|p|pending.push_back(p)).unwrap();allocations+=1;continue;}
    1=>{}
    2=>{let before=e["refs_before"].as_u64().unwrap() as u32;let after=e["refs_after"].as_u64().unwrap() as u32;
     if after==before.wrapping_add(1){let l=cache.retain_observed(target,context,|p|pending.push_back(p)).unwrap();leases[target].push(l);retains+=1;}
     else{assert_eq!(after,before.wrapping_sub(1));cache.release(leases[target].pop().expect("native released an unowned reference")).unwrap();releases+=1;}
     let slot=e["pool_slot"].as_u64().unwrap() as usize;assert_eq!(cache.metadata().0[slot].references,after);
     if !pending.is_empty(){continue;}
    }
    3=>{if pending.is_empty(){cache.access_observed(target,context,|p|pending.push_back(p)).unwrap();borrows+=1;}
     let event=pending.front().unwrap();let slot=e["pool_slot"].as_u64().unwrap() as usize;
     assert_eq!((event.object,event.slot,event.references,event.previous_stamp,event.frame,event.time),(target,slot,p["refs"].as_u64().unwrap() as u32,expected_stamps[slot],frame,time));continue;
    }
    4=>{let source=pending.pop_front().unwrap();let index=e["frame_event"].as_u64().unwrap() as usize;assert_eq!(pose_crc(&cache.get(source.slot).unwrap()),native[index]["pose_crc32"].as_u64().unwrap());for lease in &leases[target]{assert_eq!(pose_crc(&cache.lease_frame(lease).unwrap()),native[index]["pose_crc32"].as_u64().unwrap());retained_reads+=1;}expected_stamps[source.slot]=frame;preparations+=1;}
    _=>panic!("unknown native lifecycle event"),
   }
   let(s,o,c)=cache.metadata();assert_eq!(state_crc(s,o,c,frame),e["state_crc32"].as_u64().unwrap(),"{} event{}",profile["label"],e["id"]);
  }
  assert!(pending.is_empty());assert!(leases.iter().all(Vec::is_empty));assert!(cache.metadata().0.iter().all(|s|s.references==0));
 }
 assert_eq!((preparations,retains,releases,allocations,borrows),(686,4528,4528,24,48));assert!(retained_reads>0);
}

#[test]
fn registered_objects_share_core_predictions_and_recycle_unretained_slots(){
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-object-cache.json")).unwrap();let mut cache=WorldCache::new(8).unwrap();
 let mut cores=std::collections::HashMap::new();let mut held=None;let mut count=0;
 for c in data["cases"].as_array().unwrap(){if c["capture"]==2{continue;}
  let p=&c["before"];let(pred,frame,time,stamp)=controls(p);
  let key=serde_json::to_string(&[&p["time"],&p["inverse_dt"],&p["start"],&p["velocity"],&p["rotation_start"],&p["rotation_end"]]).unwrap();
  let core=*cores.entry(key).or_insert_with(||cache.register_core(pred));let object=cache.register_object(core,p["flags"].as_u64().unwrap() as u32,frame).unwrap();let context=Context{time,frame:stamp};
  let slot=cache.access(object,context).unwrap();let actual=cache.get(slot).unwrap();let expected=&c["after"];
  let components=actual.core_position.to_array().into_iter().chain(actual.pose.position.to_array()).chain(actual.quaternion).chain(actual.pose.rotation.transpose().to_cols_array()).collect::<Vec<_>>();
  let native=[("core_position",3),("position",3),("quaternion",4),("rotation_rows",9)].into_iter().flat_map(|(key,size)|(0..size).map(move |i|expected[key][i].as_f64().unwrap() as f32)).collect::<Vec<_>>();
  assert!(components.iter().zip(native).all(|(a,b)|(*a-b).abs()<0.00003),"{} step{}",c["model"],c["step"]);
  if held.is_none(){let lease=cache.retain(object,context).unwrap();held=Some((lease,pose_crc(&actual)));}
  let(lease,crc)=held.as_ref().unwrap();assert_eq!(pose_crc(&cache.lease_frame(lease).unwrap()),*crc);
  assert_eq!(cache.metadata().0.iter().map(|s|s.references).sum::<u32>(),1);count+=1;
 }
 assert_eq!(count,688);assert!(cores.len()<count,"shared core cases were not exercised");cache.release(held.unwrap().0).unwrap();assert!(cache.metadata().0.iter().all(|s|s.references==0));
}

#[test]
fn native_cache_detach_preserves_references_stamps_and_foreign_aliases(){
 use crate::ivp_cache_pool::{Slot,Object,detach};
 let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-cache-detach.json")).unwrap();
 for c in data["cases"].as_array().unwrap(){let bound=c["bound"].as_bool().unwrap();let refs=c["references"].as_u64().unwrap() as u32;let frame=c["frame"].as_u64().unwrap() as u32;
  let mut slots=vec![Slot{owner:Some(c["owner_before"].as_u64().unwrap() as usize),references:refs,frame_stamp:frame}];let mut objects=vec![Object{flags:257,cache:bound.then_some(0)},Object{flags:272,cache:Some(0)}];
  detach(&mut slots,&mut objects,0).unwrap();assert_eq!(objects[0].cache.is_some(),c["bound_after"].as_bool().unwrap());assert_eq!(slots[0].owner.map_or(-1,|i|i as i64),c["owner_after"].as_i64().unwrap());assert_eq!(objects[1].cache,Some(0));assert_eq!((slots[0].references,slots[0].frame_stamp),(refs,frame));
  let once=(slots.clone(),objects.clone());detach(&mut slots,&mut objects,0).unwrap();assert_eq!((slots,objects),once);
 }assert_eq!(data["cases"].as_array().unwrap().len(),64);
}
