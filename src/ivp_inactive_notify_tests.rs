use super::*;
use crate::ivp_joint_apply::tests::{body,snapshot};
use crate::ivp_minimizer::owner::{Properties,Schedule};
use l4d2_formats::phy::{CompactTopology,CompactEdge,TriangleHeader};
#[test]
fn fresh_object_notifications_requery_inactive_contacts_and_reactivate_in_native_order(){
 let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-inactive-notify.json")).unwrap();
 let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));
 let mut counts=[0usize;6];let mut notification_counts=[0usize;3];
 for c in d["cases"].as_array().unwrap(){
  let mut bodies=std::array::from_fn(|i|body(&c["before"][i]));
  let convexes:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|Convex{
   topology:Some(Arc::new(CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()})),
   triangle_headers:(0..4).map(|i|TriangleHeader(w(&h[4+i*4]))).collect(),triangles:vec![],planes:vec![],edges:vec![],bevels:vec![],mins:Default::default(),maxs:Default::default(),
  }).collect();
  let mut clock=crate::ivp_world_cache::Context{time:bodies[0].clock,frame:w(&c["frame"])};
  let mut cache=crate::ivp_world_cache::WorldCache::new(8).unwrap();
  for i in 0..2{let b=body(&c["initial"][i]);assert_eq!(cache.register_core(b.prediction),b.id);assert_eq!(cache.register_object(b.id,b.object_flags,crate::ivp_motion::ObjectFrame{shift:None,rotation:None}).unwrap(),b.id);}
  let initial=crate::ivp_world_cache::Context{time:f64::from_bits(c["initial_clock"].as_u64().unwrap()),frame:w(&c["initial_frame"])};
  for i in 0..2{cache.access(i,initial).unwrap();}
  let mut owner=ownership::Ownership::new(2,8,8,f64::from_bits(c["timing"]["epoch"].as_u64().unwrap())).unwrap();
  for j in 0..2{
   let initial=&c["hull_initial"][j];let mut entries=initial["entries"].as_array().unwrap().clone();entries.sort_by_key(|e|w(&e["id"]));
   for e in entries{assert_eq!(owner.hulls[j].insert(1000+w(&e["id"]) as usize,f(&e["key"])).unwrap(),w(&e["id"]) as u16);}
   assert_eq!(owner.hulls[j].ordered_keys().iter().map(|(h,_,k)|serde_json::json!({"id":h,"key":k})).collect::<Vec<_>>(),initial["entries"].as_array().unwrap().clone());
   assert_eq!(owner.hulls[j].budget.threshold.to_bits(),w(&initial["threshold"]));
  }
  let id=owner.graph.register([0,1]).unwrap();owner.graph.set_flags(id,w(&c["input_flags"])).unwrap();
  let mut entry=Entry{key:Pair{slots:[0,1],ledges:[0,0]},contact:Contact::new([0,1],w(&c["input_flags"]),0.02).unwrap(),recursive:None};
  let t=&c["timing"];owner.counter=w(&t["counter_before"]);
  let config=crate::ivp_distance_schedule::Config{targets:std::array::from_fn(|j|f(&t["targets"][j])),slow_gate:f(&t["slow_gate"]),time_scale:f(&t["time_scale"]),gap:f(&t["gap"])};
  let properties=std::array::from_fn(|j|{let p=&t["metadata"][j];Properties{radius:f(&p["radius"]),inverse_diameter:f(&p["inverse_diameter"]),angular_speed:f(&p["angular_speed"]),angular_bound:f(&p["angular_bound"]),sweep_axis:Vec3::from_array(std::array::from_fn(|k|f(&p["axis"][k])))}});
  let budgets=std::array::from_fn(|j|{let p=&t["metadata"][j];crate::ivp_distance_schedule::Budget{time:f64::from_bits(p["budget_time"].as_u64().unwrap()),flags:w(&p["budget_flags"]),rates:std::array::from_fn(|k|f(&p["rates"][k])),accumulated:std::array::from_fn(|k|f(&p["accumulated"][k]))}});
  let (update,result)=entry.initialize_ordinary_owned(&mut owner,id,[false,false],&mut cache,clock,&mut bodies,[&convexes[0],&convexes[1]],std::array::from_fn(|k|f(&c["gravity"][k])),Schedule{deadline:f64::from_bits(t["deadline"].as_u64().unwrap()),dt:f(&c["dt"]),delay:255,config,face_epsilon:f(&t["face_epsilon"]),properties,budgets,allow_far:false}).unwrap_or_else(|e|panic!("case{} ordinary: {e}",c["id"]));
  counts[0]+=usize::from(entry.contact.flags&0xc000!=0);
  assert_eq!(owner.graph.inactive().contains(&id),c["inactive_head"].as_bool().unwrap());
  for j in 0..2{assert_eq!(!owner.graph.inactive_object_links(j).unwrap().is_empty(),c["inactive_object_heads"][j].as_bool().unwrap());}
  assert_eq!(entry.contact.flags,w(&c["flags"]),"case{} flags",c["id"]);assert_eq!(owner.graph.flags(id).unwrap(),entry.contact.flags);
  assert_eq!(entry.contact.query.frame,w(&c["query_frame"]));
  for (j,e) in [entry.contact.query.vertex,entry.contact.query.face].into_iter().enumerate(){assert_eq!([e.triangle,e.edge,entry.contact.query.kinds[j] as usize],std::array::from_fn(|k|w(&c["features"][j][k]) as usize),"case{} feature{j}",c["id"]);}
  if let Some(g)=entry.contact.query.geometry{let n=&c["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[w(&n[0]),w(&n[2]),w(&n[3]),w(&n[4]),w(&n[5])],"case{} geometry",c["id"]);}
  assert_eq!(owner.graph.active().contains(&id),c["global_head"].as_bool().unwrap(),"case{} global",c["id"]);
  for j in 0..2{assert_eq!(!owner.graph.object_links(j).unwrap().is_empty(),c["object_heads"][j].as_bool().unwrap());}
  match result{Registration::Scheduled(out)=>{assert_eq!(out.kind,w(&c["kind"]));if out.kind!=0{assert_eq!(out.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap());counts[2]+=1;}counts[1]+=usize::from(out.far.is_some());},Registration::InvalidGeometry=>assert_eq!(owner.graph.inactive(),&[id])}
  assert_eq!(owner.counter,w(&c["counter_after"]));assert_eq!(owner.queue.scheduled(id).map(|(_,k)|k.to_bits()),c["key"].as_u64().map(|v|v as u32),"case{} queue",c["id"]);counts[3]+=usize::from(owner.queue.scheduled(id).is_some());
  assert_eq!(serde_json::json!(bodies.map(snapshot)),c["after"],"case{} complete bodies",c["id"]);counts[4]+=usize::from(c["before"]!=c["after"]);
  assert_eq!(usize::from(update.callback_references.is_some()),w(&c["callback_count"]) as usize);counts[5]+=usize::from(update.callback_references.is_some());
  if update.callback_references.is_some(){assert_eq!(update.callback_references,Some([0,0]));}
  assert!(cache.metadata().0.iter().all(|s|s.references==0));
  for j in 0..2{
   let after=&c["hull_after"][j];let h=&owner.hulls[j];assert_eq!(h.ordered_keys().len(),w(&after["count"]) as usize);
   assert_eq!(h.first().map(|(id,_,_)|id as u32).unwrap_or(65535),w(&after["head"]));assert_eq!(h.budget.threshold.to_bits(),w(&after["threshold"]));
   if let Some(record)=owner.far(id){assert_eq!(record.objects,[0,1]);assert_eq!(record.handles[j] as u32,w(&after["handle"]));let (listener,key)=h.entry(record.handles[j]).unwrap();assert_eq!(listener,id*2+j);assert_eq!(key.to_bits(),w(&after["key"]));assert_eq!(record.linear_delta.to_bits(),w(&c["linear_delta"]));}
   else{assert!(after["handle"].is_null());}
  }
  let r=&c["notification"];
  if r["called"]==true{
   bodies=std::array::from_fn(|j|body(&r["before"][j]));clock=crate::ivp_world_cache::Context{time:f64::from_bits(r["clock"].as_u64().unwrap()),frame:w(&r["environment_frame"])};
   assert_eq!(owner.notify_object(0,|o,pair,side|{
    assert_eq!((pair,side),(id,0));
    let (update,active)=entry.refresh_inactive_owned(o,id,[false,false],&mut cache,clock,&mut bodies,[&convexes[0],&convexes[1]],f(&c["dt"]),std::array::from_fn(|k|f(&c["gravity"][k])))?;
    assert_ne!(update.outcome,crate::ivp_minimizer::Refresh::Reused);assert_eq!(active,r["active"].as_bool().unwrap());notification_counts[usize::from(!active)]+=1;notification_counts[2]+=usize::from(update.callback_references.is_some());
    assert_eq!(usize::from(update.callback_references.is_some()),w(&r["callback_count"]) as usize);if update.callback_references.is_some(){assert_eq!(update.callback_references,Some([0,0]));}Ok(())
   }).unwrap_or_else(|e|panic!("case{} notify: {e}",c["id"])),1);
   assert_eq!(entry.contact.flags,w(&r["flags"]));assert_eq!(owner.graph.flags(id).unwrap(),entry.contact.flags);assert_eq!(entry.contact.query.frame,w(&r["frame"]));
   assert_eq!(owner.graph.active().contains(&id),r["active"].as_bool().unwrap());assert_eq!(owner.graph.inactive().contains(&id),r["inactive"].as_bool().unwrap());
   for j in 0..2{assert_eq!(!owner.graph.object_links(j).unwrap().is_empty(),r["active_objects"][j].as_bool().unwrap());assert_eq!(!owner.graph.inactive_object_links(j).unwrap().is_empty(),r["inactive_objects"][j].as_bool().unwrap());}
   for (j,e) in [entry.contact.query.vertex,entry.contact.query.face].into_iter().enumerate(){assert_eq!([e.triangle,e.edge,entry.contact.query.kinds[j] as usize],std::array::from_fn(|k|w(&r["features"][j][k]) as usize),"case{} notify feature{j}",c["id"]);}
   if let Some(g)=entry.contact.query.geometry{let n=&r["geometry"];assert_eq!([g.projection.to_bits(),g.distance.to_bits(),g.normal.x.to_bits(),g.normal.y.to_bits(),g.normal.z.to_bits()],[w(&n[0]),w(&n[2]),w(&n[3]),w(&n[4]),w(&n[5])],"case{} notify geometry",c["id"]);}
   assert_eq!(serde_json::json!(bodies.map(snapshot)),r["after"]);
   let repeat_flags=entry.contact.flags;
   assert_eq!(owner.notify_object(1,|o,pair,side|{assert_eq!((pair,side),(id,1));let (again,active)=entry.refresh_inactive_owned(o,id,[false,false],&mut cache,clock,&mut bodies,[&convexes[0],&convexes[1]],f(&c["dt"]),std::array::from_fn(|k|f(&c["gravity"][k])))?;assert_eq!(again.outcome,crate::ivp_minimizer::Refresh::Reused);assert!(!active);assert!(again.callback_references.is_none());Ok(())}).unwrap(),usize::from(r["inactive"].as_bool().unwrap()));
   assert_eq!(entry.contact.flags,repeat_flags);assert_eq!(serde_json::json!(bodies.map(snapshot)),r["repeat_after"]);assert!(owner.queue.scheduled(id).is_none());assert!(cache.metadata().0.iter().all(|s|s.references==0));
  }
  owner.retire(id).unwrap();assert!(owner.graph.active().is_empty());assert!(owner.graph.fast().is_empty());assert!(owner.graph.inactive().is_empty());assert!(owner.graph.inactive_object_links(0).unwrap().is_empty());assert!(owner.graph.inactive_object_links(1).unwrap().is_empty());assert!(owner.far(id).is_none());assert!(owner.queue.scheduled(id).is_none());
  for j in 0..2{
   let h=&owner.hulls[j];let clean=&c["hull_cleanup"][j];assert_eq!(h.ordered_keys().len(),w(&clean["count"]) as usize);assert_eq!(h.budget.threshold.to_bits(),w(&clean["threshold"]));
   assert_eq!(h.ordered_keys().iter().map(|(id,_,key)|serde_json::json!({"id":id,"key":key})).collect::<Vec<_>>(),c["hull_initial"][j]["entries"].as_array().unwrap().clone());
  }
 }
 assert_eq!(notification_counts,[39,150,98]);assert_eq!(counts,[189,582,32,32,24,126]);assert_eq!(d["cases"].as_array().unwrap().len(),1152);
}
