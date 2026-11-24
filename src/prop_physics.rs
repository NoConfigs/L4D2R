//! Local rigid-prop motion using native bounds, mass and map flags.
//! This is a limited solver, not a port of IVP/VPhysics contact stacking.
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct Props(
    pub HashMap<usize, Body>,
    f32,
    Vec<crate::physics_data::Surface>,
);
#[derive(Clone)]
pub struct Body {
    pub velocity: Vec3,
    angular: Vec3,
    pub asleep: bool,
    disabled: bool,
    pub mass: f32,
    supported_sleep: bool,
    /// The physics prop this body went to sleep resting on (a box on a box).
    /// The periodic support re-check only sees the world environment, so a
    /// body resting on another prop woke every 0.1 s, fell back into the
    /// pair solver and slept again: c12m3's start (a box on a box, a sack on
    /// a sack) spent ~23 ms a frame in map::prop_pairs.
    resting_on: Option<usize>,
    support_clock: f32,
    surface: crate::physics_data::Surface,
    on_floor: bool,
    inverse_inertia: Vec3,
    /// Renderer-local authored centre of mass, distinct from collision bounds.
    center: Vec3,
    damping:f32,
    angular_damping:f32,
    rotation: Quat,
    hulls: Vec<crate::rigid_contact::Hull>,
    failed_recovery: Option<(Vec3, Vec3, (usize, u64))>,
    debris_motion: Option<crate::articulated::Articulation>,
    /// Exact entity matrix we last published. Own Euler/Source round trips
    /// must not masquerade as authored motion and invalidate physical state.
    debris_pose: Option<l4d2r_math::Mat4>,
    /// Debris no-progress watch: (anchor position, seconds within 2 cm of
    /// it; 0.1 m ~ 4 units since c4m3 shards jitter ~3 units), asleep because of it). c4m3's pre-broken window shards jittered
    /// on their thin faces forever, ~30 ms of physics a frame.
    progress: (Vec3, f32, bool),
}
impl Body {
    pub(crate) fn launch_debris(&mut self, velocity: Vec3, angular: Vec3, disabled: bool) {
        self.velocity = velocity;
        self.angular = angular;
        self.debris_pose=None;
        let vertices = self.hulls.iter().flat_map(|h| h.vertices.iter().copied()).collect::<Vec<_>>();
        let center = self.center;
        let inertia = Mat3::from_diagonal(self.inverse_inertia);
        self.debris_motion = Some(crate::articulated::Articulation { initial_intrusions: None, pending_penetrations: Vec::new(),
            bodies: vec![crate::articulated::Rigid {
                bone: 0, position: Vec3::ZERO, rotation: Quat::IDENTITY, velocity, angular,
                center, inverse_mass: self.mass.recip(), inverse_inertia: inertia,
                vertices, hulls: self.hulls.clone(), damping: self.damping, angular_damping: self.angular_damping,
                surface: self.surface,
                world_contacts: Vec::new(), pose_cache: None,
            }], joints: vec![], collision_pairs: vec![],
            friction: self.surface.friction, elasticity: self.surface.elasticity,
        });
        self.asleep = disabled;
        self.disabled = disabled;
        self.supported_sleep = false;
        self.support_clock = 0.;
        self.failed_recovery = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore="manual matched owned PHY free/drop trajectory comparison"]
    fn replay_owned_phy_motion() {
        let path=std::env::var("L4D2R_PHY_MOTION_REFERENCE").expect("native reference path");
        let output=std::env::var("L4D2R_PHY_MOTION_REPORT").expect("comparison report path");
        let data:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let report=compare_owned_phy_motion(&data,std::env::var_os("L4D2R_PHY_EVENT_TRIAL").is_some(),std::env::var_os("L4D2R_PHY_PERSISTENT_TRIAL").is_some());
        std::fs::write(output,serde_json::to_string_pretty(&report).unwrap()).unwrap();
        assert!(report["passed"].as_bool().unwrap(),"Owned native PHY trajectory gates failed; see report");
    }
    #[test]
    fn unseeded_contacts_remove_the_native_drop_step30_divergence_through_step40(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        // A bounded regression for the original single-contact integration
        // failure, not a replacement for the still-failing540-step full test.
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-unseeded-drop-window.json")).unwrap();
        assert_eq!(data["comparison_end_step"],40);
        let report=compare_owned_phy_motion(&data,true,true);
        assert!(report["passed"].as_bool().unwrap(),"{report}");
        assert_eq!(report["cases"][0]["samples"].as_array().unwrap().last().unwrap()["step"],40);
        assert!(report["cases"][0]["event_trial"].as_array().unwrap().iter().any(|s|s["events"].as_array().unwrap().iter().any(|e|e["kind"]=="retained_response"&&e["path"]=="batch")));
        let native:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-phase.json")).unwrap();let mut timings=0;
        for row in native["countdowns"].as_array().unwrap().iter().filter(|r|r["model"]==data["cases"][0]["model"]&&r["step"].as_u64().unwrap()<=40){
            let tick=report["cases"][0]["event_trial"].as_array().unwrap().iter().find(|t|t["step"]==row["step"]).unwrap();
            let response=tick["events"].as_array().unwrap().iter().find(|e|e["kind"]=="retained_response").unwrap();
            assert_eq!(response["friction_group"]["timer_before"],row["before"][0]["timer"]);
            assert_eq!(response["friction_group"]["timer_after"],row["after"][0]["timer"]);timings+=1;
        }
        assert_eq!(timings,7);
    }
    #[test]
    fn impact_neighbours_refresh_before_retry_and_preserve_native_motion_through_step36(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-refresh.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap(),"{report}");
        assert_eq!(report["cases"][0]["samples"].as_array().unwrap().last().unwrap()["step"],36);
        let calls=report["cases"][0]["event_trial"].as_array().unwrap().iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="impact_neighbour_refresh").map(move |e|(&t["step"],e))).collect::<Vec<_>>();
        let expected=data["profiles"][2]["calls"].as_array().unwrap();assert_eq!(calls.len(),8);assert_eq!(expected.len(),8);
        let f=|v:&serde_json::Value|v.as_f64().unwrap();
        let close=|a:f64,b:f64,t:f64|assert!((a-b).abs()<=t,"{a} != {b} within {t}");
        for ((step,a),b) in calls.into_iter().zip(expected){
            assert_eq!(*step,b["step"]);assert_ne!(a["id"],a["excluded"]);assert_eq!(a["status"],b["status"]);
            close(f(&a["time"]),f(&b["now"]),1e-6);close(f(&a["before_time"]),f(&b["before_time"]),1e-6);close(f(&a["distance"]),f(&b["distance"]),1e-6);
            for key in ["point","linear","angular"]{for axis in 0..3{close(f(&a[key][axis]),f(&b[key][axis]),if key=="angular"{5e-5}else{1e-6});}}
            // The floor's equivalent edge basis can rotate the two mix axes.
            // Compare their world vector rather than requiring one edge ID.
            for axis in 0..3{let world=|r:&serde_json::Value|(0..2).map(|i|f(&r["mix_after"][i])*f(&r["basis"][i][axis])).sum::<f64>();close(world(a),world(b),1e-6);}
        }
    }
    #[test]
    fn retry_count_resets_for_a_second_primary_impact_in_the_same_tick(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-retry-epoch.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap(),"{report}");
        assert_eq!(report["cases"][0]["samples"].as_array().unwrap().last().unwrap()["step"],33);
        let calls=report["cases"][0]["event_trial"].as_array().unwrap().iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="impact"||e["kind"]=="retry").map(move |e|(&t["step"],e))).collect::<Vec<_>>();
        let expected=data["profiles"][1]["calls"].as_array().unwrap();assert_eq!(calls.len(),10);assert_eq!(calls.len(),expected.len());
        let f=|v:&serde_json::Value|v.as_f64().unwrap();let close=|a:f64,b:f64,t:f64|assert!((a-b).abs()<=t,"{a} != {b} within {t}");
        for ((step,a),b) in calls.iter().copied().zip(expected){
            assert_eq!(*step,b["step"]);assert_eq!(a["system_pushes"],b["system_pushes"]);
            let epoch=(step.as_u64().unwrap()+1) as f64*(1f32/60.) as f64;close(epoch+f(&a["time"]),f(&b["time"]),1e-6);
            for (key,native_key,tolerance) in [("arm","arm",1e-6),("velocity_after","velocity",5e-6),("angular_after","angular",5e-5)]{
                for axis in 0..3{close(f(&a[key][axis]),f(&b[if key=="arm"{"before"}else{"after"}][1][native_key][axis]),tolerance);}
            }
        }
        let last=calls.last().unwrap().1;let earlier=calls.iter().rev().skip(1).find(|(_,e)|e["kind"]=="retry"&&e["vertex"]==last["vertex"]).unwrap().1;
        assert_eq!(last["kind"],"retry");assert_eq!(last["system_pushes"],1);assert_eq!(earlier["system_pushes"],1);assert!(f(&earlier["time"])<f(&last["time"]));
    }
    #[test]
    fn queued_contact_impulses_commit_before_friction_until_the_unit_sleeps(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-force-stage.json")).unwrap();
        // The broader six-shape regression also verifies every motion/sleep
        // sample. This case retains the force-stage ordering contract.
        let report=compare_owned_phy_motion(&data["motion"],true,true);assert!(report["passed"].as_bool().unwrap(),"{report}");
        assert_eq!(report["cases"][0]["samples"].as_array().unwrap().last().unwrap()["step"],540);
        assert!(!report["cases"][0]["event_trial_exhausted"].as_bool().unwrap());
        assert_eq!(report["cases"][0]["samples"].as_array().unwrap().len(),541);
        for tick in report["cases"][0]["event_trial"].as_array().unwrap(){
            let events=tick["events"].as_array().unwrap();
            if events.iter().any(|e|e["kind"]=="retained_sleep_hold"){assert_eq!(events.len(),1);continue;}
            let force=events.iter().position(|e|e["kind"]=="retained_force_prepare").unwrap();
            if let Some(work)=events.iter().position(|e|e["kind"]=="retained_work_refresh"){assert!(work<force);}
            if let Some(response)=events.iter().position(|e|e["kind"]=="retained_response"){
                assert!(force<response);assert_eq!(events[response]["velocity_before"],events[force]["linear_after"]);assert_eq!(events[response]["angular_before"],events[force]["angular_after"]);
                if !events[response]["normal_system"].is_null(){for k in ["pending_linear","pending_angular"]{assert!(events[response]["normal_system"]["core"][k].as_array().unwrap().iter().all(|v|v.as_f64().unwrap()==0.));}}
            }
        }
        let forces=report["cases"][0]["event_trial"].as_array().unwrap().iter().flat_map(|t|t["events"].as_array().unwrap()).filter(|e|e["kind"]=="retained_force_prepare").collect::<Vec<_>>();
        let sleep:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sleep-drop.json")).unwrap();assert_eq!(forces.len() as u64,sleep["profiles"][2]["sleep_step"].as_u64().unwrap());
        assert!(forces.iter().any(|e|["pending_linear_before","pending_angular_before"].iter().any(|k|e[*k].as_array().unwrap().iter().any(|v|v.as_f64().unwrap()!=0.))));
        for e in forces{for k in ["pending_linear_after","pending_angular_after"]{assert!(e[k].as_array().unwrap().iter().all(|v|v.as_f64().unwrap()==0.));}}
    }
    #[test]
    fn owned_cache_links_flags_references_and_sleep_reset_follow_native_lifecycle(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-cache.json")).unwrap();let report=compare_owned_phy_motion(&data,true,true);let mut count=0;let mut sleeps=0;
        assert!(report["passed"].as_bool().unwrap());
        for (c,n) in report["cases"].as_array().unwrap().iter().zip(data["cases"].as_array().unwrap()){
            for (a,b) in c["samples"].as_array().unwrap().iter().zip(n["samples"].as_array().unwrap()){
                for key in ["object_flags","object_cache_bound","object_cache_references","object_cache_owner"]{assert_eq!(a["core_motion"][key],b["world_cache"][key],"{} step{} {key}",c["model"],a["step"]);}
                if let Some(native)=b.get("sleep_prediction"){
                    let p=&a["core_motion"]["diagnostic_prediction"];assert!(p["velocity"].as_array().unwrap().iter().all(|v|v.as_f64().unwrap()==0.));assert_eq!(p["rotation_start"],p["rotation_end"]);
                    assert!((p["time"].as_f64().unwrap()-native["time"].as_f64().unwrap()).abs()<1e-12);assert_eq!(p["inverse_dt"].as_f64().unwrap() as f32,native["inverse_dt"].as_f64().unwrap() as f32);sleeps+=1;
                }count+=1;
            }
        }assert_eq!(count,3246);assert!(sleeps>0);
    }
    #[test]
    fn free_world_keeps_no_unused_floor_or_object_cache_and_matches_native_epochs(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-free-cache.json")).unwrap();let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap());let mut count=0;
        for (c,n) in report["cases"].as_array().unwrap().iter().zip(data["cases"].as_array().unwrap()){
            for (a,b) in c["samples"].as_array().unwrap().iter().zip(n["samples"].as_array().unwrap()){
                assert_eq!(a["core_motion"]["registered_objects"],1);
                for key in ["object_flags","object_cache_bound","object_cache_frame","object_cache_references","object_cache_owner","environment_frame"]{assert_eq!(a["core_motion"][key],b["world_cache"][key],"{} step{} {key}",c["model"],a["step"]);}count+=1;
            }
        }assert_eq!(count,3246);
    }
    #[test]
    fn owned_bounds_driver_regenerates_native_precontact_dispatch_and_cache_epochs(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-cache.json")).unwrap();
        let native:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hull-budget.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap());
        let mut dispatches=0;let mut epochs=0;
        for (case,expected) in report["cases"].as_array().unwrap().iter().zip(data["cases"].as_array().unwrap()){
            let model=case["model"].as_str().unwrap().replace(".mdl",".phy");
            assert_eq!(model,expected["model"].as_str().unwrap());
            // Bound this regression by the independently recorded first impact.
            // The full scheduler-frame test below remains strict after contact.
            let contact=expected["collision_trace"].as_array().unwrap().iter().map(|e|e["step"].as_u64().unwrap()).min().unwrap();
            let actual=case["event_trial"].as_array().unwrap().iter().filter(|t|t["step"].as_u64().unwrap()<contact).flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="bounds_dispatch").map(move |e|(t["step"].clone(),e["time"].clone()))).collect::<Vec<_>>();
            let expected_dispatches=native["cases"].as_array().unwrap().iter().filter(|e|e["model"]==model&&e["step"].as_u64().unwrap()<contact&&e["queued_after"].as_u64().unwrap()>e["queued_before"].as_u64().unwrap()).map(|e|(e["step"].clone(),e["time"].clone())).collect::<Vec<_>>();
            assert_eq!(actual,expected_dispatches,"{} precontact dispatch order/time",case["model"]);dispatches+=actual.len();
            for (a,b) in case["samples"].as_array().unwrap().iter().zip(expected["samples"].as_array().unwrap()).take(contact as usize){
                for k in ["object_cache_frame","environment_frame"]{assert_eq!(a["core_motion"][k],b["world_cache"][k],"{} step{} {k}",case["model"],a["step"]);}epochs+=1;
            }
        }
        assert_eq!(report["cases"].as_array().unwrap().len(),6);assert_eq!(dispatches,32);assert_eq!(epochs,153);
    }
    #[test]
    fn owned_feature_queries_match_two_pre_wake_cache_lifetimes(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-cache.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap());
        // These two complete pre-wake histories regress the feature-cache
        // omission. Other shapes still expose extra/missing minimizer events
        // in the strict six-shape, all-frame scheduler gate below.
        let mut count=0;
        for (case,native) in report["cases"].as_array().unwrap().iter().zip(data["cases"].as_array().unwrap()){
            if !["models/gibs/glass_shard01.mdl","models/gibs/glass_shard05.mdl"].contains(&case["model"].as_str().unwrap()){continue;}
            let wake=native["wake_event"]["before_step"].as_u64().unwrap();
            let mut features=0;
            for (a,b) in case["samples"].as_array().unwrap().iter().zip(native["samples"].as_array().unwrap()).take(wake as usize){
                for key in ["object_cache_frame","environment_frame"]{assert_eq!(a["core_motion"][key],b["world_cache"][key],"{} step{} {key}",case["model"],a["step"]);}count+=1;
            }
            for tick in case["event_trial"].as_array().unwrap().iter().filter(|t|t["step"].as_u64().unwrap()<wake){
                for e in tick["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="feature"){
                    assert!(e["cached_point_ivp"].as_array().unwrap().iter().all(|v|v.as_f64().unwrap().is_finite()));features+=1;
                }
            }assert!(features>0);
        }assert_eq!(count,240);
    }
    #[test]
    fn owned_cache_preparation_epochs_match_every_native_scheduler_frame(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-world-cache.json")).unwrap();let report=compare_owned_phy_motion(&data,true,true);
        for (c,n) in report["cases"].as_array().unwrap().iter().zip(data["cases"].as_array().unwrap()){
            for (a,b) in c["samples"].as_array().unwrap().iter().zip(n["samples"].as_array().unwrap()){
                for key in ["object_cache_frame","environment_frame"]{assert_eq!(a["core_motion"][key],b["world_cache"][key],"{} step{} {key}",c["model"],a["step"]);}
            }
        }
    }
    #[test]
    fn six_independent_flat_floor_drops_match_every_native_motion_and_sleep_sample(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sleep-drop.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap(),"{report}");assert_eq!(report["cases"].as_array().unwrap().len(),6);
        for (i,c) in report["cases"].as_array().unwrap().iter().enumerate(){let expected=&data["cases"][i];let profile=&data["profiles"][i];
            let samples=c["samples"].as_array().unwrap();assert_eq!(samples.len(),541);
            for (step,sample) in samples.iter().enumerate(){assert_eq!(sample["step"],step);if step>0{let sleeping=expected["samples"][step]["core_flags"].as_u64().unwrap()>>8==8;assert_eq!(sample["asleep"].as_bool().unwrap(),sleeping);}}
            let ticks=c["event_trial"].as_array().unwrap();let polls=ticks.iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="retained_movement_poll").map(move |e|(&t["step"],e))).collect::<Vec<_>>();
            let native_polls=profile["polls"].as_array().unwrap();assert_eq!(polls.len(),native_polls.len());
            for ((step,p),n) in polls.into_iter().zip(native_polls){assert_eq!(*step,n["step"]);assert_eq!(p["result"],n["result"]);assert!((p["time"].as_f64().unwrap()-n["now"].as_f64().unwrap()).abs()<1e-12);}
            let sleep=profile["sleep_step"].as_u64().unwrap() as usize;
            for tick in ticks{for e in tick["events"].as_array().unwrap(){if e["kind"]=="retained_force_prepare"{let step=tick["step"].as_u64().unwrap() as usize;let state=if step==1{1}else{expected["samples"][step-1]["core_flags"].as_u64().unwrap()>>8};assert_eq!(e["movement_state"],state);}}}
            let pose=&samples[sleep]["position_source"];for sample in &samples[sleep..]{assert_eq!(&sample["position_source"],pose);assert!(sample["asleep"].as_bool().unwrap());}
            assert!(!c["event_trial_exhausted"].as_bool().unwrap());
        }
    }
    #[test]
    fn dormant_central_impulse_resumes_motion_and_preserves_sleep_poll_timing(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-wake.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);
        for (i,c) in report["cases"].as_array().unwrap().iter().enumerate(){let expected=&data["cases"][i];let profile=&data["profiles"][i];
            let samples=c["samples"].as_array().unwrap();assert_eq!(samples.len(),541);
            // Every sample through132, including the queued wake and first
            //13 resumed ticks. Later impact/CCD divergence stays visible in
            //the full report: this test does not declare those gates passed.
            for sample in &samples[..=132]{assert!(sample["errors"].as_array().unwrap().iter().all(|e|e.as_f64().unwrap()<0.1),"{} {}",c["model"],sample);}
            for (step,sample) in samples.iter().enumerate().skip(1){assert_eq!(sample["asleep"].as_bool().unwrap(),expected["samples"][step]["core_flags"].as_u64().unwrap()>>8==8);}
            let ticks=c["event_trial"].as_array().unwrap();let wakes=ticks.iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="retained_wake").map(move |e|(&t["step"],e))).collect::<Vec<_>>();
            assert_eq!(wakes.len(),1);assert_eq!(*wakes[0].0,120);
            assert!(wakes[0].1["pending_linear"].as_array().unwrap().iter().zip([1.524,-2.54,0.]).all(|(a,b)|(a.as_f64().unwrap()-b).abs()<1e-6));
            let polls=ticks.iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="retained_movement_poll").map(move |e|(&t["step"],e))).collect::<Vec<_>>();let native=profile["polls"].as_array().unwrap();assert_eq!(polls.len(),native.len());
            for ((step,p),n) in polls.into_iter().zip(native){assert_eq!(*step,n["step"]);assert_eq!(p["result"],n["result"]);assert!((p["time"].as_f64().unwrap()-n["now"].as_f64().unwrap()).abs()<1e-12);}
            for (lo,hi) in [(75,120),(profile["sleep_step"].as_u64().unwrap() as usize,541)]{for sample in &samples[lo..hi]{assert_eq!(sample["position_source"],samples[lo]["position_source"]);}}
            assert!(!c["event_trial_exhausted"].as_bool().unwrap());
        }
        //The sixth shape also passes every full-duration motion gate.
        assert!(report["cases"][5]["passed"].as_bool().unwrap());
    }
    #[test]
    fn independent_landing_holds_keep_impulses_and_stop_prediction(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impact-hold.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);let mut count=0;
        for (c,p) in report["cases"].as_array().unwrap().iter().zip(data["profiles"].as_array().unwrap()){
            assert!(c["passed"].as_bool().unwrap(),"{} {}",c["model"],c["max_errors"]);
            let holds=c["event_trial"].as_array().unwrap().iter().flat_map(|t|t["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="impact_prediction_hold").map(move|e|(&t["step"],e))).collect::<Vec<_>>();let native=p["holds"].as_array().unwrap();assert_eq!(holds.len(),native.len());
            for ((step,h),n) in holds.into_iter().zip(native){count+=1;assert_eq!(*step,n["step"]);assert_eq!(h["impact_count"],n["count"]);assert_eq!(h["limit"],n["limit"]);
                for (source,expected,tolerance) in [("position","position",1e-5),("rotation","rotation",1e-5),("queued_linear","queued_linear",1e-5),("queued_angular","queued_angular",1e-4)]{
                    assert!(h[source].as_array().unwrap().iter().zip(n[expected].as_array().unwrap()).all(|(a,b)|(a.as_f64().unwrap()-b.as_f64().unwrap()).abs()<tolerance),"{} {}",c["model"],source);
                }
            }
        }assert_eq!(count,4);
    }
    #[test]
    fn late_collision_executes_after_public_sample_and_before_next_psi(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-tick-boundary.json")).unwrap();
        let report=compare_owned_phy_motion(&data,true,true);assert!(report["passed"].as_bool().unwrap(),"{report}");let mut calls=0;
        for (i,c) in report["cases"].as_array().unwrap().iter().enumerate(){
            for (s,time) in c["samples"].as_array().unwrap().iter().zip(data["profiles"][i]["public_times"].as_array().unwrap()){assert!((s["time"].as_f64().unwrap()-time.as_f64().unwrap()).abs()<1e-12);}
            let ticks=c["event_trial"].as_array().unwrap();let mut observed=vec![];
            for tick in ticks{let step=tick["step"].as_u64().unwrap();let current=(step+1) as f64*(1./60.0f32) as f64;let mut epoch=current;let mut count=0;
                for e in tick["events"].as_array().unwrap(){
                    if e["kind"]=="retained_interval_resume"{epoch=e["epoch"].as_f64().unwrap();count=e["impact_count"].as_u64().unwrap();}
                    if e["kind"]=="retained_force_prepare"{epoch=current;count=0;}
                    if (120..=145).contains(&step)&&["impact","retry"].contains(&e["kind"].as_str().unwrap_or("")){
                        observed.push((step,epoch+e["time"].as_f64().unwrap(),count,e));count+=1;
                    }
                }
            }
            let native=data["profiles"][i]["calls"].as_array().unwrap();assert_eq!(observed.len(),native.len());calls+=native.len();
            for ((step,time,count,e),n) in observed.into_iter().zip(native){assert_eq!(step,n["step"].as_u64().unwrap());assert!((time-n["time"].as_f64().unwrap()).abs()<1e-6);assert_eq!(count,n["before_count"].as_u64().unwrap());assert_eq!(count+1,n["after_count"].as_u64().unwrap());assert_eq!(e["system_pushes"],n["pushes"]);
                for (a,b,tolerance) in [("arm","arm",1e-5),("velocity_before","velocity_before",1e-5),("angular_before","angular_before",1e-4),("velocity_after","velocity_after",1e-5),("angular_after","angular_after",1e-4)]{assert!(e[a].as_array().unwrap().iter().zip(n[b].as_array().unwrap()).all(|(v,w)|(v.as_f64().unwrap()-w.as_f64().unwrap()).abs()<tolerance),"{} step{} {}",c["model"],step,a);}
            }
            for (sample,expected) in c["samples"].as_array().unwrap().iter().skip(1).zip(data["cases"][i]["samples"].as_array().unwrap().iter().skip(1)){assert_eq!(sample["asleep"].as_bool().unwrap(),expected["core_flags"].as_u64().unwrap()>>8==8);}
        }assert_eq!(calls,54);
        let ticks=report["cases"][3]["event_trial"].as_array().unwrap();let prior=ticks.iter().find(|t|t["step"]==133).unwrap()["events"].as_array().unwrap();
        assert!(!prior.iter().any(|e|["impact","retry"].contains(&e["kind"].as_str().unwrap_or(""))));
        let deferred=prior.iter().find(|e|e["kind"]=="retained_interval_defer").unwrap();let boundary=&data["boundary"];
        assert!((deferred["public_time"].as_f64().unwrap()-boundary["public_time"].as_f64().unwrap()).abs()<1e-12);assert!((deferred["event_time"].as_f64().unwrap()-boundary["event_time"].as_f64().unwrap()).abs()<1e-6);
        let next=ticks.iter().find(|t|t["step"]==134).unwrap()["events"].as_array().unwrap();assert_eq!(next[0]["kind"],"retained_interval_resume");assert!((next[0]["next_psi"].as_f64().unwrap()-boundary["next_psi"].as_f64().unwrap()).abs()<1e-12);
        assert!(next.iter().position(|e|e["kind"]=="impact").unwrap()<next.iter().position(|e|e["kind"]=="retained_force_prepare").unwrap());
    }
    #[test]
    fn native_core_ownership_queues_and_single_commit_match(){
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2").exists(){return;}
        let d:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-raw-queue.json")).unwrap();let report=compare_owned_phy_motion(&d,true,true);let mut samples=0;let mut commits=0;
        // These controlled histories now require every sampled raw/pending
        // velocity component to match native float bits. Arbitrary contacts
        // and the trajectory/public-position contract remain separate checks.
        for (c,n) in report["cases"].as_array().unwrap().iter().zip(d["cases"].as_array().unwrap()){
            for (s,e) in c["samples"].as_array().unwrap().iter().zip(n["samples"].as_array().unwrap()){
                let a=&s["core_motion"];let b=&e["core_motion"];samples+=1;
                for k in ["core_flags","impact_count","unit_flags"]{assert_eq!(a[k],b[k],"{} step{} {k}",c["model"],s["step"]);}
                for k in ["raw_linear_ivp","core_angular_local","pending_linear_ivp","pending_angular_local"]{
                    let actual=a[k].as_array().unwrap();let expected=b[k].as_array().unwrap();assert_eq!(actual.len(),3);assert_eq!(expected.len(),3);
                    for axis in 0..3{assert_eq!((actual[axis].as_f64().unwrap() as f32).to_bits(),(expected[axis].as_f64().unwrap() as f32).to_bits(),"{} step{} {k}[{axis}]",c["model"],s["step"]);}
                }
            }
            let ticks=c["event_trial"].as_array().unwrap();
            for tick in ticks{for hold in tick["events"].as_array().unwrap().iter().filter(|e|e["kind"]=="impact_prediction_hold"){
                let next=ticks.iter().find(|t|t["step"].as_u64().unwrap()==tick["step"].as_u64().unwrap()+1).unwrap();let force=next["events"].as_array().unwrap().iter().find(|e|e["kind"]=="retained_force_prepare").unwrap();
                for (queued,raw) in [("queued_linear","linear_before"),("queued_angular","angular_before")]{assert_eq!(hold[queued],force[raw]);}commits+=1;
            }}
            let input=ticks.iter().flat_map(|t|t["events"].as_array().unwrap()).find(|e|e["kind"]=="retained_external_impulse").unwrap();let expected=&n["wake_event"];
            for phase in ["before","after"]{for (a,b) in [("core_flags","core_flags"),("unit_flags","unit_flags")]{assert_eq!(input[phase][a],expected[phase][b]);}for (a,b) in [("raw_linear_ivp","raw_linear"),("pending_linear_ivp","pending_linear")]{assert!(input[phase][a].as_array().unwrap().iter().zip(expected[phase][b].as_array().unwrap()).all(|(x,y)|(x.as_f64().unwrap()-y.as_f64().unwrap()).abs()<1e-6));}}
        }assert_eq!(samples,3246);assert_eq!(commits,4);
    }
    fn compare_owned_phy_motion(data:&serde_json::Value,trial:bool,persistent_trial:bool)->serde_json::Value{
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        assert!(root.exists());
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let v=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
        let source=|a:&serde_json::Value|l4d2r_math::Vec3::from_array(v(a).to_array());
        let render=|a:&serde_json::Value|Vec3::from_array(crate::binary::source_position(v(a).to_array()));
        assert!(!persistent_trial||trial,"persistent trial requires the independent CCD trial");
        let mut results=vec![];let mut passed=true;
        for case in data["cases"].as_array().unwrap() {
            let model=case["model"].as_str().unwrap().replace(".phy",".mdl");
            let phy=assets.read(case["model"].as_str().unwrap()).unwrap();
            let surface_offset=i32::from_le_bytes(phy[0..4].try_into().unwrap()) as usize+32;
            let radius=f32::from_le_bytes(phy[surface_offset+24..surface_offset+28].try_into().unwrap());
            let free=case["free"].as_bool().unwrap();
            let (mut world,_,bounds)=pair_world();world.ents.truncate(1);
            world.ents[0].me.mdl=Some(model.clone());
            let samples=case["samples"].as_array().unwrap();
            world.ents[0].me.origin=source(if persistent_trial{case.get("origin_source").unwrap_or(&samples[0]["origin_source"])}else{&samples[0]["origin_source"]});
            world.ents[0].me.angles=source(if persistent_trial{case.get("angles_source").unwrap_or(&samples[0]["angles_source"])}else{&samples[0]["angles_source"]});
            let mut props=Props::load(&world,&mut assets);
            let body=props.0.get_mut(&0).unwrap();
            body.surface.friction=case["body_friction"].as_f64().unwrap() as f32;
            body.surface.elasticity=case["body_elasticity"].as_f64().unwrap() as f32;
            let angular_input=if persistent_trial{case.get("initial_angular_source").unwrap_or(&samples[0]["angular_world_source_degrees"])}else{&samples[0]["angular_world_source_degrees"]};
            let linear_input=if persistent_trial{case.get("initial_linear_source").unwrap_or(&samples[0]["velocity_source"])}else{&samples[0]["velocity_source"]};
            let axis=Vec3::from_array(crate::binary::source_normal(v(angular_input).to_array()));
            let w=if persistent_trial{axis*(std::f32::consts::PI/180.)}else{axis*std::f32::consts::PI/180.};
            body.launch_debris(render(linear_input),w,false);
            let solids=l4d2_formats::phy::parse_phy(&phy).unwrap();
            let mut persistent=if persistent_trial{
                // Only the helper-created floor's serialized collision geometry
                // is loaded. Native contact rows, frames, histories and samples
                // never drive this diagnostic state after initialization.
                let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-tangent-pipeline.json")).unwrap();
                let profile=fixture["profiles"].as_array().unwrap().iter().find(|p|p["model"]==case["model"]).unwrap();
                let hex=profile["floor_collision_hex"].as_str().unwrap();
                let raw=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect::<Vec<_>>();
                let mut packet=(raw.len() as u32).to_le_bytes().to_vec();packet.extend(raw);
                let floor=l4d2_formats::phy::parse_collision(&packet,1).unwrap();
                Some(crate::articulated::PersistentTrial::new(solids.clone(),floor,&mut body.debris_motion.as_mut().unwrap().bodies[0],1./60.,case["inertia_scale"].as_f64().unwrap() as f32,!free))
            }else{None};
            if let Some(p)=persistent.as_mut(){p.initialize_queued_motion(&body.debris_motion.as_ref().unwrap().bodies[0]).unwrap();}
            let mut adjacency=vec![];
            for solid in solids {
                for convex in solid.convexes {for triangle in convex.triangles {
                    let ids=triangle.map(|v| {
                        let point=Vec3::from_array(crate::binary::source_position(v.to_array()));
                        body.debris_motion.as_ref().unwrap().bodies[0].vertices.iter().position(|v|v.distance_squared(point)<1e-10).unwrap()
                    });
                    for (a,z) in [(ids[0],ids[1]),(ids[1],ids[2]),(ids[2],ids[0])] {
                        let edge=(a.min(z),a.max(z));if !adjacency.contains(&edge) {adjacency.push(edge);}
                    }
                }}
            }
            let floor_radius=Vec3::new(512.*crate::collision::UNIT,512.*crate::collision::UNIT,8.*crate::collision::UNIT).length();
            let floor_inverse_diameter=0.5/floor_radius;

            // The synthetic floor has no texinfo/material lookup. Supply the
            // same pair coefficients as the native fixture, rather than using
            // the body's coefficients alone as the no-material fallback.
            if !free {
                let floor=crate::physics_data::Surface {
                    friction:case["floor_friction"].as_f64().unwrap() as f32,
                    elasticity:case["floor_elasticity"].as_f64().unwrap() as f32,density:2400.,
                };
                let pair=body.surface.contact_with(floor);
                let rig=body.debris_motion.as_mut().unwrap();rig.friction=pair.0;rig.elasticity=pair.1;
            }
            let collision=if free {crate::collision::Collision::default()}else{crate::collision::Collision::test_floor()};
            let first_contact=case["collision_trace"].as_array().unwrap().iter()
                .filter_map(|s|s["step"].as_u64()).min().map(|x|x as usize);
            let airborne_end=first_contact.map_or(60,|s|s.saturating_sub(1));
            let mut cursor=0usize;let mut max=[0f32;4];let mut air=[0f32;4];let mut errors=vec![];let mut audits=vec![];let mut event_trials=vec![];
            for step in 0..=540 {
                if cursor<samples.len() && samples[cursor]["step"].as_u64().unwrap() as usize==step {
                    let expected=&samples[cursor];let body=&props.0[&0];
                    let position=world.ents[0].me.origin;
                    let rotation=crate::l4d2r_cinematic::transform(world.world_matrix(0)).rotation;
                    let expected_rotation=crate::l4d2r_cinematic::transform(l4d2r_game_core::mapents::entity_matrix(
                        l4d2r_math::Vec3::ZERO,source(&expected["angles_source"]))).rotation;
                    let expected_w=Vec3::from_array(crate::binary::source_normal(v(&expected["angular_world_source_degrees"]).to_array()))*std::f32::consts::PI/180.;
                    let e=[position.distance(source(&expected["origin_source"])),rotation.angle_between(expected_rotation).to_degrees(),
                        body.velocity.distance(render(&expected["velocity_source"]))/crate::collision::UNIT,
                        body.angular.distance(expected_w).to_degrees()];
                    for i in 0..4 {assert!(e[i].is_finite());max[i]=max[i].max(e[i]);if step<=airborne_end {air[i]=air[i].max(e[i]);}}
                    let mut sample=serde_json::json!({"step":step,"errors":e,"position_source":position.to_array(),"native_position_source":expected["origin_source"],"asleep":body.asleep});
                    if let Some(p)=persistent.as_ref(){sample["time"]=serde_json::json!(p.public_time(1./60.));sample["core_motion"]=p.core_motion();}
                    errors.push(sample);
                    cursor+=1;
                }
                if step<540 {
                    crate::articulated::WORLD_IMPACT_AUDIT.with(|a|*a.borrow_mut()=if step<60 {Some(vec![])}else{None});
                    if free || trial {
                        // The articulated API takes an explicit acceleration;
                        // ordinary prop gravity stays enabled in normal play.
                        let matrix=crate::l4d2r_cinematic::transform(world.world_matrix(0));
                        let body=props.0.get_mut(&0).unwrap();let rigid=body.debris_motion.as_mut().unwrap();
                        if step==0 {rigid.bodies[0].position=matrix.transform_point(rigid.bodies[0].center);rigid.bodies[0].rotation=matrix.rotation;}
                        if trial {
                            let mut external_impulse=None;
                            if case["wake_event"]["before_step"].as_u64()==Some((step+1) as u64){
                                let i=&case["wake_event"]["impulse_source"];
                                let p=persistent.as_mut().expect("wake needs persistent trial");let before=p.core_motion();let time=p.public_time(1./60.);
                                p.apply_central_impulse(Vec3::new(i[0].as_f64().unwrap() as f32,i[1].as_f64().unwrap() as f32,i[2].as_f64().unwrap() as f32),rigid.bodies[0].inverse_mass).unwrap();
                                external_impulse=Some(serde_json::json!({"kind":"retained_external_impulse","time":time,"before":before,"after":p.core_motion()}));
                            }
                            let mut events=rigid.step_event_trial(1./60.,&collision,if free {Vec3::ZERO}else{Vec3::NEG_Y*800.*crate::collision::UNIT},radius,&adjacency,floor_inverse_diameter,persistent.as_mut());
                            if let Some(input)=external_impulse{events.insert(0,input);}
                            if !events.is_empty(){event_trials.push(serde_json::json!({"step":step+1,"events":events}));}
                        } else {rigid.step_acceleration(1./60.,&collision,true,Vec3::ZERO);}
                        let b=&rigid.bodies[0];body.velocity=b.velocity;body.angular=b.angular;
                        if persistent_trial{body.asleep=persistent.as_ref().is_some_and(|p|p.is_sleeping());}
                        let origin=b.position-b.rotation*b.center;let (yaw,pitch,roll)=b.rotation.to_euler(EulerRot::YZX);
                        world.ents[0].me.origin=l4d2r_math::Vec3::new(origin.x,-origin.z,origin.y)/crate::collision::UNIT;
                        world.ents[0].me.angles=l4d2r_math::Vec3::new(-pitch,yaw,roll)*(180./std::f32::consts::PI);
                    }else{props.tick(&mut world,&bounds,&collision,1./60.);}
                    crate::articulated::WORLD_IMPACT_AUDIT.with(|a| {
                        if let Some(events)=a.borrow_mut().take() {
                            if !events.is_empty() {audits.push(serde_json::json!({"step":step+1,"contacts":events}));}
                        }
                    });
                }
            }
            let trial_exhausted=event_trials.iter().any(|s|s["events"].as_array().unwrap().iter()
                .any(|e|e["exhausted"].as_bool()==Some(true)||e["retry_exhausted"].as_bool()==Some(true)));
            let case_passed=max.iter().all(|x|*x<0.1)&&!trial_exhausted;passed&=case_passed;
            println!("{model} free={free} first_contact={first_contact:?} air={air:?} all={max:?}");
            results.push(serde_json::json!({"model":model,"free":free,"passed":case_passed,"first_contact":first_contact,"airborne_end":airborne_end,"airborne_errors":air,"max_errors":max,"samples":errors,"world_impact_audit":audits,"event_trial":event_trials,"event_trial_exhausted":trial_exhausted}));
        }
        let mut report=serde_json::json!({"passed":passed,"gates":[0.1,0.1,0.1,0.1],"metric_order":["position_source_units","rotation_degrees","velocity_source_units_s","angular_world_degrees_s"],"cases":results});
        if persistent_trial{report["persistent_trial"]=serde_json::json!({"unseeded_after_initialization":true,"floor_input":"serialized controlled floor geometry only","limitations":["CCD admission and directed feature selection remain approximate","impact retry topology remains approximate","general coupled-unit graphs, shared poll ownership, callbacks and world scheduling remain incomplete","not enabled in gameplay"]});}
        report
    }
    #[test]
    fn debris_publication_preserves_physical_state_but_authored_moves_reset_it() {
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let (mut world,_,bounds)=pair_world();world.ents.truncate(1);
        world.ents[0].me.mdl=Some("models/gibs/glass_shard01.mdl".into());
        world.ents[0].me.origin=l4d2r_math::Vec3::new(4.,-3.,100.);
        world.ents[0].me.angles=l4d2r_math::Vec3::new(27.,13.,-33.);
        let mut props=Props::load(&world,&mut assets);
        let velocity=Vec3::new(0.4,0.3,-0.2);let angular=Vec3::new(1.3,-0.8,2.1);
        props.0.get_mut(&0).unwrap().launch_debris(velocity,angular,false);
        let mut reference=props.0[&0].debris_motion.clone().unwrap();
        let transform=crate::l4d2r_cinematic::transform(world.world_matrix(0));
        reference.bodies[0].position=transform.transform_point(reference.bodies[0].center);
        reference.bodies[0].rotation=transform.rotation;
        let empty=crate::collision::Collision::default();
        for step in 0..31 {
            if step==30 {
                // An actual entity move/rotation must override the old endpoint.
                world.ents[0].me.origin+=l4d2r_math::Vec3::new(80.,40.,10.);
                world.ents[0].me.angles.y+=15.;
                let t=crate::l4d2r_cinematic::transform(world.world_matrix(0));
                reference.bodies[0].position=t.transform_point(reference.bodies[0].center);
                reference.bodies[0].rotation=t.rotation;reference.bodies[0].pose_cache=None;
            }
            props.tick(&mut world,&bounds,&empty,1./60.);
            reference.step(1./60.,&empty);
            let actual=&props.0[&0].debris_motion.as_ref().unwrap().bodies[0];
            let expected=&reference.bodies[0];
            assert_eq!(actual.position,expected.position,"step{step}: publication altered simulation");
            assert_eq!(actual.rotation,expected.rotation,"step{step}: Euler round trip altered simulation");
            assert_eq!(actual.velocity,expected.velocity);assert_eq!(actual.angular,expected.angular);
        }
    }
    #[test]
    fn free_rotation_preserves_the_mass_center_ballistic_path() {
        for parented in [false,true] {
            let (mut world,mut props,bounds)=pair_world();
            props.0.remove(&1);
            if parented {
                world.ents[0].parent=Some(1);
                world.ents[1].me.origin=l4d2r_math::Vec3::new(30.,12.,40.);
                world.ents[1].me.angles=l4d2r_math::Vec3::new(15.,-38.,22.);
            }
            world.ents[0].me.origin=l4d2r_math::Vec3::new(12.,-35.,300.);
            world.ents[0].me.angles=l4d2r_math::Vec3::new(25.,73.,-19.);
            let body=props.0.get_mut(&0).unwrap();
            body.center=Vec3::new(0.07,-0.02,0.04);
            body.velocity=Vec3::new(0.3,0.6,-0.2);body.angular=Vec3::new(1.,2.,-0.5);
            let local_center=body.center;let mut velocity=body.velocity;
            let mut position=crate::l4d2r_cinematic::transform(world.world_matrix(0)).transform_point(local_center);
            for _ in 0..60 {
                let dt=1f32/60.;velocity*=1.-0.2*dt;velocity.y-=800.*crate::collision::UNIT*dt;position+=velocity*dt;
                props.tick(&mut world,&bounds,&crate::collision::Collision::default(),dt);
                let actual=crate::l4d2r_cinematic::transform(world.world_matrix(0)).transform_point(local_center);
                assert!(actual.distance(position)<0.00002,"COM drift: actual{actual:?} ballistic{position:?}");
            }
        }
    }
    #[test]
    fn external_prop_force_matches_native_next_damping_pass() {
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-damping.json")).unwrap();
        let case=data["cases"].as_array().unwrap().iter()
            .find(|c|c["queued_force_center"]==true && c["gravity_source_z"].as_f64().unwrap()==-800.).unwrap();
        let v=|a:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|a[i].as_f64().unwrap() as f32));
        let (mut world,mut props,bounds)=pair_world();props.0.remove(&1);
        world.ents[0].me.origin=l4d2r_math::Vec3::new(0.,0.,300.);
        let body=props.0.get_mut(&0).unwrap();
        body.velocity=Vec3::from_array(crate::binary::source_position(v(&case["base_linear_source"]).to_array()));
        let w=v(&case["samples"][0]["angular_core_ivp"]);body.angular=Vec3::new(w.x,-w.y,-w.z);
        let force=Vec3::from_array(crate::binary::source_position([90.,-30.,140.]));
        props.impulse(0,force.normalize(),force.length(),Vec3::ZERO);
        for sample in case["samples"].as_array().unwrap().iter().skip(1) {
            props.tick(&mut world,&bounds,&crate::collision::Collision::default(),1./60.);
            let body=&props.0[&0];
            let expected=Vec3::from_array(crate::binary::source_position(v(&sample["linear_source"]).to_array()));
            assert!(body.velocity.distance(expected)/crate::collision::UNIT<0.003,"queued native force step{}",sample["step"]);
            let expected=v(&sample["angular_core_ivp"]);
            assert!(body.angular.distance(Vec3::new(expected.x,-expected.y,-expected.z)).to_degrees()<0.03);
        }
    }
    #[test]
    fn owned_prop_offset_impulses_match_native_world_motion() {
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-prop-impulses.json")).unwrap();
        let v=|value:&serde_json::Value|Vec3::from_array(std::array::from_fn(|i|value[i].as_f64().unwrap() as f32));
        for case in data["cases"].as_array().unwrap() {
            let (mut world,_,_)=pair_world();world.ents.truncate(1);
            let model=case["model"].as_str().unwrap().replace(".phy",".mdl");
            world.ents[0].me.mdl=Some(model.clone());
            world.ents[0].me.origin=l4d2r_math::Vec3::ZERO;
            world.ents[0].me.angles=l4d2r_math::Vec3::from_array(v(&case["angles_source"]).to_array());
            let mut props=Props::load(&world,&mut assets);
            assert_eq!(props.0[&0].mass,case["mass"].as_f64().unwrap() as f32);
            let native=&case["solids"][0]["impulse"];
            let point=Vec3::from_array(crate::binary::source_position(v(&native["point_source"]).to_array()));
            let force=Vec3::from_array(crate::binary::source_position(v(&native["force_source"]).to_array()));
            let center=props.center_world(0,&world).unwrap();
            props.impulse(0,force.normalize(),force.length(),point-center);
            let actual=&props.0[&0];
            let expected_v=Vec3::from_array(crate::binary::source_position(v(&native["velocity_source"]).to_array()));
            let expected_w=Vec3::from_array(crate::binary::source_normal(v(&native["angular_world_source_degrees"]).to_array()))*std::f32::consts::PI/180.;
            assert!(actual.velocity.distance(expected_v)/crate::collision::UNIT<0.003,"{model} linear impulse");
            assert!(actual.angular.distance(expected_w).to_degrees()<0.03,"{model} angular impulse: actual{:?} expected{expected_w:?}",actual.angular);
        }
    }
    #[test]
    fn streets_tank_punched_car_moves_once_its_clip_is_killed() {
        // c1m2's parked sedan: OnHitByTank kills its vphysics clip, and the
        // punch launches it on its hulls (its box overlaps a pole behind it).
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        collision.add_terrain(&crate::bsp::load_file(&path, root).unwrap());
        let id = logic.world.find("AutoInstance1-car_physics1")[0];
        for _ in 0..30 {
            logic.world.tick(1. / 30.);
            if logic.physics.tick(&mut logic.world, &logic.prop_bounds, &collision, 1. / 30.) { collision.sync_entities(&logic); }
        }
        let start = logic.world.ents[id].me.origin;
        let mass = logic.physics.0[&id].mass;
        println!("car body: mass {mass} asleep {} start {start:?}", logic.physics.0[&id].asleep);
        logic.world.fire_output(id, "OnHitByTank", None);
        let impulse = crate::special_infected::tank::prop_impulse(mass);
        logic.physics.impulse(id, Vec3::Z, impulse, Vec3::ZERO);
        logic.physics.launch_punched(id);
        println!("after impulse v={:?}", logic.physics.0[&id].velocity);
        logic.world.tick(1. / 30.);
        let clips: Vec<_> = logic.world.ents.iter().enumerate().filter(|(_, e)| e.me.targetname.contains("cola_car1_clip")).map(|(i, e)| (i, e.alive, e.me.classname.clone())).collect();
        println!("clips after I/O: {clips:?}");
        for step in 0..30 {
            logic.world.tick(1. / 30.);
            collision.sync_entities(&logic);
            if logic.physics.tick(&mut logic.world, &logic.prop_bounds, &collision, 1. / 30.) { collision.sync_entities(&logic); }
            if step % 5 == 0 { println!("step {step}: {:?} v={:?}", logic.world.ents[id].me.origin, logic.physics.0[&id].velocity); }
        }
        let moved = (logic.world.ents[id].me.origin - start).length();
        assert!(moved > 50., "punched car moved only {moved} units");
    }
    #[test]
    fn streets_native_cars_preserve_grounded_map_placement() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        collision.add_terrain(&crate::bsp::load_file(&path, root).unwrap());
        let id = logic.world.find("InstanceAuto6-car_physics")[0];
        let original = logic.world.ents[id].me.origin;
        let glass = logic.world.ents.iter().position(|e| e.parent == Some(id)).unwrap();
        let glass_start = logic.world.world_matrix(glass).w_axis.truncate();
        assert!(logic.physics.0[&id].mass > 1000., "zero massScale must retain native car mass");
        assert!(logic.prop_bounds.contains_key(&glass), "attached glass needs a live collider, not a stale map OBB");
        for _ in 0..60 {
            let changed = logic.physics.tick(&mut logic.world, &logic.prop_bounds, &collision, 1. / 30.);
            if changed { collision.sync_entities(&logic); }
        }
        let position = logic.world.ents[id].me.origin;
        println!("Streets tunnel car: {original:?} -> {position:?}, mass={}kg", logic.physics.0[&id].mass);
        assert!(position.z < original.z - 8., "car remains suspended by its attached glass");
        assert!((66. ..69.).contains(&position.z), "car must settle on road at z64 with native PHY bottom, got {position:?}");
        let glass_now = logic.world.world_matrix(glass).w_axis.truncate();
        assert!((glass_now - glass_start - (position-original)).length() < 0.01,
            "attached glass must follow the car instead of hanging in the original position");
        assert!(logic.world.ents.iter().all(|e| e.me.origin.is_finite()));
    }

    #[test]
    fn sleeping_contact_islands_only_run_when_connected_to_an_awake_body() {
        let mut pairs = vec![(2, 3), (1, 2), (0, 1), (4, 5), (3, 6), (6, 7), (7, 8)];
        let mut active = std::collections::HashSet::from([0]);
        retain_active_pairs(&mut pairs, &mut active, |id| id == 6);
        assert_eq!(pairs, vec![(2, 3), (1, 2), (0, 1), (3, 6)]);
        assert_eq!(active, std::collections::HashSet::from([0, 1, 2, 3]));
    }
    fn pair_world() -> (
        l4d2r_game_core::entities::EntityWorld,
        Props,
        HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
    ) {
        let mut data = vec![0; 1036];
        data[..4].copy_from_slice(b"VBSP");
        data[4..8].copy_from_slice(&21u32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(data).unwrap());
        let entries = (0..2)
            .map(|i| l4d2r_game_core::mapents::MapEnt {
                index: i,
                classname: "prop_physics".into(),
                targetname: format!("stack_{i}"),
                origin: l4d2r_math::Vec3::new(0., 0., if i == 0 { 4. } else { 60. }),
                angles: l4d2r_math::Vec3::ZERO,
                mdl: None,
                brush: None,
                ent: l4d2_formats::bsp::Entity(vec![]),
            })
            .collect();
        let world = l4d2r_game_core::entities::EntityWorld::new(bsp, entries, 1);
        let bodies = (0..2)
            .map(|i| {
                (
                    i,
                    Body {
                        velocity: Vec3::ZERO,
                        angular: Vec3::ZERO,
                        asleep: false,
                        disabled: false,
                        mass: 10.,
                        supported_sleep: false,
                        resting_on: None,
                        support_clock: 0.,
                        progress: (Vec3::ZERO, 0., false),
                        surface: default(),
                        on_floor: false,
                        inverse_inertia: crate::rigid_contact::inverse_box_inertia(
                            10.,
                            Vec3::splat(4. * crate::collision::UNIT),
                            1.,
                        ),
                        center: Vec3::ZERO, damping:0.2, angular_damping:0.5, 
                    rotation: Quat::IDENTITY,
                        failed_recovery: None, debris_motion: None, debris_pose:None,
                        hulls: Vec::new(),
                    },
                )
            })
            .collect();
        let bounds = (0..2)
            .map(|i| {
                (
                    i,
                    (l4d2r_math::Vec3::splat(-4.), l4d2r_math::Vec3::splat(4.)),
                )
            })
            .collect();
        (world, Props(bodies, 0., Vec::new()), bounds)
    }
    #[test]
    fn boxes_stack_and_transfer_horizontal_impulses_in_both_directions() {
        let (mut world, mut props, bounds) = pair_world();
        let floor = crate::collision::Collision::test_floor();
        for _ in 0..360 {
            props.tick(&mut world, &bounds, &floor, 1. / 60.);
        }
        assert!(
            (world.ents[0].me.origin.z - 4.).abs() < 0.3,
            "bottom sank: {:?}",
            world.ents[0].me.origin
        );
        assert!(
            (world.ents[1].me.origin.z - 12.).abs() < 0.5,
            "top did not rest on bottom: {:?}",
            world.ents[1].me.origin
        );
        // A floor can absorb tangential friction. Test reciprocal momentum
        // separately in freeflight instead of requiring a grounded block to move.
        let (mut world, mut props, bounds) = pair_world();
        world.ents[0].me.origin = l4d2r_math::Vec3::new(0., 0., 100.);
        world.ents[1].me.origin = l4d2r_math::Vec3::new(9., 0., 100.);
        props.impulse(1, Vec3::NEG_X, 10., Vec3::ZERO);
        for _ in 0..10 {
            props.tick(
                &mut world,
                &bounds,
                &crate::collision::Collision::default(),
                1. / 60.,
            );
        }
        assert!(
            world.ents[0].me.origin.x < -0.1,
            "stack contact did not push the supporting body"
        );
    }
    #[test]
    fn contact_uses_material_friction_instead_of_a_fixed_velocity_multiplier() {
        let simulate = |friction| {
            let (mut world, mut p, b) = pair_world();
            world.ents[1].alive = false;
            p.0.get_mut(&0).unwrap().surface.friction = friction;
            p.impulse(0, Vec3::X, 20., Vec3::ZERO);
            for _ in 0..30 {
                p.tick(
                    &mut world,
                    &b,
                    &crate::collision::Collision::test_floor(),
                    1. / 60.,
                );
            }
            world.ents[0].me.origin.x
        };
        assert!(
            simulate(0.02) > simulate(0.8) * 2.,
            "low-friction surface must slide farther"
        );
    }
    #[test]
    fn dropped_prop_settles_and_a_shot_wakes_sleeping_clutter() {
        let mut data = vec![0; 1036];
        data[..4].copy_from_slice(b"VBSP");
        data[4..8].copy_from_slice(&21u32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(data).unwrap());
        let me = l4d2r_game_core::mapents::MapEnt {
            index: 0,
            classname: "prop_physics".into(),
            targetname: String::new(),
            origin: l4d2r_math::Vec3::new(0., 0., 100.),
            angles: l4d2r_math::Vec3::ZERO,
            mdl: None,
            brush: None,
            ent: l4d2_formats::bsp::Entity(vec![]),
        };
        let mut world = l4d2r_game_core::entities::EntityWorld::new(bsp, vec![me], 1);
        let bounds = HashMap::from([(
            0,
            (l4d2r_math::Vec3::splat(-4.), l4d2r_math::Vec3::splat(4.)),
        )]);
        let mut physics = Props(
            HashMap::from([(
                0,
                Body {
                    velocity: Vec3::ZERO,
                    angular: Vec3::ZERO,
                    asleep: false,
                    disabled: false,
                    mass: 10.,
                    supported_sleep: false,
                    resting_on: None,
                    support_clock: 0.,
                    progress: (Vec3::ZERO, 0., false),
                    surface: default(),
                    on_floor: false,
                    inverse_inertia: crate::rigid_contact::inverse_box_inertia(
                        10.,
                        Vec3::splat(4. * crate::collision::UNIT),
                        1.,
                    ),
                    center: Vec3::ZERO, damping:0.2, angular_damping:0.5, 
                    rotation: Quat::IDENTITY,
                    failed_recovery: None, debris_motion: None, debris_pose:None,
                    hulls: Vec::new(),
                },
            )]),
            0.,
            Vec::new(),
        );
        let collision = crate::collision::Collision::test_floor();
        for _ in 0..300 {
            physics.tick(&mut world, &bounds, &collision, 1. / 60.);
        }
        assert!(world.ents[0].me.origin.z >= 3.9 && world.ents[0].me.origin.z < 5.);
        assert!(physics.0[&0].asleep);
        let old = world.ents[0].me.origin;
        physics.impulse(0, Vec3::new(1., 1., 0.).normalize(), 30., Vec3::ZERO);
        assert!(!physics.0[&0].asleep);
        for _ in 0..20 {
            physics.tick(&mut world, &bounds, &collision, 1. / 60.);
        }
        assert!(world.ents[0].me.origin.x > old.x + 1.);
        world.ents[0].me.origin = l4d2r_math::Vec3::new(0., 0., 1.);
        let body = physics.0.get_mut(&0).unwrap();
        body.asleep = false;
        body.velocity = Vec3::ZERO;
        let embedded_floor = crate::collision::Collision::test_solid_floor();
        for _ in 0..120 {
            physics.tick(&mut world, &bounds, &embedded_floor, 1. / 60.);
        }
        assert!(
            world.ents[0].me.origin.z > 3.9,
            "embedded clutter froze inside the floor"
        );
        let before = world.ents[0].me.origin.z;
        for _ in 0..60 {
            physics.tick(
                &mut world,
                &bounds,
                &crate::collision::Collision::default(),
                1. / 60.,
            );
        }
        assert!(
            !physics.0[&0].asleep && world.ents[0].me.origin.z < before - 10.,
            "removed support left the prop floating"
        );
        assert!(physics.input(0, "DisableMotion"));
        let frozen = world.ents[0].me.origin;
        physics.tick(
            &mut world,
            &bounds,
            &crate::collision::Collision::default(),
            0.1,
        );
        assert_eq!(world.ents[0].me.origin, frozen);
        world
            .unhandled
            .push((0, "EnableMotion".into(), String::new()));
        physics.tick(
            &mut world,
            &bounds,
            &crate::collision::Collision::default(),
            1. / 30.,
        );
        assert!(world.ents[0].me.origin.z < frozen.z);
        // Same solver steps regardless of render partition (no contact geometry).
        let template = world.ents[0].me.clone();
        let prototype = physics.0[&0].clone();
        let simulate = |hz: u32| {
            let mut me = template.clone();
            me.origin = l4d2r_math::Vec3::new(0., 0., 100.);
            let mut w = l4d2r_game_core::entities::EntityWorld::new(world.bsp.clone(), vec![me], 1);
            let mut b = prototype.clone();
            b.velocity = Vec3::new(0.4, 0., 0.);
            b.angular = Vec3::ZERO;
            b.asleep = false;
            b.disabled = false;
            let mut p = Props(HashMap::from([(0, b)]), 0., Vec::new());
            for _ in 0..hz * 2 {
                p.tick(
                    &mut w,
                    &bounds,
                    &crate::collision::Collision::default(),
                    1. / hz as f32,
                );
            }
            (w.ents[0].me.origin, p.0[&0].velocity)
        };
        let a = simulate(30);
        let b = simulate(144);
        assert!(
            a.0.distance(b.0) < 0.001 && a.1.distance(b.1) < 0.0001,
            "frame-dependent prop freeflight"
        );
    }
    #[test]
    fn failed_recovery_cache_matches_full_search_and_invalidates_when_static_obstacle_changes() {
        use l4d2r_game_core::trace::{CONTENTS_SOLID, Obb, Tracer};
        let (mut world, mut physics, bounds) = pair_world();
        physics.0.remove(&1);
        let (mut control_world, _, _) = pair_world();
        let mut control = Props(physics.0.clone(), 0., Vec::new());
        let mut tracer = Tracer::new(world.bsp.clone());
        let mut solid = Obb::new(
            l4d2r_math::Vec3::ZERO,
            l4d2r_math::Vec3::splat(100.),
            l4d2r_math::Mat3::IDENTITY,
            CONTENTS_SOLID,
        );
        solid.owner = 99;
        tracer.set_obbs(vec![solid]);
        let tracer = std::sync::Arc::new(std::sync::Mutex::new(tracer));
        let collision = crate::collision::Collision::test_native(tracer.clone());
        for tick in 0..30 {
            if tick == 20 {
                tracer.lock().unwrap().set_obb_owner_active(99, false);
            }
            control.0.get_mut(&0).unwrap().failed_recovery = None;
            physics.tick(&mut world, &bounds, &collision, 1. / 60.);
            control.tick(&mut control_world, &bounds, &collision, 1. / 60.);
            assert_eq!(world.ents[0].me.origin, control_world.ents[0].me.origin);
            assert_eq!(physics.0[&0].velocity, control.0[&0].velocity);
            if tick == 10 {
                assert!(physics.0[&0].failed_recovery.is_some());
            }
        }
        assert!(
            world.ents[0].me.origin.z < 0.,
            "removed obstacle must resume gravity"
        );
    }
}
fn retain_active_pairs(
    pairs: &mut Vec<(usize, usize)>,
    active: &mut std::collections::HashSet<usize>,
    disabled: impl Fn(usize) -> bool,
) {
    loop {
        let mut changed = false;
        for &(a, b) in pairs.iter() {
            if active.contains(&a) && !disabled(b) {
                changed |= active.insert(b);
            }
            if active.contains(&b) && !disabled(a) {
                changed |= active.insert(a);
            }
        }
        if !changed {
            break;
        }
    }
    pairs.retain(|(a, b)| active.contains(a) || active.contains(b));
}
impl Props {
    pub(crate) fn environment_surfaces(&self) -> &[crate::physics_data::Surface] {
        &self.2
    }
    pub fn input(&mut self, id: usize, input: &str) -> bool {
        let Some(body) = self.0.get_mut(&id) else {
            return false;
        };
        match input.to_ascii_lowercase().as_str() {
            "enablemotion" => {
                body.disabled = false;
                body.asleep = false;
                body.supported_sleep = false;
            }
            "disablemotion" => {
                body.disabled = true;
                body.velocity = Vec3::ZERO;
                body.angular = Vec3::ZERO;
            }
            "wake" => {
                if !body.disabled {
                    body.asleep = false;
                    body.supported_sleep = false;
                }
            }
            "sleep" => {
                body.asleep = true;
                body.supported_sleep = false;
                body.velocity = Vec3::ZERO;
                body.angular = Vec3::ZERO;
            }
            _ => return false,
        }
        true
    }
    pub fn load(
        world: &l4d2r_game_core::entities::EntityWorld,
        assets: &mut crate::source_assets::SourceAssets,
    ) -> Self {
        let surfaces = crate::physics_data::Surfaces::load(assets).unwrap_or_default();
        let mut profiles = HashMap::<String, crate::physics_data::Profile>::new();
        let mut hull_cache = HashMap::<String, Vec<crate::rigid_contact::Hull>>::new();
        let mut bodies = HashMap::new();
        for (id, e) in world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.me.classname.starts_with("prop_physics"))
        {
            let profile = e.me.mdl.as_ref().map(|name| {
                profiles.entry(name.clone()).or_insert_with(|| {
                    crate::physics_data::Profile::load(assets, name).unwrap_or_default()
                })
            });
            let solid = profile.as_ref().and_then(|p| p.solids.first());
            // Hammer writes zero for "use the model's authored mass".
            let scale = e.me.kv_f32("massscale").filter(|s| s.is_finite() && *s > 0.).unwrap_or(1.);
            let mass = (solid.map_or(10., |s| s.mass) * scale).clamp(0.1, 50000.);
            let surface = surfaces.get(solid.map_or("default", |s| s.surface.as_str()));
            let (fallback_center, half) = e.me.mdl.as_ref()
                .and_then(|name| assets.read(name).ok())
                .and_then(|bytes| l4d2_formats::mdl::read_header_bounds(&bytes))
                .map(|(lo, hi, _, _)| {
                    let lo=Vec3::from_array(crate::binary::source_position(lo));
                    let hi=Vec3::from_array(crate::binary::source_position(hi));
                    ((lo+hi)*0.5, ((hi-lo)*0.5).abs())
                }).unwrap_or((Vec3::ZERO, Vec3::splat(0.1)));
            let physical=profile.as_ref()
                .and_then(|p| p.geometry.get(solid.map_or(0,|s|s.index)))
                .and_then(|shape| crate::mass_properties::from_phy(shape,mass,solid.map_or(1.,|s|s.inertia),0.1));
            let center=physical.map_or(fallback_center,|p|p.0);
            let inverse_inertia=physical.map_or_else(
                || crate::rigid_contact::inverse_box_inertia(mass,half,solid.map_or(1.,|s|s.inertia)),
                |p|Vec3::new(p.1.x_axis.x,p.1.y_axis.y,p.1.z_axis.z));
            let hulls =
                e.me.mdl
                    .as_ref()
                    .map(|name| {
                        hull_cache
                            .entry(name.clone())
                            .or_insert_with(|| {
                                assets
                                    .read(&name.replace(".mdl", ".phy"))
                                    .ok()
                                    .and_then(|data| l4d2_formats::phy::parse_phy(&data).ok())
                                    .and_then(|solids| solids.into_iter().next())
                                    .map(|solid| {
                                        solid
                                            .convexes
                                            .iter()
                                            .map(crate::rigid_contact::Hull::native)
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            })
                            .clone()
                    })
                    .unwrap_or_default();
            bodies.insert(
                id,
                Body {
                    velocity: Vec3::ZERO,
                    angular: Vec3::ZERO,
                    asleep: e.me.spawnflags() & 1 != 0,
                    disabled: e.me.spawnflags() & 8 != 0,
                    mass,
                    supported_sleep: false,
                    resting_on: None,
                    support_clock: 0.,
                    progress: (Vec3::ZERO, 0., false),
                    surface,
                    on_floor: false,
                    inverse_inertia,
                    center,
                    damping:solid.map_or(0.,|s|s.damping),
                    angular_damping:solid.map_or(0.,|s|s.angular_damping),
                    
                    hulls,
                    rotation: crate::l4d2r_cinematic::transform(world.world_matrix(id)).rotation,
                    failed_recovery: None, debris_motion: None, debris_pose:None,
                },
            );
        }
        let names: Vec<String> = world
            .bsp
            .texinfo
            .iter()
            .map(|info| {
                world
                    .bsp
                    .texdata
                    .get(info.texdata as usize)
                    .and_then(|data| world.bsp.texnames.get(data.name_id as usize))
                    .map(|name| crate::material::surface_property(assets, name))
                    .unwrap_or_else(|| "concrete".into())
            })
            .collect();
        crate::bullet_penetration::set_world_materials(
            crate::bullet_penetration::game_materials(assets, &names),
        );
        let environment = names.iter().map(|name| surfaces.get(name)).collect();
        Self(bodies, 0., environment)
    }
    pub fn reset(&mut self, world: &l4d2r_game_core::entities::EntityWorld) {
        self.1 = 0.;
        for (id, body) in &mut self.0 {
            let e = &world.ents[*id];
            body.velocity = Vec3::ZERO;
            body.angular = Vec3::ZERO;
            body.asleep = e.me.spawnflags() & 1 != 0;
            body.disabled = e.me.spawnflags() & 8 != 0;
            body.supported_sleep = false;
            body.support_clock = 0.;
            body.failed_recovery = None;
            body.on_floor = false;
        }
    }
    pub fn center_world(&self,id:usize,world:&l4d2r_game_core::entities::EntityWorld)->Option<Vec3> {
        let body=self.0.get(&id)?;
        Some(crate::l4d2r_cinematic::transform(world.world_matrix(id)).transform_point(body.center))
    }
    /// A prop the Tank punched flies on its convex hulls (the debris
    /// solver) instead of the box sweep: a box around a parked car often
    /// overlaps the kerb or a pole beside it, which kept hittable cars stuck.
    pub fn launch_punched(&mut self, id: usize) {
        let Some(body) = self.0.get_mut(&id) else { return };
        if body.disabled || body.debris_motion.is_some() || body.hulls.is_empty() {
            return;
        }
        let (velocity, angular) = (body.velocity, body.angular);
        body.launch_debris(velocity, angular, false);
    }
    /// Offset is measured from the physical centre of mass, in renderer metres.
    pub fn impulse(&mut self, id: usize, direction: Vec3, impulse: f32, offset: Vec3) {
        let Some(body) = self.0.get_mut(&id) else {
            return;
        };
        if body.disabled {
            return;
        }
        body.asleep = false;
        body.supported_sleep = false;
        let inertia = crate::rigid_contact::world_inverse(body.rotation, body.inverse_inertia);
        let force=direction*impulse;
        let linear=force*(1./body.mass);let angular=inertia*offset.cross(force);
        // Native PSI preparation commits external changes before its damping
        // controller. Immediate readback and the next damping pass include them.
        body.velocity+=linear;body.angular+=angular;
    }
    pub fn tick(
        &mut self,
        world: &mut l4d2r_game_core::entities::EntityWorld,
        bounds: &HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
        collision: &crate::collision::Collision,
        dt: f32,
    ) -> bool {
        let pending = std::mem::take(&mut world.unhandled);
        let _timing = crate::performance::scope("map::prop_physics");
        for (id, input, param) in pending {
            if !self.input(id, &input) {
                world.unhandled.push((id, input, param));
            }
        }
        self.1 += dt.clamp(0., 0.1);
        let mut moved = false;
        while self.1 + 1e-6 >= 1. / 60. {
            self.1 = (self.1 - 1. / 60.).max(0.);
            moved |= self.step(world, bounds, collision, 1. / 60.);
        }
        moved
    }
    fn step(
        &mut self,
        world: &mut l4d2r_game_core::entities::EntityWorld,
        bounds: &HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
        collision: &crate::collision::Collision,
        dt: f32,
    ) -> bool {
        let mut owners = self.0.keys().filter(|id| bounds.contains_key(id)).copied()
            .collect::<std::collections::HashSet<_>>();
        // Attached glass/decorations are part of the moving physics hierarchy,
        // not environment obstacles for their own parent body.
        for (id, e) in world.ents.iter().enumerate() {
            let mut parent = e.parent;
            for _ in 0..world.ents.len() {
                let Some(p) = parent else { break };
                if owners.contains(&p) { owners.insert(id); break; }
                parent = world.ents.get(p).and_then(|e| e.parent);
            }
        }
        let mut moved = false;
        let debris_collision = collision.without_prop_owners(&owners);
        // Bodies that hold still: a prop resting on one of these stays asleep.
        let resting = self
            .0
            .iter()
            .filter(|(id, b)| (b.asleep || b.disabled) && world.ents[**id].alive)
            .map(|(id, _)| *id)
            .collect::<std::collections::HashSet<_>>();
        for (&id, body) in &mut self.0 {
            if body.disabled || !world.ents[id].alive {
                continue;
            }
            let Some(&(lo, hi)) = bounds.get(&id) else {
                continue;
            };
            let old_origin = world.ents[id].me.origin;
            let old_angles = world.ents[id].me.angles;
            let source_matrix=world.world_matrix(id);
            let matrix = crate::l4d2r_cinematic::transform(source_matrix);
            let local_center =
                Vec3::from_array(crate::binary::source_position(((lo + hi) * 0.5).to_array()));
            let half =
                Vec3::from_array(crate::binary::source_position(((hi - lo) * 0.5).to_array()))
                    .abs();
            let rotation = Mat3::from_quat(matrix.rotation);
            body.rotation = matrix.rotation;
            let extent = rotation.x_axis.abs() * half.x
                + rotation.y_axis.abs() * half.y
                + rotation.z_axis.abs() * half.z;
            let center = matrix.transform_point(local_center);
            if let Some(rigid) = body.debris_motion.as_mut() {
                if rigid.bodies[0].vertices.is_empty() { continue; }
                if body.debris_pose != Some(source_matrix) {
                    let b=&mut rigid.bodies[0];
                    b.position=matrix.transform_point(b.center);b.rotation=matrix.rotation;
                    b.pose_cache=None;
                    body.asleep=false;body.supported_sleep=false;body.support_clock=0.;
                    body.progress=(b.position,0.,false);
                }
                if body.asleep {
                    // Stalled debris stays asleep until something moves it
                    // (an authored pose change wakes it above).
                    if body.progress.2 { continue; }
                    body.support_clock += dt;
                    if body.support_clock < 0.1 { continue; }
                    body.support_clock = 0.;
                    if rigid.supported(&debris_collision) { continue; }
                    body.asleep = false;
                }
                let b = &mut rigid.bodies[0];
                b.velocity = body.velocity; b.angular = body.angular;
                let started = std::time::Instant::now();
                let (supported, speed) = rigid.step(dt, &debris_collision);
                // L4D2R_SLOW_PROPS: debug, debris props whose step takes > 5 ms.
                if std::env::var_os("L4D2R_SLOW_PROPS").is_some() && started.elapsed().as_secs_f32() > 0.0005 {
                    let e = &world.ents[id];
                    println!("L4D2R slow prop {id} {} {:?} at {:?}: {:.1} ms, {} vertices", e.me.classname, e.me.mdl, e.me.origin, started.elapsed().as_secs_f32() * 1000., rigid.bodies[0].vertices.len());
                }
                let b = &mut rigid.bodies[0];
                body.velocity = b.velocity; body.angular = b.angular; body.rotation = b.rotation;
                body.on_floor = supported;
                body.support_clock = if supported && speed < 0.0004 { body.support_clock + dt } else { 0. };
                if body.support_clock >= 0.5 {
                    body.asleep = true; body.supported_sleep = true;
                    body.velocity = Vec3::ZERO; body.angular = Vec3::ZERO;
                }
                // No progress for a second: sleep even without a clean
                // support certificate.
                if b.position.distance(body.progress.0) > 0.1 {
                    body.progress = (b.position, 0., false);
                } else {
                    body.progress.1 += dt;
                    if body.progress.1 >= 1. && !body.asleep {
                        body.asleep = true; body.progress.2 = true;
                        body.velocity = Vec3::ZERO; body.angular = Vec3::ZERO;
                    }
                }
                let origin = b.position - b.rotation * b.center;
                let (yaw, negative_pitch, roll) = b.rotation.to_euler(EulerRot::YZX);
                let e = &mut world.ents[id];
                e.me.origin = l4d2r_math::Vec3::new(origin.x, -origin.z, origin.y) / crate::collision::UNIT;
                e.me.angles = l4d2r_math::Vec3::new(-negative_pitch, yaw, roll) * (180. / std::f32::consts::PI);
                moved |= e.me.origin != old_origin || e.me.angles != old_angles;
                body.debris_pose=Some(world.world_matrix(id));
                continue;
            }
            if body.asleep {
                if !body.supported_sleep {
                    continue;
                }
                body.support_clock += dt;
                if body.support_clock < 0.1 {
                    continue;
                }
                body.support_clock = 0.;
                if body.resting_on.is_some_and(|s| resting.contains(&s)) {
                    continue;
                }
                // Asleep inside world geometry after a failed recovery: hold
                // until the environment changes (a brush toggled away).
                if body.failed_recovery.is_some_and(|(_, _, tag)| collision.environment_tag() == Some(tag)) {
                    continue;
                }
                let support = collision.prop_environment_trace(&owners, center, center - Vec3::Y * 0.025, extent);
                if support.fraction < 1. && support.normal.y > 0.6 {
                    continue;
                }
                body.asleep = false;
                body.supported_sleep = false;
                body.resting_on = None;
            }
            body.on_floor = false;
            crate::ivp_damping::apply(&mut body.velocity,&mut body.angular,dt,
                body.damping,Vec3::splat(body.angular_damping),1);
            body.velocity.y -= 800. * crate::collision::UNIT * dt;
            let travel = body.velocity * dt;
            let (hit, texinfo) =
                collision.prop_environment_contact(&owners, center, center + travel, extent);
            let environment = texinfo.and_then(|id| self.2.get(id)).cloned().unwrap_or(
                crate::physics_data::Surface {
                    friction: 0.8,
                    elasticity: 0.2,
                    density: 2400.,
                },
            );
            if hit.start_solid {
                // L4D2R_PROP_DEBUG=<entity id>: debug, report a stuck prop.
                if std::env::var("L4D2R_PROP_DEBUG").ok().and_then(|v| v.parse::<usize>().ok()) == Some(id) {
                    println!("prop {id} start solid: center {:?} extent {:?} normal {:?}",
                        center / crate::collision::UNIT, extent / crate::collision::UNIT, hit.normal);
                }
                let _timing = crate::performance::scope("map::prop_recovery");
                let tag = collision.environment_tag();
                let cached =
                    tag.is_some_and(|tag| body.failed_recovery == Some((center, extent, tag)));
                let recovered = if cached {
                    None
                } else {
                    collision.recover_prop_position(&owners, center, extent, 0.6)
                };
                if let Some(free) = recovered {
                    body.failed_recovery = None;
                    let correction = free - center;
                    world.ents[id].me.origin +=
                        l4d2r_math::Vec3::new(correction.x, -correction.z, correction.y)
                            / crate::collision::UNIT;
                    body.velocity.y = body.velocity.y.max(0.);
                    moved |= world.ents[id].me.origin != old_origin;
                } else {
                    body.failed_recovery = tag.map(|tag| (center, extent, tag));
                    // A prop placed into world geometry with no free spot
                    // nearby (c12m3's flour sack and cardboard box) holds
                    // still until something touches it or the environment
                    // changes, as in retail. Awake, it fell back into the
                    // wall every tick and kept its pair in the convex
                    // solver (~45 ms a frame).
                    body.asleep = true;
                    body.supported_sleep = true;
                    body.resting_on = None;
                    body.velocity = Vec3::ZERO;
                    body.angular = Vec3::ZERO;
                }
                continue;
            }
            let delta = travel * hit.fraction;
            let e = &mut world.ents[id];
            e.me.origin +=
                l4d2r_math::Vec3::new(delta.x, -delta.z, delta.y) / crate::collision::UNIT;
            if hit.fraction < 1. {
                let normal_speed = body.velocity.dot(hit.normal);
                if normal_speed < 0. {
                    let (friction, elasticity) = body.surface.contact_with(environment);
                    let restitution = if -normal_speed > 0.1 {
                        crate::physics_data::restitution(elasticity, -normal_speed)
                    } else {
                        0.
                    };
                    let normal_impulse = -(1. + restitution) * normal_speed;
                    body.velocity += hit.normal * normal_impulse;
                    let tangent = body.velocity - hit.normal * body.velocity.dot(hit.normal);
                    let length = tangent.length();
                    if length > 0. {
                        body.velocity -= tangent
                            * (friction
                                * normal_impulse
                                / length)
                                .min(1.);
                    }
                    let along = body.velocity - hit.normal * body.velocity.dot(hit.normal);
                    let from = center + delta + hit.normal * 0.0005;
                    let slide = along * dt * (1. - hit.fraction);
                    let sweep =
                        collision.prop_environment_trace(&owners, from, from + slide, extent);
                    if !sweep.start_solid {
                        let correction = hit.normal * 0.0005 + slide * sweep.fraction;
                        e.me.origin +=
                            l4d2r_math::Vec3::new(correction.x, -correction.z, correction.y)
                                / crate::collision::UNIT;
                    }
                }
                body.angular *= 0.65;
                body.on_floor = hit.normal.y > 0.6;
                if hit.normal.y > 0.6 && body.velocity.length_squared() < 0.04 {
                    body.asleep = true;
                    body.supported_sleep = true;
                    body.velocity = Vec3::ZERO;
                    body.angular = Vec3::ZERO;
                }
            } else {
                // Apply angular motion in free flight; resting contacts keep the
                // placed shape instead of rotating a box through the floor.
                let next=(Quat::from_scaled_axis(body.angular*dt)*matrix.rotation).normalize();
                let com=matrix.transform_point(body.center)+delta;
                let origin=com-next*body.center;
                // Translate from the current parent delta back to authored space.
                let own=crate::l4d2r_cinematic::transform(l4d2r_game_core::mapents::entity_matrix(old_origin,old_angles));
                let parent_rotation=matrix.rotation*own.rotation.inverse();
                let local_origin=own.translation+parent_rotation.inverse()*(origin-matrix.translation);
                let (yaw,negative_pitch,roll)=(parent_rotation.inverse()*next).to_euler(EulerRot::YZX);
                e.me.origin=l4d2r_math::Vec3::new(local_origin.x,-local_origin.z,local_origin.y)/crate::collision::UNIT;
                e.me.angles=l4d2r_math::Vec3::new(-negative_pitch,yaw,roll)*(180./std::f32::consts::PI);
                body.rotation=next;
            }
            // Refresh colliders only for a changed transform, not for angular
            // velocity that was suppressed by a resting contact.
            moved |= e.me.origin != old_origin || e.me.angles != old_angles;
        }
        moved | self.solve_pairs(world, bounds)
    }
    fn solve_pairs(
        &mut self,
        world: &mut l4d2r_game_core::entities::EntityWorld,
        bounds: &HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
    ) -> bool {
        let _timing = crate::performance::scope("map::prop_pairs");
        if self.0.values().all(|b| b.disabled || b.asleep) {
            return false;
        }
        let mut items = self
            .0
            .keys()
            .filter(|id| world.ents[**id].alive && bounds.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        let shape = |id: usize, world: &l4d2r_game_core::entities::EntityWorld| {
            let (lo, hi) = bounds[&id];
            let t = crate::l4d2r_cinematic::transform(world.world_matrix(id));
            let c = t.transform_point(Vec3::from_array(crate::binary::source_position(
                ((lo + hi) * 0.5).to_array(),
            )));
            let h = Vec3::from_array(crate::binary::source_position(((hi - lo) * 0.5).to_array()))
                .abs();
            let r = Mat3::from_quat(t.rotation);
            let extent = r.x_axis.abs() * h.x + r.y_axis.abs() * h.y + r.z_axis.abs() * h.z;
            (c, extent)
        };
        let mut geometry = items
            .iter()
            .map(|id| (*id, shape(*id, world)))
            .collect::<HashMap<_, _>>();
        items.sort_by(|a, b| {
            let x = geometry[a];
            let y = geometry[b];
            (x.0.x - x.1.x).total_cmp(&(y.0.x - y.1.x)).then(a.cmp(b))
        });
        let mut pairs = Vec::new();
        for (i, &a) in items.iter().enumerate() {
            let (ca, ha) = geometry[&a];
            for &b in &items[i + 1..] {
                // Native loose debris does not collide with other debris.
                if self.0[&a].debris_motion.is_some() && self.0[&b].debris_motion.is_some() { continue; }
                let (cb, hb) = geometry[&b];
                if cb.x - hb.x > ca.x + ha.x + 0.003 {
                    break;
                }
                if (cb - ca).abs().cmple(ha + hb + Vec3::splat(0.003)).all()
                    && !(self.0[&a].disabled && self.0[&b].disabled)
                {
                    pairs.push((a, b));
                }
            }
        }
        // An awake prop must not make unrelated sleeping stacks run the
        // convex solver. Include the entire connected dynamic island so a
        // contact can still wake and transfer impulses through a stack.
        let mut active = self
            .0
            .iter()
            .filter(|(_, body)| !body.disabled && !body.asleep)
            .map(|(&id, _)| id)
            .collect::<std::collections::HashSet<_>>();
        retain_active_pairs(&mut pairs, &mut active, |id| self.0[&id].disabled);
        // L4D2R_PROP_DEBUG: once a second, the props keeping the solver awake.
        if std::env::var_os("L4D2R_PROP_DEBUG").is_some() {
            static TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 == 0 {
                let awake: Vec<String> = self
                    .0
                    .iter()
                    .filter(|(_, b)| !b.disabled && !b.asleep)
                    .map(|(id, _)| format!("{id}:{}", world.ents[*id].me.kv("model").unwrap_or("")))
                    .collect();
                println!("L4D2R props: {} pairs, {} awake: {:?}", pairs.len(), awake.len(), &awake[..awake.len().min(12)]);
                for &(a, b) in &pairs {
                    let hulls = |id: usize| self.0[&id].hulls.len();
                    println!("L4D2R pair {a}({} hulls, asleep {}, v {:?} w {:?}) - {b}({} hulls, asleep {}, v {:?} w {:?})", hulls(a), self.0[&a].asleep, self.0[&a].velocity, self.0[&a].angular, hulls(b), self.0[&b].asleep, self.0[&b].velocity, self.0[&b].angular);
                }
            }
        }
        if pairs.is_empty() {
            return false;
        }
        // Only broadphase participants need transformed convex vertices and planes.
        // Keep the pair order and contact iterations unchanged.
        let participants = pairs
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .collect::<std::collections::HashSet<_>>();
        let mut convexes = participants
            .iter()
            .map(|id| {
                let transform = crate::l4d2r_cinematic::transform(world.world_matrix(*id));
                (
                    *id,
                    self.0[id]
                        .hulls
                        .iter()
                        .map(|h| h.transformed(&transform))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut moved = false;
        for _ in 0..8 {
            for &(a, b) in &pairs {
                let (ca, ha) = geometry[&a];
                let (cb, hb) = geometry[&b];
                let overlap = ha + hb - (cb - ca).abs();
                if overlap.min_element() < -0.003 {
                    continue;
                }
                let axis = if overlap.x <= overlap.y && overlap.x <= overlap.z {
                    0
                } else if overlap.y <= overlap.z {
                    1
                } else {
                    2
                };
                let mut n = Vec3::ZERO;
                n[axis] = if cb[axis] >= ca[axis] { 1. } else { -1. };
                let mut depth = overlap[axis];
                let mut contact = ((ca - ha).max(cb - hb) + (ca + ha).min(cb + hb)) * 0.5;
                if !convexes[&a].is_empty() && !convexes[&b].is_empty() {
                    let result = convexes[&a]
                        .iter()
                        .flat_map(|ha| {
                            convexes[&b]
                                .iter()
                                .filter_map(move |hb| crate::rigid_contact::convex_contact(ha, hb))
                        })
                        .max_by(|a, b| a.1.total_cmp(&b.1));
                    let Some((normal, penetration, point)) = result else {
                        continue;
                    };
                    n = normal;
                    depth = penetration;
                    contact = point;
                }
                let mut ba = self.0.remove(&a).unwrap();
                let bb = self.0.get_mut(&b).unwrap();
                let ia = if ba.disabled || ba.on_floor && n.y > 0.6 {
                    0.
                } else {
                    1. / ba.mass
                };
                let ib = if bb.disabled || bb.on_floor && n.y < -0.6 {
                    0.
                } else {
                    1. / bb.mass
                };
                let sum = ia + ib;
                if sum > 0. {
                    let correction = (depth - 0.0005).max(0.) * 0.8 / sum;
                    let da = -n * correction * ia;
                    let db = n * correction * ib;
                    let source =
                        |v: Vec3| l4d2r_math::Vec3::new(v.x, -v.z, v.y) / crate::collision::UNIT;
                    world.ents[a].me.origin += source(da);
                    world.ents[b].me.origin += source(db);
                    geometry.get_mut(&a).unwrap().0 += da;
                    geometry.get_mut(&b).unwrap().0 += db;
                    moved |= correction > 0.;
                    for (id, delta) in [(a, da), (b, db)] {
                        for hull in convexes.get_mut(&id).unwrap() {
                            for point in &mut hull.vertices {
                                *point += delta;
                            }
                        }
                    }
                    let local_center = |id| {
                        let (lo,hi)=bounds[&id];
                        Vec3::from_array(crate::binary::source_position(((lo+hi)*0.5).to_array()))
                    };
                    let ra = contact - (ca + ba.rotation*(ba.center-local_center(a)));
                    let rb = contact - (cb + bb.rotation*(bb.center-local_center(b)));
                    let ina = if ba.disabled || ia == 0. {
                        Mat3::ZERO
                    } else {
                        crate::rigid_contact::world_inverse(ba.rotation, ba.inverse_inertia)
                    };
                    let inb = if bb.disabled || ib == 0. {
                        Mat3::ZERO
                    } else {
                        crate::rigid_contact::world_inverse(bb.rotation, bb.inverse_inertia)
                    };
                    let closing =
                        (bb.velocity + bb.angular.cross(rb) - ba.velocity - ba.angular.cross(ra))
                            .dot(n);
                    if closing < 0. {
                        let bounce = if -closing > 0.1 {
                            crate::physics_data::restitution(ba.surface.contact_with(bb.surface).1, -closing)
                        } else {
                            0.
                        };
                        let effective = crate::rigid_contact::effective_mass(ia, ina, ra, n)
                            + crate::rigid_contact::effective_mass(ib, inb, rb, n);
                        let impulse = -(1. + bounce) * closing / effective.max(0.000001);
                        crate::rigid_contact::apply(
                            &mut ba.velocity,
                            &mut ba.angular,
                            ia,
                            ina,
                            ra,
                            -n * impulse,
                        );
                        crate::rigid_contact::apply(
                            &mut bb.velocity,
                            &mut bb.angular,
                            ib,
                            inb,
                            rb,
                            n * impulse,
                        );
                        let relative =
                            bb.velocity + bb.angular.cross(rb) - ba.velocity - ba.angular.cross(ra);
                        let tangent = relative - n * relative.dot(n);
                        let length = tangent.length();
                        let ta = if ba.disabled { 0. } else { 1. / ba.mass };
                        let tb = if bb.disabled { 0. } else { 1. / bb.mass };
                        if length > 1e-6 && ta + tb > 0. {
                            let mu = ba.surface.contact_with(bb.surface).0;
                            let direction = tangent / length;
                            let tangent_mass =
                                crate::rigid_contact::effective_mass(ta, ina, ra, direction)
                                    + crate::rigid_contact::effective_mass(tb, inb, rb, direction);
                            let friction =
                                direction * (length / tangent_mass.max(0.000001)).min(mu * impulse);
                            crate::rigid_contact::apply(
                                &mut ba.velocity,
                                &mut ba.angular,
                                ta,
                                ina,
                                ra,
                                friction,
                            );
                            crate::rigid_contact::apply(
                                &mut bb.velocity,
                                &mut bb.angular,
                                tb,
                                inb,
                                rb,
                                -friction,
                            );
                            if friction.length_squared() > 0.000001 {
                                if !ba.disabled {
                                    ba.asleep = false;
                                }
                                if !bb.disabled {
                                    bb.asleep = false;
                                }
                            }
                        }
                        if ia > 0. {
                            ba.asleep = false;
                        }
                        if ib > 0. {
                            bb.asleep = false;
                        }
                    }
                    if n.y < -0.6 {
                        ba.supported_sleep = true;
                        if ba.velocity.length_squared() < 0.001 {
                            ba.asleep = true;
                            ba.velocity = Vec3::ZERO;
                            ba.resting_on = Some(b);
                        }
                    }
                    if n.y > 0.6 {
                        bb.supported_sleep = true;
                        if bb.velocity.length_squared() < 0.001 {
                            bb.asleep = true;
                            bb.velocity = Vec3::ZERO;
                            bb.resting_on = Some(a);
                        }
                    }
                }
                self.0.insert(a, ba);
            }
        }
        moved
    }
}
pub fn verify_freefall(root: &std::path::Path, output: &std::path::Path) -> std::io::Result<()> {
    verify_fixture(root, output, false)
}
pub fn verify_fixture(
    root: &std::path::Path,
    output: &std::path::Path,
    stack: bool,
) -> std::io::Result<()> {
    let mut data = vec![0; 1036];
    data[..4].copy_from_slice(b"VBSP");
    data[4..8].copy_from_slice(&21u32.to_le_bytes());
    let bsp =
        std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(data).map_err(std::io::Error::other)?);
    let model = "models/props_junk/wood_crate001a.mdl";
    let mut assets = crate::source_assets::SourceAssets::new(root, vec![])?;
    let bytes = assets.read(model)?;
    let (lo, hi, _, _) = l4d2_formats::mdl::read_header_bounds(&bytes)
        .ok_or_else(|| std::io::Error::other("crate bounds unavailable"))?;
    let me = l4d2r_game_core::mapents::MapEnt {
        index: 0,
        classname: "prop_physics".into(),
        targetname: "parity_crate".into(),
        origin: l4d2r_math::Vec3::new(600., 5400., 3100.),
        angles: l4d2r_math::Vec3::ZERO,
        mdl: Some(model.into()),
        brush: None,
        ent: l4d2_formats::bsp::Entity(vec![]),
    };
    let entries = if stack {
        let mut bottom = me.clone();
        bottom.origin.z = 3400.;
        let mut top = me;
        top.index = 1;
        top.origin.z = 3450.;
        vec![bottom, top]
    } else {
        vec![me]
    };
    let mut world = l4d2r_game_core::entities::EntityWorld::new(bsp, entries, 1);
    let bounds = (0..world.ents.len())
        .map(|id| {
            (
                id,
                (
                    l4d2r_math::Vec3::from_array(lo),
                    l4d2r_math::Vec3::from_array(hi),
                ),
            )
        })
        .collect();
    let mut physics = Props::load(&world, &mut assets);
    if stack {
        physics.0.get_mut(&0).unwrap().disabled = true;
    }
    let observed = usize::from(stack);
    let mut csv = "time,x,y,z,velocity_z\n".to_string();
    for i in 0..=90 {
        let p = world.ents[observed].me.origin;
        csv.push_str(&format!(
            "{:.6},{:.6},{:.6},{:.6},{:.6}\n",
            i as f32 / 30.,
            p.x,
            p.y,
            p.z,
            physics.0[&observed].velocity.y / crate::collision::UNIT
        ));
        physics.tick(
            &mut world,
            &bounds,
            &crate::collision::Collision::default(),
            1. / 30.,
        );
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, csv)?;
    println!(
        "Prop {} trace: {}",
        if stack {
            "convex stack"
        } else {
            "freeflight (no contacts)"
        },
        output.display()
    );
    Ok(())
}
