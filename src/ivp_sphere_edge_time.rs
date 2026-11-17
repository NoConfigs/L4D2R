//! Native10116340 sphere-centre/compact-edge distance and adjacent-plane timing.
use super::*;
use super::point_edge::normalized;
pub(crate) struct Input<'a>{pub features:super::Input<'a>,pub total_bound:f32,pub gap:f32,pub face_epsilon:f32}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{
    let base=i.features;if ![base.now,base.deadline].iter().all(|v|v.is_finite())||base.now>base.deadline||![base.bound,base.target,base.margin,base.distance,i.total_bound,i.gap,i.face_epsilon].iter().all(|v|v.is_finite())||base.bound<=0.||base.target<=0.||i.total_bound<=0.||i.gap<0.||i.face_epsilon<0.{return Err("invalid point edge timing inputs");}
    let vertex=|side:usize,id:CompactEdgeId|base.hulls[side].vertex(id).map(Vec3::from_array).ok_or("missing point edge vertex");let point=Vec3::ZERO;let edge=base.edges[1];let h=base.hulls[1];let origin=vertex(1,edge)?;let next=h.next(edge).ok_or("missing point edge next")?;let line=normalized(vertex(1,next)?-origin);
    let cached=base.bodies.map(|b|b.cached);let delta=cached[1].transform_point(origin)-cached[0].transform_point(point);let perpendicular=normalized(inverse(cached[1].rotation,delta.cross(rotate(cached[1].rotation,line))));let bias=(base.target+base.margin)*0.5;
    let mut poses=Poses::new(base.bodies);let mut searches=Vec::new();let mut candidate=base.deadline;let mut kind=0;let initial=super::point_edge::line_distance(cached,point,origin,line,perpendicular,bias) as f32;
    if let Some(at)=approach_evaluator(base.now,base.deadline,base.target+base.margin,base.margin*0.9+i.gap,initial,base.bound,0x113e70,&mut searches,|k,t|super::point_edge::line_distance(poses.at(k,t),point,origin,line,perpendicular,bias)){candidate=at;kind=48;}
    for face in [edge,h.opposite(edge).ok_or("missing point edge opposite")?]{let a=vertex(1,face)?;let b=vertex(1,h.next(face).ok_or("missing adjacent next edge")?)?;let c=vertex(1,h.previous(face).ok_or("missing adjacent previous edge")?)?;let side=b-a;let normal=normalized(side.cross(side.cross(c-a)));let initial=point_face(cached,point,a,normal) as f32;
        if let Some(at)=descending(base.now,candidate,-i.face_epsilon,initial,i.total_bound.recip(),0,0x113970,&mut searches,|k,t|point_face(poses.at(k,t),point,a,normal)){candidate=at;kind=49;}
    }
    Ok(Outcome{kind,candidate:(kind!=0).then_some(candidate),searches})
}
#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 #[test]
 fn sphere_edge_distance_and_planes_match_native_controls(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-edge-time.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut counts=[0;3];let mut searches=0;
    for c in data["cases"].as_array().unwrap(){let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let reverse=w(&c["reverse"]) as usize;let order=[reverse,1-reverse];let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
        let bodies=std::array::from_fn(|j|{let i=order[j];let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])),"case{} cache origin{i}",c["id"]);assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])),"case{} cache rotation{i}",c["id"]);Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
        let input=super::super::Input{hulls:order.map(|i|&hulls[i]),edges:[CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};2],bodies,now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"])};
        let result=run(Input{features:input,total_bound:200.,gap:f(&c["settings"][0]),face_epsilon:f(&c["settings"][1])}).unwrap();
        assert_eq!(result.kind,w(&c["kind"]),"case{} kind",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}counts[if result.kind==0{0}else if result.kind==48{1}else{2}]+=1;
        assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])],"case{} search{searches}",c["id"]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;}
    }assert_eq!(counts,[232,116,804]);assert_eq!(searches,3381);
 }
}
