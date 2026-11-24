use super::*;
use serde_json::{Value,json};
use crate::ivp_motion::ObjectFrame;
use crate::ivp_joint_apply::tests::{body,snapshot};
fn f(v:&Value)->f32{f32::from_bits(v.as_u64().unwrap() as u32)}
fn w(v:&Value)->u32{v.as_u64().unwrap() as u32}
fn vec(v:&Value)->bevy::prelude::Vec3{bevy::prelude::Vec3::from_array(std::array::from_fn(|i|f(&v[i])))}
fn native_paths<'a>(nodes:&'a Value,prefix:&mut Vec<Pair>,out:&mut Vec<(Vec<Pair>,&'a Value)>){for node in nodes.as_array().unwrap(){prefix.push(Pair{slots:[0,1],ledges:std::array::from_fn(|i|w(&node["ledges"][i]) as usize)});out.push((prefix.clone(),node));if let Some(children)=node.get("children"){native_paths(children,prefix,out);}prefix.pop();}}
fn check_retained_ids(before:&[Binding],after:&[Binding]){for b in before{if let Some(a)=after.iter().find(|a|a.path==b.path){assert_eq!(a.id,b.id,"retained child changed world identity");}}}
fn check_checkpoint(owner:&ownership::Ownership,parent:&Entry<'_>,bindings:&[Binding],bodies:&[Physical;2],expected:&Value,case:u64){
 let mut nodes=vec![];native_paths(&expected["tree"],&mut vec![parent.key],&mut nodes);assert_eq!(nodes.len(),bindings.len(),"case{case} checkpoint children");
 let mut ids=std::collections::BTreeMap::from([(0usize,0usize)]);
 for binding in bindings{
  let n=nodes.iter().find(|(p,_)|*p==binding.path).unwrap().1;ids.insert(binding.id,w(&n["id"]) as usize);
  let entry=parent.recursive.as_ref().unwrap().descendants.entry_at_path(&binding.path[1..]).unwrap();
  assert_eq!(entry.contact.flags&0x3fffffff,w(&n["flags"]),"case{case} checkpoint flags");assert_eq!(entry.contact.query.frame,w(&n["query_frame"]));assert_eq!(json!(entry.contact.query.kinds),n["kinds"]);
  if !n["geometry"].is_null(){let g=entry.contact.query.geometry.unwrap();let delta=owner.far(binding.id).map_or(0.,|r|r.linear_delta);assert_eq!(json!([g.projection.to_bits(),delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]),n["geometry"],"case{case} checkpoint geometry");}
  if let Some(r)=&entry.recursive{assert_eq!(r.count.load(Ordering::Relaxed),n["descendant_count"].as_i64().unwrap() as i32);assert_eq!(r.split.map_or(-1,|s|s as i32),n["split"].as_i64().unwrap() as i32);}
 }
 let g=&expected["graph"];
 assert_eq!(json!(owner.graph.active().iter().map(|id|ids[id]).collect::<Vec<_>>()),g["active"],"case{case} checkpoint active");assert_eq!(json!(owner.graph.inactive().iter().map(|id|ids[id]).collect::<Vec<_>>()),g["inactive"]);
 for side in 0..2{let h=&owner.hulls[side];let rows=h.ordered_keys().iter().map(|(handle,token,key)|json!({"handle":handle,"pair":ids[&(token/2)],"key":key})).collect::<Vec<_>>();assert_eq!(json!(rows),g["hulls"][side]["entries"],"case{case} checkpoint hulls");assert_eq!(h.budget.threshold.to_bits(),w(&g["hulls"][side]["threshold"]));}
 let q=owner.queue.snapshot();let rows=q["entries"].as_array().unwrap().iter().map(|r|json!({"handle":r["handle"],"pair":ids.get(&(r["owner"].as_u64().unwrap() as usize)).copied().unwrap_or(9999),"key":r["offset_bits"]})).collect::<Vec<_>>();assert_eq!(json!(rows),g["queue"],"case{case} checkpoint queue");
 assert_eq!(json!(bodies.map(snapshot)),expected["physical"],"case{case} checkpoint body bank");
}
#[test]
fn recursive_children_construct_real_query_graph_and_hull_state_like_native(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-tree.json")).unwrap(),false,2822);
}
#[test]
fn recursive_initial_query_selects_real_native_expansion_or_parent_schedule(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-initial.json")).unwrap(),true,1902);
}
#[test]
fn recursive_existing_trees_reconcile_without_losing_native_children_or_resources(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-reconcile.json")).unwrap(),true,7392);
}
#[test]
fn recursive_parent_callbacks_match_native_queries_collapse_and_hull_rearm(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-callback.json")).unwrap(),true,5676);
}
#[test]
fn recursive_return_callbacks_recreate_native_children_through_live_dispatch(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-return.json")).unwrap(),true,1902);
}
#[test]
fn recursive_phase5_far_contacts_rearm_and_reactivate_like_native(){
 check(serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-far.json")).unwrap(),true,9012);
}
fn check(data:Value,initial:bool,expected_children:usize){
 let phy=std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-bounds-1025-bound-cores/4.phy")).expect("restore hash-verified native pallet geometry");let pallet=l4d2_formats::phy::parse_phy(&phy).unwrap();let hex=data["box"].as_str().unwrap();let bytes=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(bytes.len() as u32).to_le_bytes().to_vec();packet.extend(bytes);let boxes=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
 let mut total=0;
 for c in data["cases"].as_array().unwrap(){
  let mut solids=std::array::from_fn::<_,2,_>(|i|if c["flags"][i]==0{boxes[0].clone()}else{pallet[0].clone()});for i in 0..2{let tree=Arc::make_mut(solids[i].compact_tree.as_mut().unwrap());tree.nodes[tree.root].radius=f(&c["radii"][i]);}
  let mut bodies=[body(&c["before"][0]),body(&c["before"][1])];let mut cache=WorldCache::new(8).unwrap();for i in 0..2{cache.register_core(bodies[i].prediction);cache.register_object(i,bodies[i].object_flags,ObjectFrame{shift:(bodies[i].object_flags&0x800==0).then(||vec(&c["shifts"][i])),rotation:None}).unwrap();}
  let root=Arc::new(AtomicI32::new(c["initial_count"].as_i64().unwrap() as i32));let own=Arc::new(AtomicI32::new(0));let mut descendants=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();descendants.listeners.push(own.clone());let key=Pair{slots:[0,1],ledges:std::array::from_fn(|i|w(&c["ledges"][i]) as usize)};
  let mut parent=Entry{key,contact:crate::ivp_minimizer::owner::Contact::new([0,1],crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS,f(&c["object_margins"][0])+f(&c["object_margins"][1])).unwrap(),recursive:Some(Box::new(Recursive{split:None,count:own,descendants}))};
  let mut owner=ownership::Ownership::new(2,8,8,0.).unwrap();let parent_id=owner.graph.register([0,1]).unwrap();if !initial{owner.graph.activate(parent_id,[false;2]).unwrap();parent.contact.flags=owner.graph.flags(parent_id).unwrap();}owner.counter=w(&c["counter_before"]);
  for i in 0..2{let budget=&c["budgets"][i];owner.hulls[i].budget.time=f64::from_bits(budget["time"].as_u64().unwrap());owner.hulls[i].budget.rates=std::array::from_fn(|j|f(&budget["rates"][j]));owner.hulls[i].budget.accumulated=std::array::from_fn(|j|f(&budget["accumulated"][j]));owner.hulls[i].budget.predicted=f(&c["hull_inputs"][i]["predicted"]);assert!(c["hull_inputs"][i]["entries"].as_array().unwrap().is_empty());}
  let mut queued=c["queue_inputs"].as_array().unwrap().clone();queued.sort_by_key(|r|w(&r["id"]));for r in queued{assert_eq!(owner.queue.insert(1000+w(&r["id"]) as usize,f(&r["key"])).unwrap() as u32,w(&r["id"]));}if c["queued"].as_bool().unwrap(){owner.queue.insert(parent_id,f(&c["queue_offset"])).unwrap();}
  let timing=Schedule{deadline:f64::from_bits(c["deadline"].as_u64().unwrap()),dt:f(&c["dt"]),delay:0,config:crate::ivp_distance_schedule::Config{slow_gate:f(&c["config"]["slow_gate"]),time_scale:f(&c["config"]["time_scale"]),gap:f(&c["config"]["gap"]),targets:std::array::from_fn(|i|f(&c["config"]["targets"][i]))},face_epsilon:f32::from_bits(975594773),properties:std::array::from_fn(|i|{let p=&c["properties"][i];crate::ivp_minimizer::owner::Properties{radius:f(&p["radius"]),inverse_diameter:f(&p["inverse_diameter"]),angular_speed:f(&p["angular_speed"]),angular_bound:f(&p["angular_bound"]),sweep_axis:vec(&p["axis"])}}),budgets:[crate::ivp_distance_schedule::Budget{time:0.,flags:0,rates:[0.;2],accumulated:[0.;2]};2],allow_far:false};
  let clock=Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])};for object in 0..2{cache.access(object,clock).unwrap();}let cache_before=format!("{:?}",cache.metadata());
  let mut production=None;
  let mut bindings={let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])};if initial&&c.get("before_update").is_none()&&c.get("callback_before").is_none()&&c.get("far_before").is_none()&&c["initial_count"]==0{production=Some(crate::ivp_surface_admission::contacts::discovery::owner::construct_for_native_check(&owner,parent_id,&parent,std::array::from_fn(|i|solids[i].compact_ledge(key.ledges[i]).unwrap()),&context));}(if initial{initialize_new_tree(&mut owner,parent_id,&mut parent,std::array::from_fn(|i|solids[i].compact_ledge(key.ledges[i]).unwrap()),&mut context)}else{expand_new_tree(&mut owner,parent_id,&mut parent,&mut context)}).unwrap_or_else(|e|panic!("case{} construct: {e}",c["id"]))};
  if let Some(before)=c.get("far_before"){
   bodies=std::array::from_fn(|i|body(&before["physical"][i]));check_checkpoint(&owner,&parent,&bindings,&bodies,before,c["id"].as_u64().unwrap());assert_eq!(root.load(Ordering::Relaxed),before["count"].as_i64().unwrap() as i32);
   if c["far_invoked"].as_bool().unwrap(){
    for object in 0..2{cache.detach(object).unwrap();}
    {let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])}; context.clock.frame=w(&before["frame"]);if c["initial_count"]==0{crate::ivp_surface_admission::contacts::discovery::hierarchy::assert_live_far_recheck(&owner,parent_id,&parent,&bindings,&context,f(&c["far_gap"]));}recheck_parent(&mut owner,parent_id,&mut parent,std::array::from_fn(|i|solids[i].compact_ledge(key.ledges[i]).unwrap()),&mut bindings,&mut context,f(&c["far_gap"])).unwrap_or_else(|e|panic!("case{} far recheck: {e}",c["id"]));}
   }
  }
  if let Some(before)=c.get("callback_before"){
   bodies=std::array::from_fn(|i|body(&before["physical"][i]));
   check_checkpoint(&owner,&parent,&bindings,&bodies,before,c["id"].as_u64().unwrap());assert_eq!(root.load(Ordering::Relaxed),before["count"].as_i64().unwrap() as i32);
   if c["callback_invoked"].as_bool().unwrap(){
    for object in 0..2{cache.detach(object).unwrap();}
    {let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])}; context.clock.frame=w(&before["frame"]);if c["initial_count"]==0{if let Some(next)=c.get("return_before"){let phases=[(*context.bodies,context.clock),(std::array::from_fn(|i|body(&next["physical"][i])),Clock{time:context.clock.time,frame:w(&next["frame"])})];crate::ivp_surface_admission::contacts::discovery::hierarchy::assert_live_parent_sequence(&owner,parent_id,&parent,&bindings,&context,f(&c["collapse_threshold"]),&phases);}else{crate::ivp_surface_admission::contacts::discovery::hierarchy::assert_live_parent_callback(&owner,parent_id,&parent,&bindings,&context,f(&c["collapse_threshold"]));}}refresh_parent(&mut owner,parent_id,&mut parent,std::array::from_fn(|i|solids[i].compact_ledge(key.ledges[i]).unwrap()),&mut bindings,&mut context,f(&c["collapse_threshold"])).unwrap_or_else(|e|panic!("case{} parent callback: {e}",c["id"]));}
   }
  }
  if let Some(before)=c.get("return_before"){
   let middle=&c["middle_callback"];check_checkpoint(&owner,&parent,&bindings,&bodies,middle,c["id"].as_u64().unwrap());assert_eq!(root.load(Ordering::Relaxed),middle["count"].as_i64().unwrap() as i32);
   bodies=std::array::from_fn(|i|body(&before["physical"][i]));check_checkpoint(&owner,&parent,&bindings,&bodies,before,c["id"].as_u64().unwrap());
   for object in 0..2{cache.detach(object).unwrap();}
   {let old_bindings=bindings.clone();let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])}; context.clock.frame=w(&before["frame"]);refresh_parent(&mut owner,parent_id,&mut parent,std::array::from_fn(|i|solids[i].compact_ledge(key.ledges[i]).unwrap()),&mut bindings,&mut context,f(&c["collapse_threshold"])).unwrap_or_else(|e|panic!("case{} return callback: {e}",c["id"]));check_retained_ids(&old_bindings,&bindings);}
  }
  if let Some(before)=c.get("before_update"){
   check_checkpoint(&owner,&parent,&bindings,&bodies,before,c["id"].as_u64().unwrap());assert_eq!(root.load(Ordering::Relaxed),before["count"].as_i64().unwrap() as i32);
   if c["update_invoked"].as_bool().unwrap(){
    {let old_bindings=bindings.clone();let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])}; update_tree(&mut owner,parent_id,&mut parent,&mut bindings,&mut context,f(&c["update_margin"])).unwrap_or_else(|e|panic!("case{} reconcile: {e}",c["id"]));check_retained_ids(&old_bindings,&bindings);}
    if let Some(middle)=c.get("middle_update"){
     check_checkpoint(&owner,&parent,&bindings,&bodies,middle,c["id"].as_u64().unwrap());assert_eq!(root.load(Ordering::Relaxed),middle["count"].as_i64().unwrap() as i32);
     {let old_bindings=bindings.clone();let mut context=Context{cache:&mut cache,bodies:&mut bodies,clock:Clock{time:f64::from_bits(c["now"].as_u64().unwrap()),frame:w(&c["frame"])},timing,gravity:vec(&c["gravity"]).to_array(),margins:std::array::from_fn(|i|f(&c["object_margins"][i])),motion:std::array::from_fn(|i|[f(&c["hull_inputs"][i]["linear"]),f(&c["hull_inputs"][i]["angular"])]),root:root.clone(),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"])}; update_tree(&mut owner,parent_id,&mut parent,&mut bindings,&mut context,f(&c["second_update_margin"])).unwrap_or_else(|e|panic!("case{} second reconcile: {e}",c["id"]));check_retained_ids(&old_bindings,&bindings);}
    }
   }
  }
  if !initial&&c["calls"]==0{assert_eq!(format!("{:?}",cache.metadata()),cache_before,"case{} rejected count touched caches",c["id"]);}
  assert_eq!(parent.recursive.as_ref().unwrap().count.load(Ordering::Relaxed),c["delta"].as_i64().unwrap() as i32);
  if initial{
   let n=&c["root_node"];assert_eq!(parent.contact.flags&0x3fffffff,w(&n["flags"]),"case{} root flags",c["id"]);assert_eq!(parent.contact.query.frame,w(&n["query_frame"]));assert_eq!(json!(parent.contact.query.kinds),n["kinds"]);assert_eq!(parent.recursive.as_ref().unwrap().split.map_or(-1,|s|s as i32),c["split"].as_i64().unwrap() as i32);
   if !n["geometry"].is_null(){let g=parent.contact.query.geometry.unwrap();let delta=owner.far(parent_id).map_or(0.,|r|r.linear_delta);assert_eq!(json!([g.projection.to_bits(),delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]),n["geometry"],"case{} root geometry",c["id"]);}
  }
  let mut expected=vec![];native_paths(&c["tree"],&mut vec![key],&mut expected);assert_eq!(bindings.len(),expected.len(),"case{} node count",c["id"]);
  let mut identities=std::collections::BTreeMap::from([(parent_id,0usize)]);for binding in &bindings{let n=expected.iter().find(|(path,_)|*path==binding.path).unwrap().1;identities.insert(binding.id,w(&n["id"]) as usize);}
  let mut forest=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();forest.entries.push(parent.snapshot_with_counters(&mut CounterSnapshot::default()));
  for binding in &bindings{let expected=expected.iter().find(|(path,_)|*path==binding.path).unwrap().1;let actual=forest.entry_at_path(&binding.path).unwrap();assert_eq!(owner.graph.flags(binding.id).unwrap()&0x3fffffff,w(&expected["flags"]),"case{} node{} flags",c["id"],expected["id"]);assert_eq!(actual.contact.query.frame,w(&expected["query_frame"]));assert_eq!(json!(actual.contact.query.kinds),expected["kinds"]);
   if expected["recursive"].as_bool().unwrap(){let r=actual.recursive.as_ref().unwrap();assert_eq!(r.split.map_or(-1,|s|s as i32),expected["split"].as_i64().unwrap() as i32);assert_eq!(r.count.load(Ordering::Relaxed),expected["descendant_count"].as_i64().unwrap() as i32);}else{assert!(actual.recursive.is_none());}
   if !expected["geometry"].is_null(){let g=actual.contact.query.geometry.unwrap();let delta=owner.far(binding.id).map_or(0.,|r|r.linear_delta);assert_eq!(json!([g.projection.to_bits(),delta.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()]),expected["geometry"],"case{} node{} geometry",c["id"],expected["id"]);}
   let event=owner.queue.scheduled(binding.id).map(|(_,key)|key.to_bits());assert_eq!(json!(event),expected["event_key"],"case{} node{} event key",c["id"],expected["id"]);
   let queue=owner.queue.snapshot();let handle=queue["entries"].as_array().unwrap().iter().find(|r|r["owner"].as_u64()==Some(binding.id as u64)).map_or(u16::MAX,|r|w(&r["handle"]) as u16);assert_eq!(handle as u32,w(&expected["handle"]),"case{} node{} event handle",c["id"],expected["id"]);
   for side in 0..2{let expected=&expected["far"][side];if expected.is_null(){assert!(owner.far(binding.id).is_none());}else{let r=owner.far(binding.id).unwrap();let h=r.handles[side];assert_eq!(h as u32,w(&expected["handle"]),"case{} node hull handle",c["id"]);assert_eq!(owner.hulls[side].entry(h).unwrap().1.to_bits(),w(&expected["key"]),"case{} node hull key",c["id"]);}}
  }
  assert_eq!(json!(owner.graph.active().iter().map(|id|identities[id]).collect::<Vec<_>>()),c["graph_after"]["active"],"case{} active order",c["id"]);assert_eq!(json!(owner.graph.inactive().iter().map(|id|identities[id]).collect::<Vec<_>>()),c["graph_after"]["inactive"],"case{} inactive order",c["id"]);
  for side in 0..2{let expected=&c["graph_after"]["hulls"][side];assert_eq!(owner.hulls[side].budget.threshold.to_bits(),w(&expected["threshold"]),"case{} hull threshold",c["id"]);let entries=owner.hulls[side].ordered_keys().iter().map(|(handle,token,key)|json!({"handle":handle,"pair":identities[&(token/2)],"key":key})).collect::<Vec<_>>();assert_eq!(json!(entries),expected["entries"],"case{} hull order",c["id"]);}
  let event_queue=owner.queue.snapshot();let events=event_queue["entries"].as_array().unwrap().iter().map(|r|{let id=r["owner"].as_u64().unwrap() as usize;json!({"handle":r["handle"],"pair":identities.get(&id).copied().unwrap_or(9999),"key":r["offset_bits"]})}).collect::<Vec<_>>();assert_eq!(json!(events),c["graph_after"]["queue"],"case{} complete event queue",c["id"]);
  assert_eq!(owner.counter,w(&c["counter_after"]),"case{} collision counter",c["id"]);assert_eq!(json!(bodies.map(snapshot)),c["physical_after"],"case{} complete physical bank",c["id"]);assert_eq!(root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32);assert!(cache.metadata().0.iter().all(|s|s.references==0));
  if let Some((root,created,candidate,bank))=production{
   assert_eq!(json!(bank.map(snapshot)),json!(bodies.map(snapshot)),"case{} production body bank",c["id"]);assert_eq!(candidate.queue.snapshot(),owner.queue.snapshot(),"case{} production queue",c["id"]);assert_eq!(candidate.graph.active(),owner.graph.active());assert_eq!(candidate.graph.inactive(),owner.graph.inactive());
   assert_eq!(format!("{:?}",root.contact),format!("{:?}",parent.contact));assert_eq!(created.descendants.len(),bindings.len());
   let mut forest=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();forest.entries.push(root);
   for (id,path,far) in std::iter::once((created.id,vec![key],created.far)).chain(created.descendants.iter().map(|d|{let mut path=vec![key];path.extend_from_slice(&d.path);(d.id,path,d.far)})){
    let actual=if path.len()==1{&parent}else{let binding=bindings.iter().find(|b|b.path==path).unwrap();assert_eq!(binding.id,id);forest.entry_at_path(&path).unwrap()};
    if path.len()>1{let e=parent.recursive.as_ref().unwrap().descendants.entry_at_path(&path[1..]).unwrap();assert_eq!(format!("{:?}",actual.contact),format!("{:?}",e.contact));}
    match (far,owner.far(id)){(None,None)=>{},(Some((keys,delta)),Some(record))=>{assert_eq!(keys.map(f32::to_bits),std::array::from_fn(|i|owner.hulls[record.objects[i]].entry(record.handles[i]).unwrap().1.to_bits()));assert_eq!(delta.to_bits(),record.linear_delta.to_bits());},_=>panic!("production far membership differs")}
    assert!(candidate.far(id).is_none());
   }
  }
  crate::ivp_surface_admission::contacts::discovery::hierarchy::assert_native_tree_handoff(&owner,parent_id,&parent,&bindings,&bodies);
  retire_tree(&mut owner,parent_id,&mut parent,&bindings).unwrap();assert_eq!(root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32);assert!(owner.graph.active().is_empty());assert!(owner.graph.inactive().is_empty());for h in &owner.hulls{assert!(h.ordered_keys().is_empty());}
  let q=owner.queue.snapshot();let events=q["entries"].as_array().unwrap().iter().map(|r|{let id=r["owner"].as_u64().unwrap() as usize;json!({"handle":r["handle"],"pair":identities.get(&id).copied().unwrap_or(9999),"key":r["offset_bits"]})}).collect::<Vec<_>>();assert_eq!(json!(events),c["graph_after_delete"]["queue"],"case{} destructor queue",c["id"]);for side in 0..2{assert_eq!(owner.hulls[side].budget.threshold.to_bits(),w(&c["graph_after_delete"]["hulls"][side]["threshold"]));}assert_eq!(json!(bodies.map(snapshot)),c["physical_after_delete"],"case{} destructor body bank",c["id"]);total+=bindings.len();
 }
 assert_eq!(total,expected_children);
}
