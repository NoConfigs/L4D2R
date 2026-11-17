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
#[test]
fn recursive_split_requests_child_hierarchies_and_counter_teardown_match_native(){
 let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-contacts.json")).unwrap();
 // Local research fixture (not in the repository); skipped when absent.
 let Ok(phy)=std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-bounds-1025-bound-cores/4.phy")) else {eprintln!("skipped: local pallet .phy fixture missing");return;};
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
    let result=parent.expand(input,&scene.root,scene.limit,scene.fallback,|k,e,c|initialize(k,e,c,&scene),retire).unwrap_or_else(|e|panic!("case{}: {e}",c["id"]));
    let recursive=parent.recursive.as_ref().unwrap();assert_eq!(recursive.split.map_or(-1,|n|n as i32),c["split"].as_i64().unwrap() as i32,"case{} split",c["id"]);
    assert_eq!(result.is_some(),c["calls"]==1);
    if let Some(_)=result{let selected=recursive.split.unwrap();let mut provided=[-1;2];let mut restricted=[-1;2];provided[1-selected]=parent.key.ledges[1-selected] as i64;restricted[selected]=parent.key.ledges[selected] as i64;assert_eq!(serde_json::json!(provided),c["provided"]);assert_eq!(serde_json::json!(restricted),c["restricted"]);calls+=1;}
    assert_eq!(tree(recursive.descendants.entries()),c["tree"],"case{} hierarchy",c["id"]);assert_eq!(scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32,"case{} aggregate",c["id"]);total+=c["delta"].as_u64().unwrap();
    let key=parent.key;retire(&key,&mut parent).unwrap();assert_eq!(scene.root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32,"case{} teardown",c["id"]);
 }assert_eq!(calls,54);assert_eq!(total,264);
}

#[test]
fn feature_directed_recursive_contact_routes_and_children_match_native(){
 let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-recursive-failure.json")).unwrap();
 // Local research fixture (not in the repository); skipped when absent.
 let Ok(phy)=std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-bounds-1025-bound-cores/4.phy")) else {eprintln!("skipped: local pallet .phy fixture missing");return;};
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
    let result=parent.dispatch_contact(input,&scene.root,scene.limit,scene.fallback,|_|{physical_calls.set(physical_calls.get()+1);Ok(())},|k,e,c|initialize(k,e,c,&scene),retire).unwrap_or_else(|e|panic!("case{}: {e}",c["id"]));assert_eq!(physical_calls.get(),w(&c["physical_calls"]),"case{} physical handoff",c["id"]);
    let recursive=parent.recursive.as_ref().unwrap();assert_eq!(recursive.split.map_or(-1,|n|n as i32),c["split"].as_i64().unwrap() as i32,"case{} split",c["id"]);
    assert_eq!(matches!(result,ContactOutcome::Expanded(_)),c["calls"]==1);
    if let ContactOutcome::Expanded(_)=result{let selected=recursive.split.unwrap();let mut provided=[-1;2];let mut restricted=[-1;2];provided[1-selected]=parent.key.ledges[1-selected] as i64;restricted[selected]=parent.key.ledges[selected] as i64;assert_eq!(serde_json::json!(provided),c["provided"]);assert_eq!(serde_json::json!(restricted),c["restricted"]);calls+=1;}
    assert_eq!(tree(recursive.descendants.entries()),c["tree"],"case{} hierarchy",c["id"]);assert_eq!(scene.root.load(Ordering::Relaxed),c["total"].as_i64().unwrap() as i32,"case{} aggregate",c["id"]);total+=c["delta"].as_u64().unwrap();
    let key=parent.key;retire(&key,&mut parent).unwrap();assert_eq!(scene.root.load(Ordering::Relaxed),c["after_delete_count"].as_i64().unwrap() as i32,"case{} teardown",c["id"]);
 }assert_eq!(calls,528);assert_eq!(total,1320);
}
