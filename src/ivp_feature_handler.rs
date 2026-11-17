//! Native101141b0 full vertex/face feature-time handler. This owns the fan walk,
//! approach/escape selection and shared quantized pose caches. World membership
//! and the other feature classes remain separate.
use bevy::prelude::*;
use crate::ivp_motion::{Prediction,ObjectFrame,Pose};
use l4d2_formats::phy::{CompactTopology,CompactEdgeId};
#[path="ivp_feature_dispatch.rs"]
pub(crate) mod dispatch;
#[path="ivp_point_pair_time.rs"]
pub(crate) mod point_pair;
#[path="ivp_point_edge_time.rs"]
pub(crate) mod point_edge;
#[path="ivp_sphere_edge_time.rs"]
pub(crate) mod sphere_edge;
#[path="ivp_sphere_face_time.rs"]
pub(crate) mod sphere_face;
#[path="ivp_sphere_point_time.rs"]
pub(crate) mod sphere_point;
#[path="ivp_edge_edge_time.rs"]
pub(crate) mod edge_edge;
#[derive(Clone,Copy)]
pub(crate) struct Body{pub prediction:Prediction,pub frame:ObjectFrame,pub flags:u32,pub cached:Pose,pub angular_speed:f32,pub inverse_diameter:f32}
struct Poses{bodies:[Body;2],cache:[Vec<Option<Pose>>;2]}
impl Poses{
 fn new(bodies:[Body;2])->Self{let mut cache:[Vec<Option<Pose>>;2]=std::array::from_fn(|_|vec![None;21]);for i in 0..2{cache[i][0]=Some(bodies[i].cached);}Self{bodies,cache}}
 fn at(&mut self,index:Option<usize>,at:f64)->[Pose;2]{std::array::from_fn(|i|{let b=self.bodies[i];if (b.flags as u8 as i8)>7{return b.cached;}
    if let Some(k)=index{if let Some(p)=self.cache[i][k]{return p;}let p=b.prediction.sample(at,b.frame);self.cache[i][k]=Some(p);p}else{b.prediction.sample(at,b.frame)}
 })}
}
fn dot(a:Vec3,b:Vec3)->f32{(a.y*b.y+a.x*b.x)+a.z*b.z}
fn rotate(m:Mat3,v:Vec3)->Vec3{let r=m.transpose();Vec3::new(dot(r.x_axis,v),dot(r.y_axis,v),dot(r.z_axis,v))}
fn inverse(m:Mat3,v:Vec3)->Vec3{Vec3::new(dot(m.x_axis,v),dot(m.y_axis,v),dot(m.z_axis,v))}
fn point_face(poses:[Pose;2],point:Vec3,origin:Vec3,normal:Vec3)->f64{let p=poses[0].transform_point(point);let o=poses[1].transform_point(origin);let n=rotate(poses[1].rotation,normal);((p.x as f64-o.x as f64)*n.x as f64+(p.y-o.y) as f64*n.y as f64)+(p.z-o.z) as f64*n.z as f64}
fn direction(poses:[Pose;2],edge:Vec3,normal:Vec3)->f64{let n=inverse(poses[0].rotation,rotate(poses[1].rotation,normal));(edge.y as f64*n.y as f64+edge.x as f64*n.x as f64)+edge.z as f64*n.z as f64}
#[derive(Debug)]
pub(crate) struct Search{pub evaluator:u32,pub target:f32,pub start:f64,pub deadline:f64,pub index:usize,pub initial:f32,pub inverse_bound:f32}
pub(crate) struct Input<'a>{pub hulls:[&'a CompactTopology;2],pub edges:[CompactEdgeId;2],pub bodies:[Body;2],pub now:f64,pub deadline:f64,pub bound:f32,pub margin:f32,pub distance:f32,pub target:f32}
/// Bit-preserving observation of source inputs. No expected/native values are
/// read by the running source-world path.
pub(crate) fn describe(i:&Input<'_>)->serde_json::Value{
 let v=|p:Vec3|p.to_array().map(f32::to_bits);
 serde_json::json!({"now_bits":i.now.to_bits(),"deadline_bits":i.deadline.to_bits(),"bound_bits":i.bound.to_bits(),"margin_bits":i.margin.to_bits(),"distance_bits":i.distance.to_bits(),"target_bits":i.target.to_bits(),
  "edges":i.edges.map(|e|[e.triangle,e.edge]),"bodies":i.bodies.map(|b|serde_json::json!({"flags":b.flags,"angular_speed_bits":b.angular_speed.to_bits(),"inverse_diameter_bits":b.inverse_diameter.to_bits(),
   "prediction":{"time_bits":b.prediction.time.to_bits(),"inverse_dt_bits":b.prediction.inverse_dt.to_bits(),"start":v(b.prediction.start),"velocity":v(b.prediction.velocity),"rotation_start":b.prediction.rotation_start.map(f32::to_bits),"rotation_end":b.prediction.rotation_end.map(f32::to_bits)},
   "frame":{"shift":b.frame.shift.map(v),"rotation":b.frame.rotation.map(|q|q.map(f32::to_bits))},"cached":{"position":v(b.cached.position),"rotation":b.cached.rotation.to_cols_array().map(f32::to_bits)}}))})
}
pub(crate) struct Outcome{pub kind:u32,pub candidate:Option<f64>,pub searches:Vec<Search>}
fn descending(start:f64,deadline:f64,target:f32,initial:f32,inverse_bound:f32,index:usize,evaluator:u32,trace:&mut Vec<Search>,evaluate:impl FnMut(Option<usize>,f64)->f64)->Option<f64>{
 trace.push(Search{evaluator,target,start,deadline,index,initial,inverse_bound});crate::ivp_time::search_indexed(start,deadline,target,initial,inverse_bound,index,evaluate).map(|v|v.refined.time)
}
///10127cc0: an already-close pair first advances away from its escape plane.
/// Its quantized index and cached poses survive a transition to descending search.
fn approach(start:f64,deadline:f64,target:f32,escape:f32,initial:f32,bound:f32,trace:&mut Vec<Search>,evaluate:impl FnMut(Option<usize>,f64)->f64)->Option<f64>{
 approach_evaluator(start,deadline,target,escape,initial,bound,0x113970,trace,evaluate)
}
fn approach_evaluator(start:f64,deadline:f64,target:f32,escape:f32,initial:f32,bound:f32,evaluator:u32,trace:&mut Vec<Search>,mut evaluate:impl FnMut(Option<usize>,f64)->f64)->Option<f64>{
 let inverse=bound.recip();if target<initial{return descending(start,deadline,target,initial,inverse,0,evaluator,trace,evaluate);}
 let mut low=start;let mut distance=initial;let mut index=0;
 while low<deadline{
    let delta=(distance as f64-escape as f64)*inverse as f64;
    let duration=if (low-deadline)+delta<=0.{(delta as f32).max(0.)}else{((deadline-low)+0.002999999992425728) as f32};
    let steps=((duration*200.) as i32).max(1) as usize;index+=steps;if index>20{return None;}
    let high=steps as f64*0.004999999888241291+low;let value=evaluate(Some(index),high);if !value.is_finite(){return None;}
    if (target as f64)<value{if high>deadline{return None;}return descending(high,deadline,target,value as f32,inverse,index,evaluator,trace,evaluate);}
    if value as f32<=initial{return Some(low);}
    low=high;distance=value as f32;
 }None
}
pub(crate) fn run(i:Input<'_>)->Result<Outcome,&'static str>{run_with_settings(i,0.000634745986,0.000634745986)}
pub(crate) fn run_with_settings(i:Input<'_>,gap:f32,face_epsilon:f32)->Result<Outcome,&'static str>{
 if !gap.is_finite()||gap<0.||!face_epsilon.is_finite()||face_epsilon<0.{return Err("invalid vertex face timing settings");}
 if !i.bodies[1].inverse_diameter.is_finite()||i.bodies[1].inverse_diameter<0.{return Err("invalid directional threshold");}
 if ![i.now,i.deadline].iter().all(|v|v.is_finite())||i.now>i.deadline||![i.bound,i.margin,i.distance,i.target].iter().all(|v|v.is_finite())||i.bound<=0.||i.target<=0.{return Err("invalid feature handler input");}
 let vertex=|h:&CompactTopology,id:CompactEdgeId|h.vertex(id).map(Vec3::from_array).ok_or("missing feature vertex");
 let point=vertex(i.hulls[0],i.edges[0])?;let face=i.edges[1];let origin=vertex(i.hulls[1],face)?;
 let next=i.hulls[1].next(face).ok_or("missing face next edge")?;let previous=i.hulls[1].previous(face).ok_or("missing face previous edge")?;
 let cross=(vertex(i.hulls[1],next)?-origin).cross(vertex(i.hulls[1],previous)?-origin);let square=(cross.x*cross.x+cross.y*cross.y)+cross.z*cross.z;
 if square<1e-10{return Err("degenerate feature plane");}let inv=1./(square as f64).sqrt();let normal=Vec3::new((cross.x as f64*inv) as f32,(cross.y as f64*inv) as f32,(cross.z as f64*inv) as f32);
 let mut poses=Poses::new(i.bodies);let mut searches=Vec::new();let mut candidate=i.deadline;let mut kind=0;
 if let Some(at)=approach(i.now,i.deadline,i.target+i.margin,i.margin*0.5+gap,i.distance+i.margin,i.bound,&mut searches,|k,t|point_face(poses.at(k,t),point,origin,normal)){candidate=at;kind=32;}
 let cached=[i.bodies[0].cached,i.bodies[1].cached];let local_normal=inverse(cached[0].rotation,rotate(cached[1].rotation,normal));
 let angular=(i.bodies[1].angular_speed+i.bodies[0].angular_speed)+1e-10f32;
 let qualifier=((candidate-i.now)*angular as f64) as f32;
 let threshold=(-(i.bodies[1].inverse_diameter*face_epsilon)*(i.margin*0.5+i.distance.min(i.target)))/i.target;
 let previous=i.hulls[0].previous(i.edges[0]).ok_or("missing vertex previous edge")?;
 let mut edge=i.hulls[0].opposite(previous).ok_or("missing vertex opposite edge")?;let mut visited=Vec::new();
 loop{
    if visited.contains(&edge){return Err("cyclic feature neighbour fan");}visited.push(edge);
    let next=i.hulls[0].next(edge).ok_or("missing fan next edge")?;let delta=vertex(i.hulls[0],next)?-point;
    let square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;if square<=0.{return Err("degenerate feature neighbour");}
    let inverse=(1./(square as f64).sqrt()) as f32;let initial=inverse*dot(delta,local_normal);
    if initial<qualifier{let unit=delta*inverse;
        if let Some(at)=descending(i.now,candidate,threshold,initial,angular.recip(),0,0x113f80,&mut searches,|k,t|direction(poses.at(k,t),unit,normal)){candidate=at;kind=33;}
    }
    if edge==i.edges[0]{break;}edge=i.hulls[0].opposite(i.hulls[0].previous(edge).ok_or("missing fan previous edge")?).ok_or("missing fan opposite edge")?;
 }
 Ok(Outcome{kind,candidate:(kind!=0).then_some(candidate),searches})
}
#[cfg(test)]
mod tests{
 #[test]
 fn production_vertex_face_driver_computes_candidates_and_owns_queue_changes(){
    use super::*;
    use crate::{ivp_distance_schedule as schedule,ivp_event_queue::Queue};
    let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");assert!(root.exists(),"installed PHY inputs required for this parity control");let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
    let timing:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-feature-handler.json")).unwrap();let schedules:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-distance-schedule.json")).unwrap();
    let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;let v3=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));let q4=|v:&serde_json::Value|std::array::from_fn(|i|f(&v[i]));
    let prediction=|p:&serde_json::Value|Prediction{start:v3(&p["start"]),velocity:v3(&p["velocity"]),rotation_start:q4(&p["rotation_start"]),rotation_end:q4(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])};
    let frame=|p:&serde_json::Value|ObjectFrame{shift:(p["flags"].as_u64().unwrap()&0x800==0).then(||v3(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||q4(&p["object_rotation"]))};
    let mut count=0;let mut derived=0;let mut queued=0;let mut far=0;
    for c in schedules["cases"].as_array().unwrap(){let source=timing["cases"].as_array().unwrap().iter().find(|h|h["mode"]==c["mode"]&&h["model"]==c["model"]).unwrap();
        let moving=l4d2_formats::phy::parse_phy(&assets.read(c["model"].as_str().unwrap()).unwrap()).unwrap();let hex=source["floor"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);let floor=l4d2_formats::phy::parse_collision(&packet,1).unwrap();let solids=[&moving[0],&floor[0]];
        for s in c["schedules"].as_array().unwrap(){let mut input=schedule::captured_input(s);let config=schedule::Config::default();input.target=config.target(input.flags).unwrap();input.slow_gate=config.slow_gate;
            let decision=schedule::prepare(&input).unwrap();let h=if let schedule::Action::Feature{closing_bound,..}=decision.action{derived+=1;source["calls"].as_array().unwrap().iter().find(|h|h["now"].as_f64().unwrap().to_bits()==input.now.to_bits()&&h["deadline"].as_f64().unwrap().to_bits()==input.deadline.to_bits()&&h["flags"].as_u64().unwrap() as u32==decision.flags&&f(&h["distance"]).to_bits()==input.distance.to_bits()&&f(&h["bound"]).to_bits()==closing_bound.to_bits()).expect("matching independent feature-input packet") }else{&source["calls"][0]};
            let edges=std::array::from_fn(|i|CompactEdgeId{triangle:h["sides"][i]["triangle"].as_u64().unwrap() as usize,edge:h["sides"][i]["edge"].as_u64().unwrap() as usize});let hulls=std::array::from_fn(|i|solids[i].convexes[h["sides"][i]["convex"].as_u64().unwrap() as usize].topology.as_ref().unwrap().as_ref());let bodies=std::array::from_fn(|i|{let b=&h["sides"][i];let p=&b["current"];let old=&b["cached"];Body{prediction:prediction(p),frame:frame(p),flags:p["flags"].as_u64().unwrap() as u32,cached:prediction(old).prepare_object_cache(old["now"].as_f64().unwrap(),frame(old),old["frame"].as_u64().unwrap() as u32).pose,angular_speed:f(&b["angular_speed"]),inverse_diameter:f(&b["inverse_diameter"])}});
            let geometry=Input{hulls,edges,bodies,now:0.,deadline:0.,bound:0.,margin:f(&h["margin"]),distance:0.,target:0.};let epoch=s["queue_before"]["epoch"].as_f64().unwrap();assert_eq!(epoch.to_bits(),s["queue_after"]["epoch"].as_f64().unwrap().to_bits());let mut queue=Queue::new(16,epoch).unwrap();if s["queue_before"]["queued"]==true{queue.insert(0,f(&s["queue_before"]["key"])).unwrap();}
            let geometry=dispatch::Input{sides:std::array::from_fn(|j|dispatch::Side{feature:if j==0{dispatch::Feature::Point(geometry.hulls[j],geometry.edges[j])}else{dispatch::Feature::Face(geometry.hulls[j],geometry.edges[j])},body:geometry.bodies[j],radius:1.,linear_speed:0.,angular_radius:0.}),flags:0,now:geometry.now,deadline:geometry.deadline,bound:geometry.bound,total_bound:0.,margin:geometry.margin,distance:geometry.distance,target:geometry.target,normal:Vec3::ZERO,gap:0.000634745986,face_epsilon:0.000634745986};
            let result=schedule::queue::feature(&mut queue,0,input,s["delay"].as_u64().unwrap() as u8,config,geometry).unwrap();
            assert_eq!(result.kind,s["feature_kind"].as_u64().unwrap() as u32,"case{count} kind");assert_eq!(result.candidate.map(f64::to_bits),s["feature_candidate"].as_f64().map(f64::to_bits),"case{count} time");assert_eq!(result.flags,s["after"]["flags"].as_u64().unwrap() as u32);assert_eq!(result.counter,s["counter_after"].as_u64().unwrap() as u32);
            assert_eq!(queue.scheduled(0).map(|(_,key)|key.to_bits()),s["queue_after"]["key"].as_f64().map(|key|(key as f32).to_bits()),"case{count} queue key");assert_eq!(result.handle.is_some(),queue.scheduled(0).is_some());queued+=usize::from(result.handle.is_some());
            if let Some((keys,linear))=result.far{assert_eq!(keys.map(f32::to_bits),std::array::from_fn::<_,2,_>(|i|f(&s["hull_keys"][i]).to_bits()));assert_eq!(linear.to_bits(),f(&s["after"]["measures"][1]).to_bits());far+=1;}else{assert!(s["hull_keys"].is_null());}count+=1;
        }
    }assert_eq!((count,derived,queued,far),(278,214,153,24));
 }
 use super::*;
 #[test]
 fn complete_native_point_face_handlers_compute_their_own_candidate_times(){check(false);}
 #[test]
 fn recovered_feature_candidates_feed_the_native_scheduling_gates_without_recorded_times(){check(true);}
 fn check(compose:bool){
    let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");if !root.exists(){return;}let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
    let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-feature-handler.json")).unwrap();
    let f=|v:&serde_json::Value|v.as_f64().unwrap() as f32;let v3=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|k|f(&v[k])));let q4=|v:&serde_json::Value|std::array::from_fn(|k|f(&v[k]));
    let prediction=|p:&serde_json::Value|Prediction{start:v3(&p["start"]),velocity:v3(&p["velocity"]),rotation_start:q4(&p["rotation_start"]),rotation_end:q4(&p["rotation_end"]),time:p["time"].as_f64().unwrap(),inverse_dt:f(&p["inverse_dt"])};
    let frame=|p:&serde_json::Value|ObjectFrame{shift:(p["flags"].as_u64().unwrap()&0x800==0).then(||v3(&p["shift"])),rotation:p["extra"].as_bool().unwrap().then(||q4(&p["object_rotation"]))};
    let mut count=0;let mut max_time=0f64;let mut outcomes=std::collections::BTreeMap::new();let mut input_audit=vec![];
    for c in d["cases"].as_array().unwrap(){
        let body=l4d2_formats::phy::parse_phy(&assets.read(c["model"].as_str().unwrap()).unwrap()).unwrap();let hex=c["floor"].as_str().unwrap();let raw=(0..hex.len()).step_by(2).map(|k|u8::from_str_radix(&hex[k..k+2],16).unwrap()).collect::<Vec<_>>();let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);let floor=l4d2_formats::phy::parse_collision(&packet,1).unwrap();let solids=[&body[0],&floor[0]];
        for h in c["calls"].as_array().unwrap(){let edges=std::array::from_fn(|k|CompactEdgeId{triangle:h["sides"][k]["triangle"].as_u64().unwrap() as usize,edge:h["sides"][k]["edge"].as_u64().unwrap() as usize});
            let hulls=std::array::from_fn(|k|solids[k].convexes[h["sides"][k]["convex"].as_u64().unwrap() as usize].topology.as_ref().unwrap().as_ref());
            let bodies=std::array::from_fn(|k|{let b=&h["sides"][k];let p=&b["current"];let old=&b["cached"];Body{prediction:prediction(p),frame:frame(p),flags:p["flags"].as_u64().unwrap() as u32,cached:prediction(old).prepare_object_cache(old["now"].as_f64().unwrap(),frame(old),old["frame"].as_u64().unwrap() as u32).pose,angular_speed:f(&b["angular_speed"]),inverse_diameter:f(&b["inverse_diameter"])}});
            let input=Input{hulls,edges,bodies,now:h["now"].as_f64().unwrap(),deadline:h["deadline"].as_f64().unwrap(),bound:f(&h["bound"]),margin:f(&h["margin"]),distance:f(&h["distance"]),target:f(&h["target"])};
            if std::env::var_os("L4D2R_PHY_FEATURE_INPUT_REPORT").is_some(){input_audit.push(serde_json::json!({"mode":c["mode"],"model":c["model"],"step":h["step"],"input":describe(&input)}));}
            let routed=dispatch::run(dispatch::Input{sides:std::array::from_fn(|j|dispatch::Side{feature:if j==0{dispatch::Feature::Point(input.hulls[j],input.edges[j])}else{dispatch::Feature::Face(input.hulls[j],input.edges[j])},body:input.bodies[j],radius:1.,linear_speed:0.,angular_radius:0.}),flags:h["flags"].as_u64().unwrap() as u32,now:input.now,deadline:input.deadline,bound:input.bound,total_bound:0.,margin:input.margin,distance:input.distance,target:input.target,normal:Vec3::ZERO,gap:0.000634745986,face_epsilon:0.000634745986}).unwrap();assert_eq!(routed.route.handler,0x1141b0);let result=routed.outcome;
            assert_eq!(result.kind,h["kind"].as_u64().unwrap() as u32,"{} {} step{} call{count}",c["mode"],c["model"],h["step"]);
            if let Some(at)=result.candidate{let error=(at-h["candidate"].as_f64().unwrap()).abs();max_time=max_time.max(error);assert_eq!(at.to_bits(),h["candidate"].as_f64().unwrap().to_bits(),"candidate bits{count}");}
            assert_eq!(result.searches.len(),h["searches"].as_array().unwrap().len(),"search count{count}");
            for (q,n)in result.searches.iter().zip(h["searches"].as_array().unwrap()){
                assert_eq!(q.evaluator,n["evaluator_rva"].as_u64().unwrap() as u32,"evaluator {count}");assert_eq!(q.target.to_bits(),f(&n["target"]).to_bits(),"target {count}");assert_eq!(q.initial.to_bits(),f(&n["initial"]).to_bits(),"initial {count}");assert_eq!(q.index,n["index"].as_u64().unwrap() as usize,"index {count}");assert_eq!(q.start.to_bits(),n["start"].as_f64().unwrap().to_bits(),"search start{count}");assert_eq!(q.deadline.to_bits(),n["deadline"].as_f64().unwrap().to_bits(),"search deadline{count}");
            }
            let key=(c["mode"].as_str().unwrap().to_owned(),c["model"].as_str().unwrap().to_owned(),h["now"].as_f64().unwrap().to_bits(),h["deadline"].as_f64().unwrap().to_bits(),h["flags"].as_u64().unwrap() as u32,f(&h["distance"]).to_bits(),f(&h["bound"]).to_bits());
            let value=(result.kind,result.candidate);if let Some(previous)=outcomes.insert(key,value){assert_eq!(previous,value,"same local inputs changed result");}count+=1;
        }
    }println!("{count} complete handlers: max time error{max_time}");assert_eq!(count,379);
    if let Some(path)=std::env::var_os("L4D2R_PHY_FEATURE_INPUT_REPORT"){std::fs::write(path,serde_json::to_vec_pretty(&input_audit).unwrap()).unwrap();}
    if compose{
        use crate::ivp_distance_schedule as schedule;
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-distance-schedule.json")).unwrap();let mut calls=0;let mut derived=0;let mut queued=0;
        for c in fixture["cases"].as_array().unwrap(){for packet in c["schedules"].as_array().unwrap(){
            let input=schedule::captured_input(packet);let mut decision=schedule::prepare(&input).unwrap();
            let (kind,candidate)=if let schedule::Action::Feature{closing_bound,..}=decision.action{
                let key=(c["mode"].as_str().unwrap().to_owned(),c["model"].as_str().unwrap().to_owned(),input.now.to_bits(),input.deadline.to_bits(),decision.flags,input.distance.to_bits(),closing_bound.to_bits());
                derived+=1;*outcomes.get(&key).expect("source scheduling inputs have no independently captured handler input packet")
            }else{(0,None)};
            // Native feature_candidate/feature_kind fields are comparison data
            // in the prior scheduler test. This composition never reads them.
            let at=schedule::complete(&input,&mut decision,kind,candidate,packet["delay"].as_u64().unwrap() as u8,f(&packet["time_scale"]),packet["queue_after"]["epoch"].as_f64().unwrap()).unwrap();
            assert_eq!(at.map(f32::to_bits),packet["queue_after"]["key"].as_f64().map(|v|(v as f32).to_bits()),"composed queue{calls}");
            assert_eq!(decision.flags,packet["after"]["flags"].as_u64().unwrap() as u32,"composed flags{calls}");assert_eq!(decision.counter,packet["counter_after"].as_u64().unwrap() as u32);queued+=usize::from(at.is_some());calls+=1;
        }}assert_eq!(calls,278);assert_eq!(derived,214);assert_eq!(queued,153);
    }
 }
}
