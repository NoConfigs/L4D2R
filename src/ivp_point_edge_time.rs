//!10115910 point/edge distance, adjacent-plane and point-neighbour timing.
use super::*;
pub(crate) struct Input<'a>{pub features:super::Input<'a>,pub total_bound:f32,pub linear_speeds:[f32;2],pub edge_angular_bound:f32,pub gap:f32,pub face_epsilon:f32}
///100ec450 and100ec4d0 retain the x87 reciprocal through all three
/// multiplications; only the final components spill to float.
pub(super) fn normalized(v:Vec3)->Vec3{
    let square=(v.x*v.x+v.y*v.y)+v.z*v.z;if square<f32::from_bits(786163455){return v;}
    let inv=1./(square as f64).sqrt();Vec3::new((v.x as f64*inv) as f32,(v.y as f64*inv) as f32,(v.z as f64*inv) as f32)
}

#[cfg(test)]
#[test]
fn normalization_retains_native_reciprocal_through_component_products(){
 let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-normalization.json")).unwrap();
 assert_eq!(data["threshold"],serde_json::json!(786163455u32));let mut distinguishes_early_rounding=0;
 for case in data["cases"].as_array().unwrap(){
  let input=Vec3::from_array(std::array::from_fn(|k|f32::from_bits(case["input"][k].as_u64().unwrap() as u32)));
  let expected=case["results"][0]["value"].clone();assert_eq!(case["results"][0],case["results"][1]);
  assert_eq!(serde_json::json!(normalized(input).to_array().map(f32::to_bits)),expected);
  let square=(input.x*input.x+input.y*input.y)+input.z*input.z;
  let early=if square<f32::from_bits(786163455){input}else{input*(1f32/square.sqrt())};
  distinguishes_early_rounding+=usize::from(serde_json::json!(early.to_array().map(f32::to_bits))!=expected);
 }
 assert_eq!(data["cases"].as_array().unwrap().len(),1024);assert_eq!(distinguishes_early_rounding,602);
}

pub(super) fn line_distance(poses:[Pose;2],point:Vec3,origin:Vec3,line:Vec3,perpendicular:Vec3,bias:f32)->f64{
    let delta=poses[1].transform_point(origin)-poses[0].transform_point(point);let e=rotate(poses[1].rotation,line);let p=rotate(poses[1].rotation,perpendicular);let c=delta.cross(e);
    let length=((c.z as f64*c.z as f64+c.x as f64*c.x as f64)+c.y as f64*c.y as f64).sqrt();let directed=dot(c,p)+bias;if length as f32<=directed{length}else{directed as f64}
}
fn toward_edge(poses:[Pose;2],point:Vec3,direction:Vec3,origin:Vec3,line:Vec3)->f64{
    let delta=poses[0].transform_point(point)-poses[1].transform_point(origin);let e=rotate(poses[1].rotation,line);let n=normalized(e.cross(delta).cross(e));let d=rotate(poses[0].rotation,direction);
    (d.y as f64*n.y as f64+d.x as f64*n.x as f64)+d.z as f64*n.z as f64
}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{
    let base=i.features;if ![base.now,base.deadline].iter().all(|v|v.is_finite())||base.now>base.deadline||![base.bound,base.target,base.margin,base.distance,i.total_bound,i.edge_angular_bound,i.gap,i.face_epsilon].iter().all(|v|v.is_finite())||base.bound<=0.||base.target<=0.||i.total_bound<=0.||i.gap<0.||i.face_epsilon<0.{return Err("invalid point edge timing inputs");}
    let vertex=|side:usize,id:CompactEdgeId|base.hulls[side].vertex(id).map(Vec3::from_array).ok_or("missing point edge vertex");let point=vertex(0,base.edges[0])?;let edge=base.edges[1];let h=base.hulls[1];let origin=vertex(1,edge)?;let next=h.next(edge).ok_or("missing point edge next")?;let line=normalized(vertex(1,next)?-origin);
    let cached=base.bodies.map(|b|b.cached);let delta=cached[1].transform_point(origin)-cached[0].transform_point(point);let perpendicular=normalized(inverse(cached[1].rotation,delta.cross(rotate(cached[1].rotation,line))));let bias=(base.target+base.margin)*0.5;
    let mut poses=Poses::new(base.bodies);let mut searches=Vec::new();let mut candidate=base.deadline;let mut kind=0;let initial=line_distance(cached,point,origin,line,perpendicular,bias) as f32;
    if let Some(at)=approach_evaluator(base.now,base.deadline,base.target+base.margin,base.margin*0.9+i.gap,initial,base.bound,0x113e70,&mut searches,|k,t|line_distance(poses.at(k,t),point,origin,line,perpendicular,bias)){candidate=at;kind=48;}
    for face in [edge,h.opposite(edge).ok_or("missing point edge opposite")?]{let a=vertex(1,face)?;let b=vertex(1,h.next(face).ok_or("missing adjacent next edge")?)?;let c=vertex(1,h.previous(face).ok_or("missing adjacent previous edge")?)?;let side=b-a;let normal=normalized(side.cross(side.cross(c-a)));let initial=point_face(cached,point,a,normal) as f32;
        if let Some(at)=descending(base.now,candidate,-i.face_epsilon,initial,i.total_bound.recip(),0,0x113970,&mut searches,|k,t|point_face(poses.at(k,t),point,a,normal)){candidate=at;kind=49;}
    }
    let distance=((base.distance as f64-(candidate-base.now)*i.total_bound as f64) as f32).max(f32::from_bits(994352038));let length=((point.z as f64*point.z as f64+point.x as f64*point.x as f64)+point.y as f64*point.y as f64).sqrt();let angular=base.bodies[0].angular_speed+base.bodies[1].angular_speed;
    let bound=(((i.edge_angular_bound as f64*base.bodies[1].angular_speed as f64+i.linear_speeds[1] as f64+length*base.bodies[0].angular_speed as f64)+i.linear_speeds[0] as f64)/distance as f64+angular as f64) as f32;
    let target=(base.bodies[1].inverse_diameter*2.)*(base.distance.min(base.target)*-0.3f32);let h=base.hulls[0];let start=base.edges[0];let mut edge=h.opposite(h.previous(start).ok_or("missing point fan previous")?).ok_or("missing point fan opposite")?;let mut visited=Vec::new();
    loop{if visited.contains(&edge){return Err("cyclic point edge fan");}visited.push(edge);let direction=normalized(vertex(0,h.next(edge).ok_or("missing point fan next")?)?-point);let initial=toward_edge(cached,point,direction,origin,line) as f32;
        if let Some(at)=descending(base.now,candidate,target,initial,bound.recip(),0,0x113d30,&mut searches,|k,t|toward_edge(poses.at(k,t),point,direction,origin,line)){candidate=at;kind=50;}
        if edge==start{break;}edge=h.opposite(h.previous(edge).ok_or("missing fan previous")?).ok_or("missing fan opposite")?;
    }Ok(Outcome{kind,candidate:(kind!=0).then_some(candidate),searches})
}

#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 #[test]
 fn point_edge_distance_planes_and_neighbour_times_match_native_controls(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-point-edge-time-settings.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut counts=[0;4];let mut searches=0;
    assert_eq!(data["constants"],serde_json::json!([0.5f32.to_bits(),0.9f32.to_bits(),0.001f32.to_bits(),0.001f32.to_bits(),0.003f32.to_bits(),2f32.to_bits(),(-0.3f32).to_bits()]));
    for c in data["cases"].as_array().unwrap(){let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let reverse=w(&c["reverse"]) as usize;let order=[reverse,1-reverse];let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
        let bodies=std::array::from_fn(|j|{let i=order[j];let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])),"case{} cache origin{i}",c["id"]);assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])),"case{} cache rotation{i}",c["id"]);Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
        let input=super::super::Input{hulls:order.map(|i|&hulls[i]),edges:[CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};2],bodies,now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"])};
        let result=run(Input{features:input,total_bound:200.,linear_speeds:order.map(|i|f(&c["metadata"][i][2])),edge_angular_bound:f(&c["metadata"][order[1]][3]),gap:f(&c["settings"][0]),face_epsilon:f(&c["settings"][1])}).unwrap();
        assert_eq!(result.kind,w(&c["kind"]),"case{} kind",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}counts[if result.kind==0{0}else if result.kind==48{1}else if result.kind==49{2}else{3}]+=1;
        assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])],"case{} search{searches}",c["id"]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;}
    }assert_eq!(counts,[148,16,40,948]);assert_eq!(searches,6912);
 }
}
