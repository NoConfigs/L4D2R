//! Numerical callbacks -> recovery generation -> ordinary impulse application
//! -> queue/error completion. World prediction/integration and constructors
//! remain external inputs; this owner does not replace the world controller.
use crate::{ivp_joint_apply as apply, ivp_joint_mixed as mixed, ivp_joint_tail as tail};
use bevy::prelude::Mat3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Member { pub body: apply::Body, pub object_offset: Option<[f32; 3]> }
#[derive(Clone, Copy, Debug)]
pub(crate) struct Environment {
    pub dt: f32,
    pub inverse_dt: f32,
    /// Native rescue reads environment PSI dt, separately from callback args.
    pub rescue_dt: f32,
    pub gravity: [f32; 3],
}
#[derive(Clone, Debug)]
pub(crate) struct Group { pub bindings: Vec<mixed::Binding>, pub tail: tail::State }
#[derive(Clone, Debug)]
pub(crate) struct Outcome { pub error: mixed::GroupError, pub commands: Vec<tail::Command> }
impl Group {
    pub(crate) fn run_with(
        &mut self, members: &mut [Member], additional: u32, environment: Environment,
        mut numerical: impl FnMut(usize, mixed::Phase, mixed::Step, &[mixed::Body], &[mixed::Binding], &mixed::GroupError),
        mut applied: impl FnMut(usize, tail::Command, Option<crate::ivp_motion::CoreSample>, &[apply::Body], &tail::State),
    ) -> Result<Outcome, &'static str> {
        // Contract failures, including unsupported activation discovered after
        // a solve, must not commit a half-completed group to its caller.
        let mut next = self.clone(); let mut end = members.to_vec();
        let mut cores = end.iter().map(|m| {
            let b = m.body; let r = b.current.rotation.transpose().to_cols_array();
            mixed::Body { flags: b.flags, inverse: b.inverse,
                rotation: std::array::from_fn(|i| std::array::from_fn(|j|r[i*3+j])),
                position: b.current.position.to_array(), object_offset: m.object_offset,
                angular: b.angular, linear: b.linear }
        }).collect::<Vec<_>>();
        let error = mixed::solve_group_with(&mut cores, &mut next.bindings, additional,
            environment.dt, environment.inverse_dt, |slot,phase,step,bodies,bindings,error| {
                numerical(slot,phase,step,bodies,bindings,error);
            })?;
        for (m,c) in end.iter_mut().zip(cores) {
            m.body.flags=c.flags;m.body.inverse=c.inverse;
            m.body.current.position=bevy::prelude::Vec3::from_array(c.position);
            m.body.current.rotation=Mat3::from_cols_array(&std::array::from_fn(|i|c.rotation[i/3][i%3])).transpose();
            m.body.angular=c.angular;m.body.linear=c.linear;m.object_offset=c.object_offset;
            // Prediction remains the world's pre-existing cache. Rebuilding it
            // from the new solver velocity would change native impulse poses.
        }
        let inputs=end.iter().map(|m|tail::Body { id:m.body.id,flags:m.body.flags,mass:m.body.mass,
            position:m.body.current.position.to_array() }).collect::<Vec<_>>();
        let commands=next.tail.prepare(&inputs,environment.rescue_dt,environment.gravity)?;
        let mut bodies=end.iter().map(|m|m.body).collect::<Vec<_>>();
        apply::apply_with(&mut bodies,&commands,|slot,command,sample,bodies| {
            applied(slot,command,sample,bodies,&next.tail);
        })?;
        for (m,b) in end.iter_mut().zip(bodies) { m.body=b; }
        // 100af9fb/100afa02: consume queue/update counter after all impulses.
        next.tail.complete(error.maximum[0]);
        *self=next;members.copy_from_slice(&end);
        Ok(Outcome { error,commands })
    }
}

#[cfg(all(test,any(target_arch="x86",target_arch="x86_64")))]
mod tests {
    use super::*;
    use crate::{ivp_hinge_callback::tests as ht,ivp_joint_callback::tests as rt,ivp_joint_apply::tests as at};
    use serde_json::{Value,json};
    fn data()->Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-complete.json")).unwrap() }
    fn physical()->Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-apply.json")).unwrap() }
    fn f(v:&Value)->f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn member(p:&Value,constructor:&[&Value])->Member {
        let id=p["id"].as_u64().unwrap();
        let core=constructor.iter().flat_map(|c|c["before"]["cores"].as_array().unwrap()).find(|b|b["id"].as_u64()==Some(id)).unwrap();
        Member { body:at::body(p),object_offset:(core["object_flags"].as_u64().unwrap()&0x800==0).then(||std::array::from_fn(|i|f(&core["offset"][i]))) }
    }
    fn group(case:&Value,entry:&Value,callbacks:&[&Value])->Group {
        let members=entry["before"]["members"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect::<Vec<_>>();
        let bindings=case["order"].as_array().unwrap().iter().map(|id| {
            let seed=callbacks.iter().find(|c|&c["joint"]==id).unwrap();let id=id.as_u64().unwrap() as usize;
            mixed::Binding { bodies:[members.iter().position(|&n|n==id+1).unwrap(),members.iter().position(|&n|n==id).unwrap()],
                joint:if seed["kind"]==0 { mixed::Joint::Ragdoll(rt::joint(&seed["before"])) } else { mixed::Joint::Hinge(ht::joint(&seed["before"])) } }
        }).collect();
        Group { bindings,tail:tail::State::new(members,(case["scenario"].as_u64().unwrap()%3) as i32,3.) }
    }
    #[test]
    fn native_complete_groups_own_solver_error_recovery_impulses_and_counter_order() {
        let data=data();let physical=physical();let all=data["calls"].as_array().unwrap();let controls=physical["calls"].as_array().unwrap();
        let mut groups=0;let mut steps=0;let mut solved=0;let mut applied=0;
        for case in data["cases"].as_array().unwrap() {
            let scenario=case["scenario"].as_u64().unwrap();
            let callbacks=all.iter().filter(|c|c["scenario"].as_u64()==Some(scenario)).collect::<Vec<_>>();
            let expected=controls.iter().filter(|c|c["scenario"].as_u64()==Some(scenario)).collect::<Vec<_>>();
            let mut group=group(case,expected[0],&callbacks);
            for control in expected {
                let tick=control["step"].as_u64().unwrap();
                let packets=callbacks.iter().copied().filter(|c|c["step"].as_u64()==Some(tick)).collect::<Vec<_>>();
                let actions=&control["actions"];
                if actions["clear"]==true { group.tail.clear_error(); }
                if actions["set_params"]==true { group.tail.set_public_params(actions["ticks"].as_i64().unwrap() as i32,f(&actions["tolerance"])); }
                for request in actions["queue"].as_array().unwrap() {
                    let ids:[Option<usize>;2]=std::array::from_fn(|i|{let id=request[i].as_u64().unwrap();(id!=99).then_some(id as usize)});
                    group.tail.queue(ids[0],ids[1]);
                }
                assert_eq!(group.tail.ticks,control["before"]["ticks"].as_i64().unwrap() as i32);
                assert_eq!(json!(group.tail.pairs),control["before"]["pairs"]);
                // Seed physical/prediction inputs at group entry, before ANY
                // callback. No post-solve native velocities/error are loaded.
                let mut members=control["before"]["cores"].as_array().unwrap().iter().map(|b|member(b,&callbacks)).collect::<Vec<_>>();
                let environment=Environment { dt:f(&packets[0]["dt_bits"]),inverse_dt:f(&packets[0]["inverse_dt_bits"]),
                    rescue_dt:f(&control["before"]["dt"]),gravity:std::array::from_fn(|i|f(&control["before"]["gravity"][i])) };
                let mut n=0;let mut a=0;
                let outcome=group.run_with(&mut members,case["authored_iterations"].as_i64().unwrap().max(0) as u32,environment,
                    |slot,phase,step,bodies,bindings,error| {
                        let p=packets[n];let binding=&bindings[slot];let pair=binding.bodies.map(|i|bodies[i]);
                        assert_eq!(case["order"][slot],p["joint"]);
                        assert_eq!(u64::from(matches!(phase,mixed::Phase::Cached)),p["phase"].as_u64().unwrap());
                        assert_eq!((step.alpha.to_bits(),step.beta.to_bits()),(p["p4"].as_u64().unwrap() as u32,p["p5"].as_u64().unwrap() as u32));
                        let actual=match &binding.joint {
                            mixed::Joint::Fixed(_)=>panic!("legacy complete-group corpus contains a fixed joint"),
                            mixed::Joint::Ragdoll(j)=>{assert_eq!(p["kind"],0);rt::snapshot(&p["before"],j,&pair,Some(*error))},
                            mixed::Joint::Hinge(j)=>{assert_eq!(p["kind"],1);let mut v=ht::snapshot(&p["before"],&pair,j);v["group_error"]=json!(error.maximum.map(f32::to_bits));v},
                        };
                        assert_eq!(actual,p["after"],"group{scenario}/step{tick}/solver{n}");n+=1;
                    },|slot,command,sample,bodies,pending| {
                        let p=&control["commands"][a];let mut args=p.clone();for k in ["before","after","pose"] { args.as_object_mut().unwrap().remove(k); }
                        assert_eq!(at::command(command),args);
                        assert_eq!(at::snapshot(bodies[slot]),p["after"],"group{scenario}/step{tick}/application{a}");
                        if let Some(sample)=sample { at::check_pose(sample,&p["pose"]); } else { assert!(p["pose"].is_null()); }
                        // Counter/queue must still be uncompleted while the
                        // command observer runs, matching native instruction order.
                        assert_eq!(pending.ticks,control["tail"]["ticks"].as_i64().unwrap() as i32);
                        assert_eq!(json!(pending.pairs),control["tail"]["pairs"]);a+=1;
                    }).unwrap();
                assert_eq!(n,packets.len());assert_eq!(a,control["commands"].as_array().unwrap().len());
                assert_eq!(outcome.commands.len(),a);
                assert_eq!(json!(outcome.error.maximum.map(f32::to_bits)),control["tail"]["maximum"]);
                for (m,p) in members.iter().zip(control["after"]["cores"].as_array().unwrap()) { assert_eq!(at::snapshot(m.body),*p); }
                assert_eq!(group.tail.ticks,control["after"]["ticks"].as_i64().unwrap() as i32);
                assert_eq!(json!(group.tail.in_error()),control["after"]["in_error"]);
                assert_eq!(json!(group.tail.pairs),control["after"]["pairs"]);
                solved+=n;applied+=a;steps+=1;
            }
            groups+=1;
        }
        assert_eq!((groups,steps,solved,applied),(20,120,3498,608));
    }
    #[test]
    fn late_recovery_failure_rolls_back_solver_targets_caches_and_physical_members() {
        let data=data();let physical=physical();let case=&data["cases"][0];let entry=&physical["calls"][0];
        let callbacks=data["calls"].as_array().unwrap().iter().filter(|c|c["scenario"]==case["scenario"]).collect::<Vec<_>>();
        let seed=group(case,entry,&callbacks);
        let members=entry["before"]["cores"].as_array().unwrap().iter().map(|p|member(p,&callbacks)).collect::<Vec<_>>();
        for special in [false,true] {
            let mut group=seed.clone();let mut members=members.clone();
            if special { members[1].body.object_flags=(members[1].body.object_flags&!0xff)|8;group.tail.queue(Some(members[0].body.id),Some(members[1].body.id)); }
            else { group.tail.members.push(99); }
            let before_group=group.clone();let before_members=members.clone();
            let result=group.run_with(&mut members,0,Environment{dt:1./60.,inverse_dt:60.,rescue_dt:1./60.,gravity:[0.,9.83,0.]},|_,_,_,_,_,_|{},|_,_,_,_,_|panic!("invalid recovery applied"));
            assert!(result.is_err());assert_eq!(format!("{group:?}"),format!("{before_group:?}"));
            for (a,b) in members.iter().zip(before_members) { assert_eq!(at::snapshot(a.body),at::snapshot(b.body)); }
        }
    }
}
