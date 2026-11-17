//!101147e0 edge distance, parallel-axis and four adjacent-face events.
use super::*;
pub(crate) struct Input<'a>{pub features:super::Input<'a>,pub normal:Vec3,pub gap:f32,pub face_epsilon:f32}
fn retained_dot(a:Vec3,b:Vec3)->f64{(a.y as f64*b.y as f64+a.x as f64*b.x as f64)+a.z as f64*b.z as f64}
fn distance(poses:[Pose;2],origins:[Vec3;2],lines:[Vec3;2],sign:f32)->f64{
    let a=poses[0].transform_point(origins[0]);let b=poses[1].transform_point(origins[1]);let cross=rotate(poses[0].rotation,lines[0]).cross(rotate(poses[1].rotation,lines[1]));
    let pa=retained_dot(a,cross) as f32;let pb=retained_dot(b,cross) as f32;let square=retained_dot(cross,cross) as f32;
    ((pa as f64-pb as f64)*(1./(square as f64).sqrt()))*sign as f64
}
fn parallel(poses:[Pose;2],lines:[Vec3;2])->f64{let c=rotate(poses[0].rotation,lines[0]).cross(rotate(poses[1].rotation,lines[1]));retained_dot(c,c)}
fn qualifier(poses:[Pose;2],other:usize,line:Vec3,normal:Vec3)->f64{retained_dot(rotate(poses[other].rotation,line),rotate(poses[1-other].rotation,normal))}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{
    let base=i.features;if ![base.now,base.deadline].iter().all(|v|v.is_finite())||base.now>base.deadline||![base.bound,base.target,i.gap,i.face_epsilon].iter().all(|v|v.is_finite())||base.bound<=0.||base.target<=0.||i.gap<0.||i.face_epsilon<0.||!i.normal.is_finite(){return Err("invalid edge timing inputs");}
    let vertex=|side:usize,id:CompactEdgeId|base.hulls[side].vertex(id).map(Vec3::from_array).ok_or("missing edge timing vertex");let mut origins=[Vec3::ZERO;2];let mut lines=[Vec3::ZERO;2];
    for side in 0..2{origins[side]=vertex(side,base.edges[side])?;let next=base.hulls[side].next(base.edges[side]).ok_or("missing edge timing next")?;lines[side]=super::point_edge::normalized(vertex(side,next)?-origins[side]);}
    let cached=base.bodies.map(|b|b.cached);let cross=rotate(cached[0].rotation,lines[0]).cross(rotate(cached[1].rotation,lines[1]));let positive=-dot(i.normal,cross)<=0.;let sign=if positive{1.}else{-1.};let angular=base.bodies[1].angular_speed+base.bodies[0].angular_speed;
    let mut poses=Poses::new(base.bodies);let mut candidate=base.deadline;let mut kind=0;let mut searches=Vec::new();let initial=distance(cached,origins,lines,sign) as f32;
    if let Some(at)=approach_evaluator(base.now,candidate,base.target,i.gap,initial,base.bound,0x114000,&mut searches,|k,t|distance(poses.at(k,t),origins,lines,sign)){candidate=at;kind=64;}
    let epsilon=f32::from_bits(786163455);let initial=parallel(cached,lines) as f32;
    if let Some(at)=descending(base.now,candidate,epsilon,initial,(angular*2.+epsilon).recip(),0,0x114100,&mut searches,|k,t|parallel(poses.at(k,t),lines)){candidate=at;kind=65;}
    for face_side in 0..2{let h=base.hulls[face_side];let edge=base.edges[face_side];let other=1-face_side;
        for (pass,face) in [h.opposite(edge).ok_or("missing edge timing opposite")?,edge].into_iter().enumerate(){let a=vertex(face_side,face)?;let next=h.next(face).ok_or("missing edge face next")?;let previous=h.previous(face).ok_or("missing edge face previous")?;let normal=super::point_edge::normalized((vertex(face_side,next)?-a).cross(vertex(face_side,previous)?-a));
            let forward=if pass==0{positive}else{!positive};let line=if forward{lines[other]}else{-lines[other]};let target=-(base.bodies[other].inverse_diameter*i.face_epsilon);let initial=qualifier(cached,other,line,normal) as f32;
            if let Some(at)=descending(base.now,candidate,target,initial,(angular+epsilon).recip(),0,0x113b20,&mut searches,|k,t|qualifier(poses.at(k,t),other,line,normal)){candidate=at;kind=66;}
        }
    }
    Ok(Outcome{kind,candidate:(kind!=0).then_some(candidate),searches})
}

#[cfg(test)]
mod tests{
 use super::*;
 use l4d2_formats::phy::CompactEdge;
 #[test]
 fn edge_edge_contact_parallel_and_adjacent_times_match_native_controls(){
    let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-edge-edge-time.json")).unwrap();let w=|v:&serde_json::Value|v.as_u64().unwrap() as u32;let f=|v:&serde_json::Value|f32::from_bits(w(v));let d=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());let mut counts=[0;4];let mut searches=0;
    assert_eq!(data["constants"],serde_json::json!([0.5f32.to_bits(),0.9f32.to_bits(),0.001f32.to_bits(),0.001f32.to_bits(),0.003f32.to_bits(),2f32.to_bits(),(-0.3f32).to_bits()]));
    for c in data["cases"].as_array().unwrap(){let hulls:Vec<_>=c["hulls"].as_array().unwrap().iter().map(|h|CompactTopology{edges:(0..4).map(|t|std::array::from_fn(|e|CompactEdge(w(&h[5+t*4+e])))).collect(),vertices_ivp:(0..4).map(|i|(i as u16,std::array::from_fn(|j|f(&h[20+i*4+j])))).collect()}).collect();let reverse=w(&c["reverse"]) as usize;let order=[reverse,1-reverse];let physical:Vec<_>=c["before"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
        let bodies=std::array::from_fn(|j|{let i=order[j];let b=physical[i];let frame=ObjectFrame{shift:(b.object_flags&0x800==0).then_some(Vec3::ZERO),rotation:None};let cache=b.prediction.prepare_object_cache(b.clock,frame,1);let n=&c["frames"][i];assert_eq!(cache.pose.position.to_array().map(f32::to_bits),std::array::from_fn::<_,3,_>(|k|w(&n[20+k])),"case{} cache origin{i}",c["id"]);assert_eq!(cache.pose.rotation.to_cols_array().map(f32::to_bits),std::array::from_fn::<_,9,_>(|k|w(&n[8+(k%3)*4+k/3])),"case{} cache rotation{i}",c["id"]);Body{prediction:b.prediction,frame,flags:b.object_flags,cached:cache.pose,angular_speed:f(&c["metadata"][i][1]),inverse_diameter:f(&c["metadata"][i][4])}});
        let input=super::super::Input{hulls:order.map(|i|&hulls[i]),edges:[CompactEdgeId{triangle:0,edge:w(&c["edge"]) as usize};2],bodies,now:d(&c["now"]),deadline:d(&c["deadline"]),bound:100.,margin:f(&c["margin"]),distance:f(&c["distance"]),target:f(&c["target"])};
        let result=run(Input{features:input,normal:Vec3::from_array(std::array::from_fn(|j|f(&c["normal"][j]))),gap:f(&c["settings"][0]),face_epsilon:f(&c["settings"][1])}).unwrap();
        assert_eq!(result.kind,w(&c["kind"]),"case{} kind",c["id"]);if result.kind!=0{assert_eq!(result.candidate.unwrap().to_bits(),c["candidate"].as_u64().unwrap(),"case{} candidate",c["id"]);}counts[if result.kind==0{0}else if result.kind==64{1}else if result.kind==65{2}else{3}]+=1;
        assert_eq!(result.searches.len(),c["searches"].as_array().unwrap().len());for (q,n) in result.searches.iter().zip(c["searches"].as_array().unwrap()){assert_eq!([q.evaluator,q.target.to_bits(),q.inverse_bound.to_bits(),q.index as u32],[w(&n["evaluator"]),w(&n["target"]),w(&n["inverse"]),w(&n["index"])],"case{} search{searches}",c["id"]);assert_eq!(q.start.to_bits(),n["start"].as_u64().unwrap());assert_eq!(q.deadline.to_bits(),n["deadline"].as_u64().unwrap());if !n["initial"].is_null(){assert_eq!(q.initial.to_bits(),w(&n["initial"]));}searches+=1;}
    }assert_eq!(counts,[144,116,96,796]);assert_eq!(searches,6788);
 }
}
