use super::*;
use l4d2_formats::phy::CompactEdge;
#[test]
fn typed_routes_follow_the_live_table_and_native_wrapper_role_invariants(){
 // Local research fixture (not in the repository); skipped when absent.
 let Ok(text)=std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),"/local-research/parity/ivp-feature-table-1109-native-provenance.json")) else {eprintln!("skipped: local ivp feature table fixture missing");return;};
 let table:serde_json::Value=serde_json::from_str(&text).unwrap();
 let kinds=[Kind::Point,Kind::Edge,Kind::Face,Kind::Sphere];let mut valid=0;
 for a in kinds{for b in kinds{for selected in 0..2{
    let physical=[a,b];let ordered=[physical[selected],physical[1-selected]];let wrapper=table["entries"][ordered[0] as usize*4+ordered[1] as usize].as_u64().unwrap() as u32;
    let result=route(physical,(selected as u32)<<8);
    // A mixed wrapper selected with the sphere in physical endpoint one would
    // read the wrong native object packet. Owners must canonicalize it first.
    let mixed_layout_invalid=ordered[0]==Kind::Sphere&&ordered[1]!=Kind::Sphere&&selected!=0;
    if wrapper==0x114190||mixed_layout_invalid{assert!(result.is_err(),"{physical:?} selected{selected}");}else{let r=result.unwrap();assert_eq!(r.wrapper,wrapper);valid+=1;}
 }}}assert_eq!(valid,13);
 assert!(route([Kind::Point;2],0x200).is_err());
}
#[test]
fn native_live_table_routes_compute_all_six_authored_timing_groups(){
 let sources=[
 (include_str!("../tools/fixtures/native-ivp-feature-route-point-pair.json"),[Kind::Point,Kind::Point],0x114dc0),
 (include_str!("../tools/fixtures/native-ivp-feature-route-point-edge.json"),[Kind::Point,Kind::Edge],0x115910),
 (include_str!("../tools/fixtures/native-ivp-feature-route-edge-edge.json"),[Kind::Edge,Kind::Edge],0x1147e0),
 (include_str!("../tools/fixtures/native-ivp-feature-route-sphere-point.json"),[Kind::Sphere,Kind::Point],0x115370),
 (include_str!("../tools/fixtures/native-ivp-feature-route-sphere-edge.json"),[Kind::Sphere,Kind::Edge],0x116340),
 (include_str!("../tools/fixtures/native-ivp-feature-route-sphere-face.json"),[Kind::Sphere,Kind::Face],0x114660)];
 let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut count=0;let mut searches=0;
 for (source,kinds,handler) in sources{let data:serde_json::Value=serde_json::from_str(source).unwrap();
 for c in data["cases"].as_array().unwrap(){
    let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();
    let reverse=w(&c["reverse"]) as usize;let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();let edge=CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};
    let bodies=std::array::from_fn::<_,2,_>(|i|{let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])));assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])));Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
    let mixed=kinds[0]==Kind::Sphere;
    let sides=std::array::from_fn(|j|{let i=if mixed{[reverse,1-reverse][j]}else{j};let kind=if mixed{kinds[j]}else{kinds[if j==reverse{0}else{1}]};let feature=match kind{Kind::Point=>Feature::Point(&hulls[i],edge),Kind::Edge=>Feature::Edge(&hulls[i],edge),Kind::Face=>Feature::Face(&hulls[i],edge),Kind::Sphere=>Feature::Sphere};Side{feature,body:bodies[i],radius:f(&c["metadata"][i][0]),linear_speed:f(&c["metadata"][i][2]),angular_radius:f(&c["metadata"][i][3])}});
    let result=run(Input{sides,flags:w(&c["input_flags"]),now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,total_bound:200.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"]),normal:Vec3::from_array(std::array::from_fn(|j|f(&c["normal"][j]))),gap:f(&c["settings"][0]),face_epsilon:f(&c["settings"][1])}).unwrap();
    assert_eq!(result.route.wrapper,w(&c["native_wrapper"]));assert_eq!(result.route.handler,handler);let result=result.outcome;
    assert_eq!(result.kind,w(&c["kind"]),"group{handler:x} case{}",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"group{handler:x} case{} time",c["id"]);}
    assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());
    for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){
      assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;
    }count+=1;
 }}assert_eq!(count,6912);assert_eq!(searches,30314);
}
