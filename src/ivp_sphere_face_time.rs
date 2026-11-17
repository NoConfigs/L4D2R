//! Native10114660 sphere-centre/compact-face signed-distance timing.
use super::*;
use super::point_edge::normalized;
pub(crate) struct Input<'a>{pub features:super::Input<'a>,pub gap:f32}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{
 let b=i.features;if ![b.now,b.deadline].iter().all(|v|v.is_finite())||b.now>b.deadline||![b.bound,b.target,b.margin,b.distance,i.gap].iter().all(|v|v.is_finite())||b.bound<=0.||b.target<=0.||i.gap<0.{return Err("invalid sphere face timing inputs");}
 let h=b.hulls[1];let face=b.edges[1];let vertex=|id|h.vertex(id).map(Vec3::from_array).ok_or("missing sphere face vertex");let origin=vertex(face)?;let next=vertex(h.next(face).ok_or("missing sphere face next")?)?;let previous=vertex(h.previous(face).ok_or("missing sphere face previous")?)?;let normal=normalized((next-origin).cross(previous-origin));
 let mut poses=Poses::new(b.bodies);let mut searches=Vec::new();let candidate=approach_evaluator(b.now,b.deadline,b.target+b.margin,b.margin*0.5+i.gap,b.distance+b.margin,b.bound,0x113970,&mut searches,|k,t|point_face(poses.at(k,t),Vec3::ZERO,origin,normal));
 Ok(Outcome{kind:if candidate.is_some(){32}else{0},candidate,searches})
}
#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 #[test]
 fn sphere_face_signed_distance_time_matches_native_controls(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-face-time.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut counts=[0;2];let mut searches=0;
    for c in data["cases"].as_array().unwrap(){let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let reverse=w(&c["reverse"]) as usize;let order=[reverse,1-reverse];let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
        let bodies=std::array::from_fn(|j|{let i=order[j];let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])),"case{} cache origin{i}",c["id"]);assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])),"case{} cache rotation{i}",c["id"]);Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
        let input=super::super::Input{hulls:order.map(|i|&hulls[i]),edges:[CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};2],bodies,now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"])};
        let result=run(Input{features:input,gap:f(&c["settings"][0])}).unwrap();
        assert_eq!(result.kind,w(&c["kind"]),"case{} kind",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}counts[if result.kind==0{0}else{1}]+=1;
        assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])],"case{} search{searches}",c["id"]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;}
    }assert_eq!(counts,[366,786]);assert_eq!(searches,561);
 }
}
