//! Sphere feature generation and1010b840 queue admission in the PSI phase.
//! The caller owns pair discovery, cached frames, motion bounds and dispatch.
use crate::{ivp_distance_schedule as schedule,ivp_event_queue::Queue,ivp_motion::Pose};
pub(crate) struct Input {
    pub schedule:schedule::Input,
    pub initial:[Pose;2],
    pub inactive:[bool;2],
    pub margin:f32,
    pub config:schedule::Config,
    pub delay:u8,
}
pub(crate) use schedule::queue::Outcome;
/// The owning PSI callback must have removed its own queue entry already.
/// Candidate geometry never comes from a recorded/native event timeline.
pub(crate) fn run(queue:&mut Queue,owner:usize,i:Input,mut sample:impl FnMut(usize,f64)->Pose)->Result<Outcome,&'static str>{
    schedule::queue::run(queue,owner,i.schedule,i.delay,i.config,|request|{
            let candidate=crate::ivp_time::sphere_candidate(crate::ivp_time::SphereCandidate{
                now:request.now,deadline:request.deadline,margin:i.margin,
                target:request.target,gap:i.config.gap,bound:request.bound,
                initial:i.initial,inactive:i.inactive,
            },&mut sample);
            Ok(schedule::queue::Candidate{kind:if candidate.is_some(){16}else{0},time:candidate})
    })
}
#[cfg(test)]mod tests {
    use super::*;
    use bevy::prelude::*;
    #[test]
    fn single_feature_call_matches_pending_keys_and_increments_once(){
        let data:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-sphere-queue.json")).unwrap();
        let mut states_checked=0;let mut entries_checked=0;
        for c in data["cases"].as_array().unwrap(){
            let rows=c["states"].as_array().unwrap();
            let first=rows.iter().position(|r|!r["contact"].is_null()).unwrap();
            let mut world=crate::ivp_registered_contacts::ContactWorld::empty_native(1./60.,rows[0]["poll"]["seed"].as_u64().unwrap() as u32,Vec3::new(0.,-0.,0.),Vec3::ZERO).unwrap();
            for side in 0..2 {
                world.create_sphere(side+1,crate::ivp_sphere_constructor::Parameters{radius:c["radius"].as_f64().unwrap() as f32,position:[c["separation"].as_f64().unwrap() as f32*side as f32,0.,128.],angles:[0.;3],mass:10.+5.*side as f32,inertia_scale:1.,damping:0.125,angular_damping:0.25,rotational_limit:0.1,fixed:side==0&&c["fixed0"].as_bool().unwrap(),collisions:true},0.8).unwrap();
                world.set_velocity(side as usize,Some(Vec3::from_array(std::array::from_fn(|k|c["linear_commands"][side as usize][k].as_f64().unwrap() as f32))),Some(Vec3::ZERO)).unwrap();
            }
            let mut graph=crate::ivp_mindist_graph::Graph::new(2);let pair=graph.register([1,0]).unwrap();graph.activate(pair,[false;2]).unwrap();
            for (step,row) in rows.iter().enumerate().take(first){
                if step!=0{world.advance(0.001).unwrap();}
                assert_eq!(serde_json::json!(world.bodies().iter().copied().map(crate::ivp_joint_apply::tests::snapshot).collect::<Vec<_>>()),row["physical"],"source pre-impact bank scenario {} step {step}",c["scenario"]);states_checked+=1;
                for observed in row["pairs"].as_array().unwrap().iter().filter(|p|p["queued"]==true){
                    assert_eq!(observed["objects"],serde_json::json!([1,0]));assert_eq!(observed["features"],serde_json::json!([3,3]));
                    let bodies=[world.bodies()[1],world.bodies()[0]];let now=world.clock().start;let deadline=world.clock().deadline;
                    let frame=crate::ivp_motion::ObjectFrame{shift:None,rotation:None};
                    let caches=bodies.map(|b|b.prediction.prepare_object_cache(now,frame,0));
                    let radius=c["radius"].as_f64().unwrap() as f32*f32::from_bits(0x3cd013a9);let margin=radius+radius;
                    let geometry=crate::ivp_minimizer::sphere_geometry(caches.map(|p|p.pose.position),caches.map(|p|p.core_position),margin).unwrap();
                    assert_eq!(margin.to_bits() as u64,observed["margin"].as_u64().unwrap());
                    assert_eq!(geometry.distance.to_bits() as u64,observed["measures"][2].as_u64().unwrap());
                    assert_eq!(geometry.projection.to_bits() as u64,observed["measures"][0].as_u64().unwrap());
                    assert_eq!(serde_json::json!(geometry.normal.to_array().map(f32::to_bits)),observed["normal"]);
                    // These reference worlds have centred spheres and no spin.
                    // Their angular-radius contribution is zero; far hull
                    // budgets are not read in this near-contact feature branch.
                    let velocity=|side:usize|Vec3::from_array(std::array::from_fn(|k|bodies[side].linear[k]));
                    for body in bodies{assert_eq!(&body.angular[..3],&[0.;3]);}
                    let config=schedule::Config::default();let flags=graph.flags(pair).unwrap();
                    let input=schedule::Input{flags,counter:0,now,deadline,dt:1./60.,distance:geometry.distance,target:config.target(flags).unwrap(),normal:geometry.normal,
                        cores:std::array::from_fn(|side|schedule::Core{motion:crate::ivp_bounds::Motion{axis:Vec3::ZERO,angular_bound:0.,velocity:velocity(side)},linear:crate::ivp_bounds::speed(velocity(side))}),
                        budgets:[schedule::Budget{time:0.,flags:0,rates:[0.;2],accumulated:[0.;2]};2],allow_far:false,slow_gate:config.slow_gate};
                    // Reconstruct the PSI callback phase from the owner: its
                    // entry is removed before pair insertion and restored last.
                    // This exercises the real queue allocator, not a time-only
                    // assertion, without claiming live dispatch is connected.
                    let prediction_owner=world.clock().prediction_owner();
                    let mut queue=world.clock().queue.clone();queue.cancel(prediction_owner).unwrap();
                    let outcome=run(&mut queue,0,Input{schedule:input,initial:caches.map(|p|p.pose),inactive:bodies.map(|b|b.object_flags&0xff>=8),margin,config,delay:1},|side,at|bodies[side].prediction.sample(at,frame)).unwrap();
                    queue.insert(prediction_owner,(deadline-queue.epoch) as f32).unwrap();
                    let (epoch,key)=queue.scheduled(0).unwrap();
                    assert_eq!(outcome.handle.map(u64::from),observed["handle"].as_u64());
                    assert_eq!(outcome.kind,16);assert!(outcome.candidate.is_some());
                    assert_eq!(outcome.flags as u64,observed["flags"].as_u64().unwrap());
                    assert_eq!(epoch.to_bits(),observed["epoch"].as_u64().unwrap());assert_eq!(key.to_bits() as u64,observed["key"].as_u64().unwrap(),"queue key scenario {} step {step}",c["scenario"]);entries_checked+=1;
                    // This is one low-level scheduling call, not the complete native
                    // lifecycle. The owner regression checks the full native counter.
                    assert_eq!(outcome.counter,1);
                }
            }
        }
        assert_eq!(states_checked,1430);assert_eq!(entries_checked,10);
        println!("Matched {states_checked} source-constructed physical banks and {entries_checked} pending queue keys/epochs/handles/flags");

    }
}
