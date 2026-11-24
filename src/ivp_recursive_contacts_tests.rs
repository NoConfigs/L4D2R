use super::*;
use crate::ivp_motion::{PreparedObjectCache,ObjectFrame};
struct Scene{frames:[PreparedObjectCache;2],input:Input,root:Counter,limit:i32,fallback:f32,margin:f32,frame:u32}
fn retire<'a>(_:&Pair,e:&mut Entry<'a>)->Result<(),&'static str>{if let Some(r)=e.recursive.as_mut(){r.descendants.clear(retire)?;}Ok(())}
fn initialize<'a>(_:&Pair,e:&mut Entry<'a>,convexes:[&Convex;2],s:&Scene)->Result<(),&'static str>{
 e.contact.flags=(e.contact.flags&0xffcfffff)|0xc0000;
 let outcome=e.contact.query.refresh_contact(s.frame,convexes.map(|c|c.topology.as_deref().unwrap()),s.frames,s.margin,convexes.map(|c|c.triangle_headers.as_slice()),&mut e.contact.flags)?;
 if matches!(outcome,crate::ivp_minimizer::Refresh::Retirement{..})&&e.recursive.is_some(){e.expand(s.input,&s.root,s.limit,s.fallback,|k,e,c|initialize(k,e,c,s),retire)?;}
 Ok(())
}
fn tree(entries:&[Entry<'_>])->serde_json::Value{serde_json::json!(entries.iter().map(|e|{let mut p=serde_json::json!({"ledges":e.key.ledges,"kinds":e.contact.query.kinds,"recursive":e.recursive.is_some()});if let Some(r)=e.recursive.as_ref(){p["split"]=serde_json::json!(r.split.map_or(-1,|n|n as i32));p["descendant_count"]=serde_json::json!(r.count.load(Ordering::Relaxed));p["children"]=tree(r.descendants.entries());}p}).collect::<Vec<_>>())}
fn staged_replay(parent:&Entry<'_>,s:&Scene,c:&serde_json::Value,dispatch:bool){
 let original_tree=tree(std::slice::from_ref(parent));let original_root=s.root.load(Ordering::Relaxed);
 if c["calls"]==1&&c["children"].as_u64().unwrap()>0{
  let mut rejected_counters=CounterSnapshot::default();let mut rejected=parent.snapshot_with_counters(&mut rejected_counters);
  let rejected_scene=Scene{frames:s.frames,input:s.input,root:rejected_counters.counter(&s.root),limit:s.limit,fallback:s.fallback,margin:s.margin,frame:s.frame};
  let mut reached=false;
  let mut initialize_rejected=|key:&Pair,entry:&mut Entry<'_>,convexes:[&Convex;2]|{initialize(key,entry,convexes,&rejected_scene)?;reached=true;Err("reject recursive child initialization")};
  let result=if dispatch{rejected.dispatch_contact(s.input,&rejected_scene.root,s.limit,s.fallback,|_|Ok(()),&mut initialize_rejected,retire).map(|_|())}else{rejected.expand(s.input,&rejected_scene.root,s.limit,s.fallback,&mut initialize_rejected,retire).map(|_|())};
  assert!(reached);assert_eq!(result,Err("reject recursive child initialization"));
  assert_eq!(tree(std::slice::from_ref(parent)),original_tree);assert_eq!(s.root.load(Ordering::Relaxed),original_root);
 }
 let mut counters=CounterSnapshot::default();let mut candidate=parent.snapshot_with_counters(&mut counters);
 let candidate_scene=Scene{frames:s.frames,input:s.input,root:counters.counter(&s.root),limit:s.limit,fallback:s.fallback,margin:s.margin,frame:s.frame};
 assert!(!Arc::ptr_eq(&candidate_scene.root,&s.root));
 let physical=std::cell::Cell::new(0);
 if dispatch{candidate.dispatch_contact(s.input,&candidate_scene.root,s.limit,s.fallback,|_|{physical.set(physical.get()+1);Ok(())},|k,e,c|initialize(k,e,c,&candidate_scene),retire).unwrap();assert_eq!(physical.get(),c["physical_calls"].as_u64().unwrap() as usize);}
 else{candidate.expand(s.input,&candidate_scene.root,s.limit,s.fallback,|k,e,c|initialize(k,e,c,&candidate_scene),retire).unwrap();}
 assert_eq!(tree(candidate.recursive.as_ref().unwrap().descendants.entries()),c["tree"],"staged case{}",c["id"]);
 assert_eq!(candidate_scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32);
 assert_eq!(tree(std::slice::from_ref(parent)),original_tree);assert_eq!(s.root.load(Ordering::Relaxed),original_root);
 // Clone an expanded hierarchy, then tear it down independently.
 let expanded_tree=tree(std::slice::from_ref(&candidate));let mut second=CounterSnapshot::default();let mut expanded=candidate.snapshot_with_counters(&mut second);let root=second.counter(&candidate_scene.root);
 let key=expanded.key;retire(&key,&mut expanded).unwrap();assert_eq!(root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32);
 assert_eq!(tree(std::slice::from_ref(&candidate)),expanded_tree);assert_eq!(candidate_scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32);
 let key=candidate.key;retire(&key,&mut candidate).unwrap();assert_eq!(candidate_scene.root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32);
 assert_eq!(tree(std::slice::from_ref(parent)),original_tree);assert_eq!(s.root.load(Ordering::Relaxed),original_root);
}

#[test]
fn recursive_split_requests_child_hierarchies_and_counter_teardown_match_native(){check_split(false);}
#[test]
fn registered_recursive_parent_transfers_lists_and_hull_listeners_before_children(){check_split(true);}
fn parent_packet(owner:&ownership::Ownership,id:usize)->serde_json::Value{
 let objects=owner.graph.objects(id).unwrap();let queue=owner.queue.snapshot();let handle=queue["entries"].as_array().unwrap().iter().find(|e|e["owner"].as_u64()==Some(id as u64)).map_or(u16::MAX,|e|e["handle"].as_u64().unwrap() as u16);
 serde_json::json!({"flags":owner.graph.flags(id).unwrap(),"active":owner.graph.active().contains(&id),"inactive":owner.graph.inactive().contains(&id),"handle":handle,"object_active":objects.map(|o|owner.graph.object_links(o).unwrap().iter().any(|l|l.pair==id)),"object_inactive":objects.map(|o|owner.graph.inactive_object_links(o).unwrap().iter().any(|l|l.pair==id)),"far":owner.far(id).map_or([None,None],|f|std::array::from_fn(|side|{let (token,key)=owner.hulls[objects[side]].entry(f.handles[side]).unwrap();Some(serde_json::json!({"handle":f.handles[side],"key":key.to_bits(),"owner_matches":token==id*2+side}))}))})
}
fn check_split(registered:bool){
 let data:serde_json::Value=serde_json::from_str(if registered{include_str!("../tools/fixtures/native-ivp-recursive-parent.json")}else{include_str!("../tools/fixtures/native-ivp-recursive-contacts.json")}).unwrap();
 // Retail geometry stays local; missing evidence must fail this validation.
 let phy=std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-bounds-1025-bound-cores/4.phy")).expect("restore the hash-verified local pallet .phy before running native recursive validation");
 let pallet=l4d2_formats::phy::parse_phy(&phy).unwrap();let hex=data["box"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);let boxes=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
 let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let mut calls=0;let mut total=0;
 for c in data["cases"].as_array().unwrap(){
    let mut solids=std::array::from_fn::<_,2,_>(|j|if c["flags"][j]==0{boxes[0].clone()}else{pallet[0].clone()});
    for j in 0..2{let t=Arc::make_mut(solids[j].compact_tree.as_mut().unwrap());let node=t.root;t.nodes[node].radius=f(&c["radii"][j]);}
    let physical=std::array::from_fn::<_,2,_>(|j|crate::ivp_joint_apply::tests::body(&c["before"][j]));let frame=w(&c["frame"]);
    let frames=std::array::from_fn(|j|{let b=physical[j];b.prediction.prepare_object_cache(b.clock,ObjectFrame{shift:(b.object_flags&0x800==0).then(||Vec3::from_array(std::array::from_fn(|k|f(&c["shifts"][j][k])))),rotation:None},frame)});
    let input=Input{bodies:std::array::from_fn(|j|Body{prediction:physical[j].prediction,prepared_pose:frames[j].pose,core_radius:f(&c["core_radii"][j]),margin:f(&c["object_margins"][j])}),at:physical[0].clock,margin:f(&c["margin"]),provided:[None;2],restricted:[None;2]};
    let own=Arc::new(AtomicI32::new(0));let mut descendants=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();descendants.listeners.push(own.clone());
    let mut parent=Entry{key:Pair{slots:[0,1],ledges:std::array::from_fn(|j|w(&c["ledges"][j]) as usize)},contact:Contact::new([0,1],crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS,input.bodies[0].margin+input.bodies[1].margin).unwrap(),recursive:Some(Box::new(Recursive{split:None,count:own,descendants}))};
    let scene=Scene{frames,input,root:Arc::new(AtomicI32::new(c["initial_count"].as_i64().unwrap() as i32)),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"]),margin:input.bodies[0].margin+input.bodies[1].margin,frame};
    staged_replay(&parent,&scene,c,false);
    let mut registered_owner=None;
    let result=if registered{
     let mut owner=ownership::Ownership::new(2,8,8,0.).unwrap();let id=owner.graph.register([0,1]).unwrap();owner.graph.activate(id,[false;2]).unwrap();parent.contact.flags=owner.graph.flags(id).unwrap();
     for j in 0..2{let initial=&c["hull_inputs"][j];owner.hulls[j].budget.predicted=f(&initial["predicted"]);let mut rows=initial["entries"].as_array().unwrap().clone();rows.sort_by_key(|r|w(&r["id"]));for r in rows{assert_eq!(owner.hulls[j].insert(1000+w(&r["id"]) as usize,f(&r["key"])).unwrap() as u32,w(&r["id"]));}assert_eq!(owner.hulls[j].budget.threshold.to_bits(),w(&initial["threshold"]));}
     let mut queued=c["queue_inputs"].as_array().unwrap().clone();queued.sort_by_key(|r|w(&r["id"]));for row in queued{assert_eq!(owner.queue.insert(1000+w(&row["id"]) as usize,f(&row["key"])).unwrap() as u32,w(&row["id"]));}
     if c["queued"].as_bool().unwrap(){owner.queue.insert(id,f(&c["queue_offset"])).unwrap();}
     assert_eq!(parent_packet(&owner,id),c["parent_before"]);
     let motion=std::array::from_fn(|side|[f(&c["hull_inputs"][side]["linear"]),f(&c["hull_inputs"][side]["angular"])]);
     if c["calls"]==1&&c["children"].as_u64().unwrap()>0{
      let before=format!("{owner:?}");let original_tree=tree(std::slice::from_ref(&parent));let original_root=scene.root.load(Ordering::Relaxed);
      let mut counters=CounterSnapshot::default();let mut rejected=parent.snapshot_with_counters(&mut counters);let rejected_root=counters.counter(&scene.root);let mut rejected_owner=owner.try_clone_unbound().unwrap();
      let rejected_scene=Scene{frames:scene.frames,input:scene.input,root:rejected_root.clone(),limit:scene.limit,fallback:scene.fallback,margin:scene.margin,frame:scene.frame};let mut reached=false;
      let result=rejected.expand_registered(&mut rejected_owner,id,input,&rejected_root,scene.limit,scene.fallback,physical.map(|b|b.object_flags),motion,0.,|owner,k,e,convexes|{assert_eq!(parent_packet(owner,id),c["parent_after"]);initialize(k,e,convexes,&rejected_scene)?;reached=true;Err("reject registered recursive child")},|_,k,e|retire(k,e));
      assert!(reached);assert_eq!(result.err(),Some("reject registered recursive child"));assert_eq!(format!("{owner:?}"),before);assert_eq!(tree(std::slice::from_ref(&parent)),original_tree);assert_eq!(scene.root.load(Ordering::Relaxed),original_root);
     }

     let out=parent.expand_registered(&mut owner,id,input,&scene.root,scene.limit,scene.fallback,physical.map(|b|b.object_flags),motion,0.,|owner,k,e,convexes|{
      assert_eq!(parent_packet(owner,id),c["parent_after"],"case{} parent transition precedes child",c["id"]);initialize(k,e,convexes,&scene)
     },|_,k,e|retire(k,e)).unwrap_or_else(|e|panic!("registered case{}: {e}",c["id"]));
     assert_eq!(parent_packet(&owner,id),c["parent_after"],"case{} final parent",c["id"]);registered_owner=Some((owner,id));out
    }else{parent.expand(input,&scene.root,scene.limit,scene.fallback,|k,e,c|initialize(k,e,c,&scene),retire).unwrap_or_else(|e|panic!("case{}: {e}",c["id"]))};
    let recursive=parent.recursive.as_ref().unwrap();assert_eq!(recursive.split.map_or(-1,|n|n as i32),c["split"].as_i64().unwrap() as i32,"case{} split",c["id"]);
    assert_eq!(result.is_some(),c["calls"]==1);
    if let Some(_)=result{let selected=recursive.split.unwrap();let mut provided=[-1;2];let mut restricted=[-1;2];provided[1-selected]=parent.key.ledges[1-selected] as i64;restricted[selected]=parent.key.ledges[selected] as i64;assert_eq!(serde_json::json!(provided),c["provided"]);assert_eq!(serde_json::json!(restricted),c["restricted"]);calls+=1;}
    assert_eq!(tree(recursive.descendants.entries()),c["tree"],"case{} hierarchy",c["id"]);assert_eq!(scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32,"case{} aggregate",c["id"]);total+=c["delta"].as_u64().unwrap();
    let mut path_forest=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();path_forest.entries.push(parent.snapshot_with_counters(&mut CounterSnapshot::default()));
    discovery::Discovery::assert_native_tree_paths(path_forest,&physical);
    let key=parent.key;retire(&key,&mut parent).unwrap();assert_eq!(scene.root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32,"case{} teardown",c["id"]);
    if let Some((mut owner,id))=registered_owner{owner.retire(id).unwrap();assert!(owner.graph.active().is_empty());assert!(owner.graph.inactive().is_empty());assert!(owner.far(id).is_none());for j in 0..2{assert_eq!(owner.hulls[j].ordered_keys().len(),c["hull_inputs"][j]["entries"].as_array().unwrap().len());}}

 }assert_eq!(calls,if registered{162}else{54});assert_eq!(total,if registered{792}else{264});
}

#[test]
fn feature_directed_recursive_contact_routes_and_children_match_native(){
 let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-failure.json")).unwrap();
 // Retail geometry stays local; missing evidence must fail this validation.
 let phy=std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-bounds-1025-bound-cores/4.phy")).expect("restore the hash-verified local pallet .phy before running native recursive validation");
 let pallet=l4d2_formats::phy::parse_phy(&phy).unwrap();let hex=data["box"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);let boxes=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
 let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let mut calls=0;let mut total=0;
 for c in data["cases"].as_array().unwrap(){
    let mut solids=std::array::from_fn::<_,2,_>(|j|if c["flags"][j]==0{boxes[0].clone()}else{pallet[0].clone()});
    for j in 0..2{let t=Arc::make_mut(solids[j].compact_tree.as_mut().unwrap());let node=t.root;t.nodes[node].radius=f(&c["radii"][j]);let key=w(&c["ledges"][j]) as usize;let cv=Arc::make_mut(t.coarse_convexes.get_mut(&key).unwrap());cv.triangle_headers[0].0=w(&c["input_triangles"][j]);let topology=Arc::make_mut(cv.topology.as_mut().unwrap());topology.edges[0][0]=l4d2_formats::phy::CompactEdge(w(&c["input_edges"][j]));}
    let physical=std::array::from_fn::<_,2,_>(|j|crate::ivp_joint_apply::tests::body(&c["before"][j]));let frame=w(&c["frame"]);
    let frames=std::array::from_fn(|j|{let b=physical[j];b.prediction.prepare_object_cache(b.clock,ObjectFrame{shift:(b.object_flags&0x800==0).then(||Vec3::from_array(std::array::from_fn(|k|f(&c["shifts"][j][k])))),rotation:None},frame)});
    let input=Input{bodies:std::array::from_fn(|j|Body{prediction:physical[j].prediction,prepared_pose:frames[j].pose,core_radius:f(&c["core_radii"][j]),margin:f(&c["object_margins"][j])}),at:physical[0].clock,margin:f(&c["margin"]),provided:[None;2],restricted:[None;2]};
    let own=Arc::new(AtomicI32::new(0));let mut descendants=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();descendants.listeners.push(own.clone());
    let mut parent=Entry{key:Pair{slots:[0,1],ledges:std::array::from_fn(|j|w(&c["ledges"][j]) as usize)},contact:Contact::new([0,1],crate::ivp_mindist_graph::COMPACT_PAIR_FLAGS,input.bodies[0].margin+input.bodies[1].margin).unwrap(),recursive:Some(Box::new(Recursive{split:None,count:own,descendants}))};
    let scene=Scene{frames,input,root:Arc::new(AtomicI32::new(c["initial_count"].as_i64().unwrap() as i32)),limit:c["limit"].as_i64().unwrap() as i32,fallback:f(&data["fallback_bits"]),margin:input.bodies[0].margin+input.bodies[1].margin,frame};
    parent.contact.query.kinds=std::array::from_fn(|j|c["input_kinds"][j].as_i64().unwrap() as i16);let physical_calls=std::cell::Cell::new(0);
    staged_replay(&parent,&scene,c,true);
    let result=parent.dispatch_contact(input,&scene.root,scene.limit,scene.fallback,|_|{physical_calls.set(physical_calls.get()+1);Ok(())},|k,e,c|initialize(k,e,c,&scene),retire).unwrap_or_else(|e|panic!("case{}: {e}",c["id"]));assert_eq!(physical_calls.get(),w(&c["physical_calls"]),"case{} physical handoff",c["id"]);
    let recursive=parent.recursive.as_ref().unwrap();assert_eq!(recursive.split.map_or(-1,|n|n as i32),c["split"].as_i64().unwrap() as i32,"case{} split",c["id"]);
    assert_eq!(matches!(result,ContactOutcome::Expanded(_)),c["calls"]==1);
    if let ContactOutcome::Expanded(_)=result{let selected=recursive.split.unwrap();let mut provided=[-1;2];let mut restricted=[-1;2];provided[1-selected]=parent.key.ledges[1-selected] as i64;restricted[selected]=parent.key.ledges[selected] as i64;assert_eq!(serde_json::json!(provided),c["provided"]);assert_eq!(serde_json::json!(restricted),c["restricted"]);calls+=1;}
    assert_eq!(tree(recursive.descendants.entries()),c["tree"],"case{} hierarchy",c["id"]);assert_eq!(scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32,"case{} aggregate",c["id"]);total+=c["delta"].as_u64().unwrap();
    let mut path_forest=Contacts::new([0,1],[&solids[0],&solids[1]]).unwrap();path_forest.entries.push(parent.snapshot_with_counters(&mut CounterSnapshot::default()));
    discovery::Discovery::assert_native_tree_paths(path_forest,&physical);
    let key=parent.key;retire(&key,&mut parent).unwrap();assert_eq!(scene.root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32,"case{} teardown",c["id"]);
 }assert_eq!(calls,528);assert_eq!(total,1320);
}
