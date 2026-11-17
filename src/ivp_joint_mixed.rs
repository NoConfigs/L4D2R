//! Shared numerical ownership for 100af4d0 ragdoll/hinge groups. Group-tail
//! rescue/lifecycle and the world/contact controller remain separate.
use crate::{ivp_hinge_callback as hinge, ivp_joint_callback as ragdoll};
pub(crate) use ragdoll::{Body, GroupError, Phase, Step};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Joint { Ragdoll(ragdoll::Joint), Hinge(hinge::Joint), Fixed(crate::ivp_fixed_callback::Joint) }
impl Joint {
    pub(crate) fn run(&mut self, bodies: &mut [Body; 2], error: &mut GroupError, step: Step, phase: Phase) -> Result<(), &'static str> {
        match self {
            Self::Ragdoll(j) => j.run(bodies, Some(error), step, phase),
            // Native hinges do not report an anchor error to 100aeeb0.
            Self::Hinge(j) => j.run(bodies, step, phase),
            Self::Fixed(j) => j.run(bodies, step, phase),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding { pub bodies: [usize; 2], pub joint: Joint }

pub(crate) fn solve_group_with(
    bodies: &mut [Body], bindings: &mut [Binding], additional: u32, dt: f32, inverse_dt: f32,
    mut observe: impl FnMut(usize, Phase, Step, &[Body], &[Binding], &GroupError),
) -> Result<GroupError, &'static str> {
    if bindings.iter().any(|b| b.bodies[0] == b.bodies[1] || b.bodies.iter().any(|&i| i >= bodies.len())) {
        return Err("invalid mixed joint core binding");
    }
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        return Err("native joint estimate/angle backend unavailable");
    }
    let mut error = GroupError::default();
    for batch in crate::ivp_joint_group::Group::new(additional).batches() {
        for slot in 0..bindings.len() {
            let slot = if batch.reverse { bindings.len() - 1 - slot } else { slot };
            let ids = bindings[slot].bodies; let mut pair = ids.map(|i| bodies[i]);
            let phase = if batch.prepare { Phase::Initial } else { Phase::Cached };
            let step = Step { dt, inverse_dt, alpha: batch.p4, beta: batch.p5 };
            bindings[slot].joint.run(&mut pair, &mut error, step, phase)?;
            bodies[ids[0]] = pair[0]; bodies[ids[1]] = pair[1];
            observe(slot, phase, step, bodies, bindings, &error);
        }
    }
    Ok(error)
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
mod tests {
    use super::*;
    use crate::{ivp_hinge_callback::tests as ht, ivp_joint_callback::tests as rt};
    use serde_json::{Value, json};
    fn data() -> Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-mixed.json")).unwrap() }
    fn f(v: &Value) -> f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn joint(c: &Value) -> Joint {
        if c["kind"] == 0 { Joint::Ragdoll(rt::joint(&c["before"])) }
        else { assert_eq!(c["kind"],1); Joint::Hinge(ht::joint(&c["before"])) }
    }
    #[test]
    fn native_mixed_groups_share_source_velocities_targets_caches_and_group_error() {
        let data = data(); let calls = data["calls"].as_array().unwrap();
        let mut total = 0; let mut groups = 0; let mut mixed_groups = 0; let mut kinds = [0; 2];
        for case in data["cases"].as_array().unwrap() {
            let scenario = case["scenario"].as_u64().unwrap(); let order = case["order"].as_array().unwrap();
            let mut bindings = order.iter().map(|id| {
                let seed = calls.iter().find(|c| c["scenario"].as_u64() == Some(scenario) && &c["joint"] == id).unwrap();
                let id = id.as_u64().unwrap() as usize;
                Binding { bodies: [id+1,id], joint: joint(seed) }
            }).collect::<Vec<_>>();
            mixed_groups += usize::from(bindings.iter().any(|b| matches!(b.joint,Joint::Hinge(_)))
                && bindings.iter().any(|b| matches!(b.joint,Joint::Ragdoll(_))));
            for tick in 0..3 {
                let expected = calls.iter().filter(|c| c["scenario"].as_u64() == Some(scenario) && c["step"].as_u64() == Some(tick)).collect::<Vec<_>>();
                // Physical poses/properties are inputs per PSI. Seed each
                // core once; all later shared velocities are source-owned.
                let mut seeds = vec![None;case["count"].as_u64().unwrap() as usize+1];
                for c in &expected { for core in c["before"]["cores"].as_array().unwrap() {
                    let id = core["id"].as_u64().unwrap() as usize;
                    if seeds[id].is_none() { seeds[id] = Some(rt::body(core)); }
                }}
                let mut bodies = seeds.into_iter().map(Option::unwrap).collect::<Vec<_>>(); let mut index = 0;
                let result = solve_group_with(&mut bodies,&mut bindings,case["authored_iterations"].as_i64().unwrap().max(0) as u32,
                    f(&expected[0]["dt_bits"]),f(&expected[0]["inverse_dt_bits"]),|slot,phase,step,bodies,bindings,error| {
                        let c = expected[index]; let b = &bindings[slot]; let pair = b.bodies.map(|i|bodies[i]);
                        assert_eq!(order[slot],c["joint"]);
                        assert_eq!(u64::from(matches!(phase,Phase::Cached)),c["phase"].as_u64().unwrap());
                        assert_eq!((step.alpha.to_bits(),step.beta.to_bits()),(c["p4"].as_u64().unwrap() as u32,c["p5"].as_u64().unwrap() as u32));
                        let actual = match &b.joint {
                            Joint::Fixed(_) => panic!("legacy mixed corpus contains a fixed joint"),
                            Joint::Ragdoll(j) => { assert_eq!(c["kind"],0); kinds[0]+=1; rt::snapshot(&c["before"],j,&pair,Some(*error)) },
                            Joint::Hinge(j) => { assert_eq!(c["kind"],1); kinds[1]+=1;
                                let mut out = ht::snapshot(&c["before"],&pair,j);
                                out["group_error"] = json!(error.maximum.map(f32::to_bits)); out },
                        };
                        assert_eq!(actual,c["after"],"mixed group{scenario}/tick{tick}/callback{index}"); index+=1;
                    }).unwrap();
                assert_eq!(index,expected.len()); total+=index;
                assert_eq!(json!(result.maximum.map(f32::to_bits)),expected.last().unwrap()["after"]["group_error"]);
            }
            groups+=1;
        }
        assert_eq!((groups,mixed_groups,total,kinds),(21,9,1764,[1368,396]));
    }
    #[test]
    fn invalid_later_mixed_binding_cannot_mutate_an_earlier_valid_joint() {
        let data=data();let seed=&data["calls"][0];
        let bodies = std::array::from_fn::<_,2,_>(|s|rt::body(&seed["before"]["cores"][s]));
        for invalid in [[0,0],[1,2]] {
            let mut bodies=bodies;let before=bodies;
            let mut bindings=[Binding{bodies:[0,1],joint:joint(seed)},Binding{bodies:invalid,joint:joint(seed)}];
            let original=bindings;
            assert!(solve_group_with(&mut bodies,&mut bindings,0,1./60.,60.,|_,_,_,_,_,_|panic!("invalid mixed group dispatched")).is_err());
            assert_eq!(format!("{before:?}"),format!("{bodies:?}"));
            assert_eq!(format!("{original:?}"),format!("{bindings:?}"));
        }
    }
}
