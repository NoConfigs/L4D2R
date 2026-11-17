//! Native10115370 sphere-centre/compact-point timing and compact neighbour fan.
use super::*;
pub(crate) struct Input<'a>{pub features:super::Input<'a>,pub normal:Vec3,pub total_bound:f32,pub linear_speeds:[f32;2],pub gap:f32}
fn separation(poses:[Pose;2],points:[Vec3;2],normal:Vec3)->f64{
    let delta=poses[1].transform_point(points[1])-poses[0].transform_point(points[0]);
    let square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;let projection=dot(delta,normal)*1.2f32;
    if square as f64<=(projection as f64).abs()*projection as f64{(square as f64).sqrt()}else{projection as f64}
}
fn neighbour(poses:[Pose;2],points:[Vec3;2],own:usize,unit:Vec3)->f64{
    let a=poses[1-own].transform_point(points[1-own]);let b=poses[own].transform_point(points[own]);let n=rotate(poses[own].rotation,unit);
    (((b.y as f64-a.y as f64)*n.y as f64)+((b.x as f64-a.x as f64)*n.x as f64))+((b.z as f64-a.z as f64)*n.z as f64)
}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{
 let base=i.features;if !i.normal.is_finite()||!i.total_bound.is_finite()||i.total_bound<0.||!i.gap.is_finite()||i.gap<0.||i.linear_speeds.iter().any(|v|!v.is_finite()||*v<0.)||![base.now,base.deadline].iter().all(|v|v.is_finite())||base.now>base.deadline||!base.margin.is_finite()||!base.distance.is_finite()||!base.bound.is_finite()||base.bound<=0.||!base.target.is_finite()||base.target<=0.||!base.bodies[1].inverse_diameter.is_finite()||base.bodies[1].inverse_diameter<=0.{return Err("invalid sphere point timing inputs");}
 let points=[Vec3::ZERO,Vec3::from_array(base.hulls[1].vertex(base.edges[1]).ok_or("missing compact vertex")?)];let mut poses=Poses::new(base.bodies);let cached=base.bodies.map(|b|b.cached);let normal=-i.normal;let initial=separation(cached,points,normal) as f32;let mut searches=Vec::new();let mut candidate=base.deadline;let mut kind=0;
 if let Some(at)=approach_evaluator(base.now,base.deadline,base.target+base.margin,base.margin*0.5+i.gap,initial,base.bound,0x113ba0,&mut searches,|k,t|separation(poses.at(k,t),points,normal)){candidate=at;kind=16;}
 let qualifier=((candidate-base.now)*i.total_bound as f64+base.distance as f64) as f32;let p=points[1];let length=((p.z as f64*p.z as f64+p.x as f64*p.x as f64)+p.y as f64*p.y as f64).sqrt();let angular=base.bodies[1].angular_speed;
 let bound=((length*angular as f64+i.linear_speeds[1] as f64) as f32+i.linear_speeds[0])+angular*qualifier;
 let threshold=-((base.target*base.target).min(qualifier*qualifier)*base.bodies[1].inverse_diameter);
 let h=base.hulls[1];let origin=points[1];let start=base.edges[1];let prev=h.previous(start).ok_or("missing compact previous edge")?;let mut edge=h.opposite(prev).ok_or("missing compact opposite edge")?;let mut seen=Vec::new();
 loop{if seen.contains(&edge){return Err("cyclic compact point fan");}seen.push(edge);let next=h.next(edge).ok_or("missing compact next edge")?;let delta=Vec3::from_array(h.vertex(next).ok_or("missing compact neighbour")?)-origin;let square=(delta.x*delta.x+delta.y*delta.y)+delta.z*delta.z;if square<=0.{return Err("degenerate compact neighbour");}let inverse=(1./(square as f64).sqrt()) as f32;let unit=delta*inverse;let target=(square*threshold)*inverse;let initial=neighbour(cached,points,1,unit) as f32;
 if let Some(at)=descending(base.now,candidate,target,initial,bound.recip(),0,0x113c90,&mut searches,|k,t|neighbour(poses.at(k,t),points,1,unit)){candidate=at;kind=17;}
 if edge==start{break;}edge=h.opposite(h.previous(edge).ok_or("missing compact fan previous edge")?).ok_or("missing compact fan opposite edge")?;
 }Ok(Outcome{kind,candidate:(kind!=0).then_some(candidate),searches})
}
#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 #[test]
 fn sphere_point_time_and_compact_neighbour_fan_match_native_controls(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-point-time.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut counts=[0;3];let mut searches=0;
    assert_eq!(data["constants"],serde_json::json!([(-1.0f32).to_bits(),0.5f32.to_bits()]));
    for c in data["cases"].as_array().unwrap(){let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let reverse=w(&c["reverse"]) as usize;let order=[reverse,1-reverse];let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
        let bodies=std::array::from_fn(|j|{let i=order[j];let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])),"case{} cache origin{i}",c["id"]);assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])),"case{} cache rotation{i}",c["id"]);Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
        let input=super::super::Input{hulls:order.map(|i|&hulls[i]),edges:[CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};2],bodies,now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"])};
        let result=run(Input{features:input,normal:Vec3::from_array(std::array::from_fn(|j|f(&c["normal"][j]))),total_bound:200.,gap:f(&c["settings"][0]),linear_speeds:order.map(|i|f(&c["metadata"][i][2]))}).unwrap();
        assert_eq!(result.kind,w(&c["kind"]),"case{} kind",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}counts[if result.kind==0{0}else if result.kind==16{1}else{2}]+=1;
        assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])],"case{} search{searches}",c["id"]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;}
    }assert_eq!(counts,[136,12,1004]);assert_eq!(searches,4608);
 }
}
