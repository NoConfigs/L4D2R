//! Owned 10127910 bracket refinement. Broadphase, feature transitions and
//! conservative bracket discovery are separate scheduler responsibilities.
use bevy::prelude::{Mat3,Vec3};
/// Owned 10113f80 directional feature-change predicate. Rotate the plane
/// normal into the edge object's frame before the retained Y/X/Z dot.
pub(crate) fn direction_distance(a:Mat3,b:Mat3,edge:Vec3,normal:Vec3)->f64 {
    let local=a.transpose()*(b*normal);
    (edge.y as f64*local.y as f64+edge.x as f64*local.x as f64)+edge.z as f64*local.z as f64
}
/// Owned 10113970 point-versus-plane evaluator, including its mixed
/// float/retained subtraction precision. Geometry/feature selection is separate.
pub(crate) fn point_face_distance(a:crate::ivp_motion::Pose,b:crate::ivp_motion::Pose,vertex:Vec3,origin:Vec3,normal:Vec3)->f64 {
    let point=a.transform_point(vertex);let origin=b.transform_point(origin);let n=b.rotation*normal;
    ((point.x as f64-origin.x as f64)*n.x as f64+(point.y-origin.y) as f64*n.y as f64)+(point.z-origin.z) as f64*n.z as f64
}
///10113ba0, selected by the initialized sphere/sphere feature handler.
/// The initial contact direction supplies a signed projection. Keep its SSE
/// float spill and the subsequent retained-precision comparison; replacing
/// this evaluator with unsigned centre distance changes event scheduling.
pub(crate) fn sphere_distance(a:crate::ivp_motion::Pose,b:crate::ivp_motion::Pose,points:[Vec3;2],normal:Vec3)->f64 {
    let delta=b.transform_point(points[1])-a.transform_point(points[0]);
    let square=(delta.y*delta.y+delta.x*delta.x)+delta.z*delta.z;
    let projection=((normal.y*delta.y+normal.x*delta.x)+normal.z*delta.z)*f32::from_bits(0x3f99999a);
    let signed=projection as f64;
    if square as f64<=signed.abs()*signed {(square as f64).sqrt()}else{signed}
}
///101141b0's directional feature threshold; the caller owns cached mindist
/// geometry and the triangle object's inverse diameter.
pub(crate) fn feature_target(margin:f32,distance:f32,target:f32,inverse_diameter:f32)->Option<f32> {
    if ![margin,distance,target,inverse_diameter].iter().all(|v|v.is_finite()) || target<=0. || inverse_diameter<0. {return None;}
    Some((-(inverse_diameter*0.000634745986)*(margin*0.5+distance.min(target)))/target)
}
/// Native event insertion stores a float offset from the queue epoch.
pub(crate) fn queue_time(at:f64,epoch:f64)->f64 {epoch+(at-epoch) as f32 as f64}
///1010b840/1010c450 queue admission. Nearly coincident candidates use the
/// cached pair separation and the full motion bound, rather than directional
/// closing speed. The environment supplies PSI dt, delay mode and time scale.
pub(crate) struct EventAdmission {
    pub now:f64, pub deadline:f64, pub candidate:f64, pub epoch:f64,
    pub dt:f32, pub distance:f32, pub total_bound:f32,
    pub kind:u32, pub delay_mode:u8, pub time_scale:f32,
}
impl EventAdmission {
    pub(crate) fn queued(&self)->Option<f64> {
        if ![self.now,self.deadline,self.candidate,self.epoch].iter().all(|v|v.is_finite())
            || ![self.dt,self.distance,self.total_bound,self.time_scale].iter().all(|v|v.is_finite())
            || self.dt<=0. || self.total_bound<0. || self.time_scale<0. || self.delay_mode>2 || self.kind==0 {return None;}
        let mut at=self.candidate;
        if at-self.now<0.000001f32 as f64 {
            if self.delay_mode==0 {at=self.now;}
            else {
                let distance=(self.distance-0.000634745986f32) as f64;
                let scale=self.time_scale as f64;let dt=self.dt as f64;
                at=self.now+if self.delay_mode==2 || self.kind&15!=0 {
                    if distance<0.000001f32 as f64 {scale*0.001f32 as f64*dt}
                    else {scale*0.0001f32 as f64*dt+distance/self.total_bound as f64}
                } else if distance<0.000001f32 as f64 {scale*0.00001f32 as f64*dt}
                else {scale*0.0000001f32 as f64*dt+(distance*0.1f32 as f64)/self.total_bound as f64};
                if at>=self.deadline {return None;}
            }
        }
        (at>=self.now && at<=self.deadline).then(||queue_time(at,self.epoch))
    }
}
pub(crate) struct Refined {
    pub time:f64,
    pub queries:usize,
    pub exhausted:bool,
}
pub(crate) fn refine(mut low:f64,mut high:f64,target:f32,mut d0:f32,mut d1:f32,mut evaluate:impl FnMut(f64)->f64)->Option<Refined> {
    if !low.is_finite() || !high.is_finite() || low>high || !target.is_finite() || !d0.is_finite() || !d1.is_finite() || d0==d1 {return None;}
    let mut iteration=0usize;
    loop {
        let mut at=((high-low)*(target as f64-d0 as f64))/(d1 as f64-d0 as f64)+low;
        if iteration&3==3 {
            if iteration>64 {return Some(Refined{time:low,queries:iteration,exhausted:true});}
            at=((low-at)+(high-at))*0.375+at;
        }
        if !at.is_finite() {return None;}
        let value=evaluate(at);
        if !value.is_finite() {return None;}
        if (value-target as f64).abs()<f32::from_bits(994352038) as f64 {
            return Some(Refined{time:at,queries:iteration+1,exhausted:false});
        }
        iteration+=1;
        let stored=value as f32;
        if target<=stored {low=at;d0=stored;} else {high=at;d1=stored;}
    }
}
pub(crate) struct Bracketed {
    pub refined:Refined,
    pub low:f64,
    pub high:f64,
}
/// Owned 10127aa0 descending, initial-index-zero conservative advance
/// using 5ms cached pose samples.
/// The caller owns the signed-distance evaluator and its motion bound.
pub(crate) fn search(start:f64,deadline:f64,target:f32,initial:f32,inverse_bound:f32,mut evaluate:impl FnMut(f64)->f64)->Option<Bracketed> {
    search_indexed(start,deadline,target,initial,inverse_bound,0,|_,at|evaluate(at))
}
/// The parent feature handler shares its21 quantized endpoint caches across
/// searches. None requests a fresh arbitrary-time refinement evaluation.
pub(crate) fn search_indexed(start:f64,deadline:f64,target:f32,initial:f32,inverse_bound:f32,start_index:usize,mut evaluate:impl FnMut(Option<usize>,f64)->f64)->Option<Bracketed> {
    if !start.is_finite() || !deadline.is_finite() || start>deadline || !target.is_finite() || !initial.is_finite() || !inverse_bound.is_finite() || inverse_bound<=0. {return None;}
    if initial<=target {return Some(Bracketed{refined:Refined{time:start,queries:0,exhausted:false},low:start,high:start});}
    let mut low=start;let mut d0=initial;let mut delta=(initial as f64-target as f64)*inverse_bound as f64;let mut index=start_index;
    while (low-deadline)+delta<=0. {
        delta+=delta;
        if (low-deadline)+delta>0. {delta=(deadline-low)+0.002999999992425728;}
        let steps=((delta as f32*200.) as i32).max(1);
        index+=steps as usize;
        if index>20 {return None;}
        let high=steps as f64*0.004999999888241291+low;
        let value=evaluate(Some(index),high);if !value.is_finite() {return None;}
        if value<=target as f64 {
            let refined=refine(low,high,target,d0,value as f32,|at|evaluate(None,at))?;
            return (refined.time<=deadline).then_some(Bracketed{refined,low,high});
        }
        if index==20 {return None;}
        low=high;d0=value as f32;delta=(value-target as f64)*inverse_bound as f64;
    }
    None
}
///101157a0 sphere-pair feature inputs. The owner supplies prepared initial
/// object frames and the same object sampler used by the 21-slot pose caches.
/// Pair discovery and queue insertion remain responsibilities of the owner.
pub(crate) struct SphereCandidate {
    pub now:f64, pub deadline:f64, pub margin:f32, pub target:f32,
    pub gap:f32, pub bound:f32, pub initial:[crate::ivp_motion::Pose;2],
    pub inactive:[bool;2],
}
pub(crate) fn sphere_candidate(i:SphereCandidate,mut sample:impl FnMut(usize,f64)->crate::ivp_motion::Pose)->Option<f64> {
    if !i.now.is_finite()||!i.deadline.is_finite()||i.now>i.deadline
        ||[i.margin,i.target,i.gap,i.bound].iter().any(|v|!v.is_finite())||i.bound<0.
        ||i.initial.iter().any(|p|!p.position.is_finite()||!p.rotation.is_finite()){return None;}
    let mut normal=i.initial[1].position-i.initial[0].position;
    let square=(normal.x*normal.x+normal.y*normal.y)+normal.z*normal.z;
    if square>=f32::from_bits(0x2edbe6ff) {
        let inverse=1./(square as f64).sqrt();
        normal=Vec3::from_array(normal.to_array().map(|v|(v as f64*inverse) as f32));
    }
    let mut frames=[[None;21];2];
    for side in 0..2 {if i.inactive[side]{frames[side].fill(Some(i.initial[side]));}else{frames[side][0]=Some(i.initial[side]);}}
    let mut evaluate=|index:Option<usize>,at:f64| {
        let poses:[crate::ivp_motion::Pose;2]=std::array::from_fn(|side|match index {
            Some(index) if index<21=>*frames[side][index].get_or_insert_with(||sample(side,at)),
            Some(_)=>crate::ivp_motion::Pose{position:Vec3::NAN,rotation:Mat3::IDENTITY},
            None if i.inactive[side]=>i.initial[side],
            None=>sample(side,at),
        });
        sphere_distance(poses[0],poses[1],[Vec3::ZERO;2],normal)
    };
    let initial=evaluate(Some(0),i.now) as f32;let target=i.margin+i.target;
    let inverse=i.bound.recip();
    if target<initial {
        return search_indexed(i.now,i.deadline,target,initial,inverse,0,&mut evaluate).map(|r|r.refined.time);
    }
    //10127cc0's near-surface branch shares the slot cache with any descending
    // search it starts later. A decreasing sample returns the preceding time.
    let escape=i.margin*0.5+i.gap;let mut at=i.now;let mut previous=initial;let mut index=0;
    while at<i.deadline {
        let delta=(previous as f64-escape as f64)*inverse as f64;
        let short=if (at-i.deadline)+delta<=0. {(delta as f32).max(0.)}
            else{((i.deadline-at)+0.002999999992425728) as f32};
        let steps=((short*200.) as i32).max(1) as usize;index+=steps;
        if index>20{return None;}
        let next=at+steps as f64*0.004999999888241291;
        let value=evaluate(Some(index),next);if !value.is_finite(){return None;}
        if (target as f64)<value {
            if next>i.deadline{return None;}
            return search_indexed(next,i.deadline,target,value as f32,inverse,index,&mut evaluate).map(|r|r.refined.time);
        }
        if value as f32<=initial{return Some(at);}
        at=next;previous=value as f32;
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use crate::ivp_motion::{Prediction,ObjectFrame};
    #[test]
    fn native_sphere_feature_candidates_match_owned_cached_pose_search(){
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-candidate.json")).unwrap();
        let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);
        let double=|v:&serde_json::Value|f64::from_bits(v.as_u64().unwrap());
        let vector=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|k|f(&v[k])));
        let mut hits=0;
        for c in d["cases"].as_array().unwrap(){
            let bodies:Vec<_>=c["before"]["physical"].as_array().unwrap().iter().map(crate::ivp_joint_apply::tests::body).collect();
            let initial:[crate::ivp_motion::Pose;2]=std::array::from_fn(|side|{let p=&c["initial_frames"][side];crate::ivp_motion::Pose{position:vector(&p["position"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|k|f(&p["rotation"][k]))).transpose()}});
            let routed=crate::ivp_feature_handler::dispatch::run(crate::ivp_feature_handler::dispatch::Input{sides:std::array::from_fn(|side|{let b=bodies[side];crate::ivp_feature_handler::dispatch::Side{feature:crate::ivp_feature_handler::dispatch::Feature::Sphere,body:crate::ivp_feature_handler::Body{prediction:b.prediction,frame:ObjectFrame{shift:None,rotation:None},flags:b.object_flags,cached:initial[side],angular_speed:0.,inverse_diameter:0.},radius:1.,linear_speed:0.,angular_radius:0.}}),flags:0,now:double(&c["now"]),deadline:double(&c["deadline"]),margin:f(&c["margin"]),target:f(&c["target_bits"]),gap:f(&c["gap_bits"]),bound:f(&c["bound"]),total_bound:0.,distance:0.,normal:Vec3::ZERO,face_epsilon:0.000634745986}).unwrap();assert_eq!(routed.route.handler,0x1157a0);let result=routed.outcome.candidate;
            let expected=if c["kind"]==16{hits+=1;Some(c["candidate"].as_u64().unwrap())}else{assert_eq!(c["kind"],0);None};
            assert_eq!(result.map(f64::to_bits),expected,"native sphere feature case {}",c["id"]);
            assert_eq!(c["before"],c["after"],"native observation changed physical motion");
        }
        assert_eq!(d["cases"].as_array().unwrap().len(),48);assert!(hits>0&&hits<48);
    }
    #[test]
    fn native_sphere_signed_distance_matches_all_owned_evaluator_outputs(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-distance.json")).unwrap();
        assert_eq!(data["projection_scale_bits"].as_u64().unwrap(),0x3f99999a);
        let f=|v:&serde_json::Value|f32::from_bits(v.as_u64().unwrap() as u32);
        let vector=|v:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|f(&v[i])));
        let pose=|v:&serde_json::Value|crate::ivp_motion::Pose{position:vector(&v["position"]),rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&v["rotation"][i]))).transpose()};
        let mut negative=0;let mut projected=0;let mut radial=0;
        for c in data["cases"].as_array().unwrap(){
            let a=pose(&c["a"]);let b=pose(&c["b"]);let points=[vector(&c["points"][0]),vector(&c["points"][1])];let normal=vector(&c["normal"]);
            let result=sphere_distance(a,b,points,normal);assert_eq!(result.to_bits(),c["result"].as_u64().unwrap(),"native sphere evaluator case {}",c["id"]);
            negative+=usize::from(result<0.);let delta=b.transform_point(points[1])-a.transform_point(points[0]);let p=((normal.y*delta.y+normal.x*delta.x)+normal.z*delta.z)*f32::from_bits(0x3f99999a);
            if result.to_bits()==(p as f64).to_bits(){projected+=1;}else{radial+=1;}
        }
        assert_eq!(data["cases"].as_array().unwrap().len(),256);assert!(negative>0&&projected>0&&radial>0);
    }
    fn fixture()->serde_json::Value {serde_json::from_str(include_str!("../tools/fixtures/native-ivp-time-refine.json")).unwrap()}
    fn v(a:&serde_json::Value)->Vec3 {Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32))}
    fn q(a:&serde_json::Value)->[f32;4] {std::array::from_fn(|i|a[i].as_f64().unwrap() as f32)}
    fn pose(object:&serde_json::Value,at:f64)->crate::ivp_motion::Pose {
        Prediction {start:v(&object["start"]),velocity:v(&object["velocity"]),rotation_start:q(&object["q0"]),rotation_end:q(&object["q1"]),
            time:object["time"].as_f64().unwrap(),inverse_dt:object["inverse_dt"].as_f64().unwrap() as f32}.sample(at,ObjectFrame {
                shift:object["shifted"].as_bool().unwrap().then(||v(&object["shift"])),
                rotation:object["rotated"].as_bool().unwrap().then(||q(&object["object_q"]))})
    }
    fn prepared_pose(o:&serde_json::Value,at:f64)->crate::ivp_motion::Pose {
        Prediction{start:v(&o["start"]),velocity:v(&o["velocity"]),rotation_start:q(&o["q0"]),rotation_end:q(&o["q1"]),time:o["time"].as_f64().unwrap(),inverse_dt:o["inverse_dt"].as_f64().unwrap() as f32}.prepare_object_cache(at,ObjectFrame{shift:o["shifted"].as_bool().unwrap().then(||v(&o["shift"])),rotation:o["rotated"].as_bool().unwrap().then(||q(&o["object_q"]))},0).pose
    }
    #[test]
    fn native_initial_separation_uses_prepared_cache_instead_of_public_pose(){
        let data=fixture();let mut count=0;let mut public_differences=0;
        for c in data["cases"].as_array().unwrap(){if c["evaluator_rva"]!=0x113970{continue;}
            let at=c["low"].as_f64().unwrap();let actual=point_face_distance(prepared_pose(&c["objects"][0],at),prepared_pose(&c["objects"][1],at),v(&c["point"]),v(&c["origin"]),v(&c["normal"])) as f32;
            let expected=c["d0"].as_f64().unwrap() as f32;assert_eq!(actual,expected,"{} step{}",c["model"],c["step"]);count+=1;
            if distance(c,at) as f32!=expected{public_differences+=1;}
        }assert_eq!(count,44);
        // Correct100eda40 shift ordering removes one false public-pose
        // difference; the six remaining differences require cached preparation.
        assert_eq!(public_differences,6);
    }
    fn distance(case:&serde_json::Value,at:f64)->f64 {
        let a=pose(&case["objects"][0],at);let b=pose(&case["objects"][1],at);
        if case["evaluator_rva"].as_u64().unwrap()==0x113f80 {
            return direction_distance(a.rotation,b.rotation,v(&case["point"]),v(&case["normal"]));
        }
        point_face_distance(a,b,v(&case["point"]),v(&case["origin"]),v(&case["normal"]))
    }
    #[test]
    fn native_queue_admission_matches_recorded_insertions() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-event-admission.json")).unwrap();
        let mut max=0f64;let mut delayed=0;
        for c in data["cases"].as_array().unwrap() {
            let f=|k:&str|c[k].as_f64().unwrap();
            let input=EventAdmission {now:f("low"),deadline:f("deadline"),candidate:f("candidate"),epoch:f("epoch"),
                dt:f("dt") as f32,distance:f("distance") as f32,total_bound:f("total_bound") as f32,
                kind:c["kind"].as_u64().unwrap() as u32,delay_mode:1,time_scale:f("time_scale") as f32};
            let at=input.queued().unwrap();let error=(at-f("scheduled")).abs();max=max.max(error);
            if input.candidate-input.now<0.000001f32 as f64 {delayed+=1;}
            assert!(error<1e-9,"{} step{} error{}",c["model"],c["step"],error);
        }
        assert_eq!(delayed,2);
        println!("{} native queue admissions, {} delayed: max{} seconds",data["cases"].as_array().unwrap().len(),delayed,max);
    }
    #[test]
    fn native_direction_targets_use_cached_distance_and_collider_size() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-feature-target.json")).unwrap();
        for c in data["cases"].as_array().unwrap() {
            let p=&c["input"];assert_eq!(p["matched"],true);
            let f=|key:&str|p[key].as_f64().unwrap() as f32;
            let actual=feature_target(f("margin"),f("distance"),f("target"),f("inverse_diameter")).unwrap();
            assert_eq!(actual.to_bits(),(c["expected"].as_f64().unwrap() as f32).to_bits());
        }
    }
    #[test]
    fn observed_feature_queue_clocks_use_float_offsets() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-queue-clocks.json")).unwrap();
        for c in data["cases"].as_array().unwrap() {
            let epoch=(c["step"].as_u64().unwrap()+1) as f64*(1f32/60.) as f64;
            assert_eq!(queue_time(c["direction_candidate_time"].as_f64().unwrap(),epoch),c["face_search_start"].as_f64().unwrap());
        }
    }
    #[test]
    fn native_directional_feature_transition_queries_match() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-feature-time.json")).unwrap();
        let mut max_d=0f64;let mut max_t=0f64;
        for c in data["cases"].as_array().unwrap() {
            let mut queries=vec![];let result=refine(c["low"].as_f64().unwrap(),c["high"].as_f64().unwrap(),c["target"].as_f64().unwrap() as f32,
                c["d0"].as_f64().unwrap() as f32,c["d1"].as_f64().unwrap() as f32,|t|{queries.push(t);distance(c,t)}).unwrap();
            assert!(!result.exhausted);let expected=c["queries"].as_array().unwrap();assert_eq!(queries.len(),expected.len());
            max_t=max_t.max((result.time-c["result"].as_f64().unwrap()).abs());
            for (time,q) in queries.iter().zip(expected) {
                max_t=max_t.max((time-q["time"].as_f64().unwrap()).abs());
                max_d=max_d.max((distance(c,q["time"].as_f64().unwrap()) as f32-q["distance"].as_f64().unwrap() as f32).abs() as f64);
            }
        }
        println!("{} native direction transitions: max dot error{max_d}, time error{max_t}s",data["cases"].as_array().unwrap().len());
        assert!(max_d<0.000002 && max_t<0.000000001);
    }
    #[test]
    fn native_point_face_brackets_follow_quantized_conservative_advance() {
        let data=fixture();let mut passed=0;let mut max_time=0f64;
        for case in data["cases"].as_array().unwrap() {
            let input=&case["search"];assert_eq!(input["matched"],true);assert_eq!(input["has_initial"],true);assert_eq!(input["index"],0);
            let start=input["low"].as_f64().unwrap();let deadline=input["deadline"].as_f64().unwrap();
            let target=case["target"].as_f64().unwrap() as f32;
            let result=search(start,deadline,target,input["initial"].as_f64().unwrap() as f32,case["inverse_bound"].as_f64().unwrap() as f32,|t|distance(case,t));
            if let Some(result)=result {
                let error=(result.refined.time-case["result"].as_f64().unwrap()).abs();max_time=max_time.max(error);
                if error<0.000000001 {passed+=1;} else {println!("bracket mismatch {} step{} error{} low {} != {} high {} != {}",case["model"],case["step"],error,result.low,case["low"],result.high,case["high"]);}
            } else {println!("missing bracket {} step{}",case["model"],case["step"]);}
        }
        println!("Native point-face bracket replay: {passed}/{}; max time error{max_time}",data["cases"].as_array().unwrap().len());
        assert_eq!(passed,data["cases"].as_array().unwrap().len());
    }
    #[test]
    fn controlled_native_refinement_covers_nonlinearity_midpoint_and_iteration_limit() {
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-time-refine-controlled.json")).unwrap();
        let mut max_time=0f64;
        for case in fixture["cases"].as_array().unwrap() {
            let kind=case["kind"].as_u64().unwrap();
            let evaluate=|t:f64| {
                let x=t as f32;
                // The helper keeps kind 1's product in x87; kind 2 spills its square.
                match kind {0=>(1.-x) as f64,1=>1.-(x as f64)*(x as f64),2=>{let square=x*x;(1.-square*square) as f64},
                    3=>(-5.*x as f64).exp() as f32 as f64,_=>if x<0.6 {1.}else{-1.}}
            };
            let expected=case["queries"].as_array().unwrap();let mut queries=vec![];
            let result=refine(case["low"].as_f64().unwrap(),case["high"].as_f64().unwrap(),case["target"].as_f64().unwrap() as f32,
                case["d0"].as_f64().unwrap() as f32,case["d1"].as_f64().unwrap() as f32,|t|{queries.push(t);evaluate(t)}).unwrap();
            assert_eq!(queries.len(),expected.len(),"kind{kind}");assert_eq!(result.queries,queries.len());
            assert_eq!(result.exhausted,kind==4);max_time=max_time.max((result.time-case["result"].as_f64().unwrap()).abs());
            for (actual,expected) in queries.iter().zip(expected) {
                max_time=max_time.max((actual-expected["time"].as_f64().unwrap()).abs());
                assert_eq!(evaluate(expected["time"].as_f64().unwrap()) as f32,expected["distance"].as_f64().unwrap() as f32);
            }
        }
        println!("5 controlled native refinements (1/3/2/4/67 queries): max time error{max_time}s");
        assert!(max_time<0.000000001);
    }
    #[test]
    fn native_point_face_evaluations_and_refinement_queries_match() {
        let data=fixture();let mut max_d=0f64;let mut max_t=0f64;let mut max_qt=0f64;let mut counts=0;let mut audit=vec![];
        for case in data["cases"].as_array().unwrap() {
            if std::env::var_os("L4D2R_IVP_DISTANCE_AUDIT").is_some(){
                let at=case["low"].as_f64().unwrap();
                let prepared=|i|prepared_pose(&case["objects"][i],at);
                let cache=point_face_distance(prepared(0),prepared(1),v(&case["point"]),v(&case["origin"]),v(&case["normal"]));
                audit.push(serde_json::json!({"model":case["model"],"step":case["step"],"evaluator_rva":case["evaluator_rva"],"low":at,"prediction_time":case["objects"][0]["time"],"expected_low":case["d0"],"public_low":distance(case,at),"prepared_low":cache,"queries":case["queries"].as_array().unwrap().iter().map(|n|{let at=n["time"].as_f64().unwrap();serde_json::json!({"at":at,"actual":distance(case,at),"expected":n["distance"]})}).collect::<Vec<_>>()}));
            }
            for query in case["queries"].as_array().unwrap() {max_d=max_d.max((distance(case,query["time"].as_f64().unwrap()) as f32-query["distance"].as_f64().unwrap() as f32).abs() as f64);}
            let mut times=vec![];
            let result=refine(case["low"].as_f64().unwrap(),case["high"].as_f64().unwrap(),case["target"].as_f64().unwrap() as f32,
                case["d0"].as_f64().unwrap() as f32,case["d1"].as_f64().unwrap() as f32,|t|{times.push(t);distance(case,t)}).unwrap();
            assert!(!result.exhausted);max_t=max_t.max((result.time-case["result"].as_f64().unwrap()).abs());
            let expected=case["queries"].as_array().unwrap();
            if times.len()!=expected.len() {println!("count mismatch {} step{} actual{} expected{}",case["model"],case["step"],times.len(),expected.len());}
            assert_eq!(times.len(),expected.len());assert_eq!(result.queries,times.len());
            for (actual,expected) in times.iter().zip(expected) {max_qt=max_qt.max((actual-expected["time"].as_f64().unwrap()).abs());}
            counts+=1;
        }
        if let Ok(path)=std::env::var("L4D2R_IVP_DISTANCE_AUDIT"){std::fs::write(path,serde_json::to_vec_pretty(&audit).unwrap()).unwrap();}
        println!("{counts} native time refinements: distance {max_d}m, return {max_t}s, query time {max_qt}s");
        assert!(max_d<0.000002 && max_t<0.000000001 && max_qt<0.000000001);
    }
}
