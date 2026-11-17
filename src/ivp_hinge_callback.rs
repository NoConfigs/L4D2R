//! Complete initial/cached hinge numerical callbacks. World/contact/body
//! construction and mixed-kernel group lifecycle remain separate.
use crate::{ivp_hinge_prepare as prepare, ivp_hinge_rows as rows, ivp_joint_angular as angular};
pub(crate) use crate::ivp_joint_callback::{Body, Step, Phase};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Joint {
    pub anchors: [[f32; 3]; 2],
    pub directions: [[f32; 3]; 2],
    pub perpendiculars: [[f32; 3]; 2],
    pub axis: angular::Axis,
    pub limit_enabled: bool,
    pub tau: f32,
    pub damping: f32,
    pub work: Option<rows::Cache>,
}
impl Joint {
    pub(crate) fn run(&mut self, bodies: &mut [Body; 2], step: Step, phase: Phase) -> Result<(), &'static str> {
        let mut next = *self; let mut end = *bodies;
        next.run_inner(&mut end, step, phase)?;
        *self = next; *bodies = end; Ok(())
    }
    fn run_inner(&mut self, bodies: &mut [Body; 2], step: Step, phase: Phase) -> Result<(), &'static str> {
        let initial = matches!(phase, Phase::Initial);
        let mut cache = if initial {
            let geometry = std::array::from_fn(|side| {
                let b = bodies[side];
                prepare::Geometry { core: crate::ivp_joint_linear::Geometry { flags: b.flags,
                    inverse: b.inverse, rotation: b.rotation, position: b.position,
                    object_offset: b.object_offset, anchor: self.anchors[side] }, axis: self.directions[side] }
            });
            if self.limit_enabled || self.axis.motor {
                let world = prepare::transform(bodies[0].rotation, self.directions[0]);
                let perpendicular = prepare::transform(bodies[0].rotation, self.perpendiculars[0]);
                let second = prepare::cross(perpendicular, world);
                let attached = prepare::transform(bodies[1].rotation, self.perpendiculars[1]);
                let coordinate = crate::ivp_hinge_angle::coordinate(perpendicular, second, attached)?;
                let cores = bodies.map(|b| angular::CoreInput { flags: b.flags, rotation: b.rotation,
                    inverse_inertia: [b.inverse[0], b.inverse[1], b.inverse[2]] });
                let mut velocity = bodies.map(|b| b.angular);
                crate::ivp_hinge_motor::solve(&cores, &mut velocity, &mut self.axis, self.limit_enabled,
                    world, coordinate, step.dt, step.inverse_dt, step.alpha, step.beta)?;
                for side in 0..2 { bodies[side].angular = velocity[side]; }
            }
            prepare::prepare(&geometry, self.perpendiculars[0], step.inverse_dt)?
        } else { self.work.ok_or("cached hinge callback lacks preparation")? };
        let mut cores = bodies.map(|b| rows::Core { flags: b.flags, angular: b.angular,
            linear: b.linear, inverse_mass: b.inverse[3] });
        if initial { rows::solve_initial(&mut cores, &mut cache, self.tau, self.damping, step.alpha, step.beta)?; }
        else { rows::solve(&mut cores, &mut cache, self.tau, self.damping, step.alpha, step.beta)?; }
        for side in 0..2 { bodies[side].angular = cores[side].angular; bodies[side].linear = cores[side].linear; }
        self.work = Some(cache); Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding { pub bodies: [usize; 2], pub joint: Joint }
pub(crate) fn solve_group_with(
    bodies: &mut [Body], bindings: &mut [Binding], additional: u32, dt: f32, inverse_dt: f32,
    mut observe: impl FnMut(usize, Phase, Step, &[Body], &[Binding]),
) -> Result<(), &'static str> {
    if bindings.iter().any(|b| b.bodies[0] == b.bodies[1] || b.bodies.iter().any(|&i| i >= bodies.len())) {
        return Err("invalid hinge core binding");
    }
    for batch in crate::ivp_joint_group::Group::new(additional).batches() {
        for slot in 0..bindings.len() {
            let slot = if batch.reverse { bindings.len() - 1 - slot } else { slot };
            let ids = bindings[slot].bodies; let mut pair = ids.map(|i| bodies[i]);
            let phase = if batch.prepare { Phase::Initial } else { Phase::Cached };
            let step = Step { dt, inverse_dt, alpha: batch.p4, beta: batch.p5 };
            bindings[slot].joint.run(&mut pair, step, phase)?;
            bodies[ids[0]] = pair[0]; bodies[ids[1]] = pair[1];
            observe(slot, phase, step, bodies, bindings);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    fn data() -> Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-callback.json")).unwrap() }
    fn f(v: &Value) -> f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    pub(crate) fn body(c: &Value) -> Body {
        Body { flags: c["flags"].as_u64().unwrap() as u16,
            inverse: std::array::from_fn(|i| f(&c["inverse"][i])),
            rotation: std::array::from_fn(|i| std::array::from_fn(|j| f(&c["rotation"][i*3+j]))),
            position: std::array::from_fn(|i| f(&c["position"][i])),
            object_offset: (c["object_flags"].as_u64().unwrap() & 0x800 == 0).then(|| std::array::from_fn(|i| f(&c["offset"][i]))),
            angular: std::array::from_fn(|i| f(&c["angular"][i])), linear: std::array::from_fn(|i| f(&c["linear"][i])) }
    }
    pub(crate) fn joint(p: &Value) -> Joint {
        Joint { anchors: std::array::from_fn(|s| std::array::from_fn(|i| f(&p["geometry"][s*8+i]))),
            directions: std::array::from_fn(|s| std::array::from_fn(|i| f(&p["geometry"][s*8+4+i]))),
            perpendiculars: std::array::from_fn(|s| std::array::from_fn(|i| f(&p["geometry"][16+s*4+i]))),
            axis: angular::Axis { motor: p["motor"] == 1, min: f(&p["axis"][0]), max: f(&p["axis"][1]),
                tau: f(&p["axis"][2]), force: f(&p["axis"][3]), velocity: f(&p["axis"][4]), target: f(&p["axis"][5]) },
            limit_enabled: p["limit"] == 1, tau: f(&p["tau"]), damping: f(&p["damping"]), work: None }
    }
    pub(crate) fn snapshot(p: &Value, bodies: &[Body; 2], joint: &Joint) -> Value {
        let mut out = p.clone(); out.as_object_mut().unwrap().remove("work");
        for side in 0..2 {
            out["cores"][side]["angular"] = json!(bodies[side].angular.map(f32::to_bits));
            out["cores"][side]["linear"] = json!(bodies[side].linear.map(f32::to_bits));
        }
        out["axis"][5] = json!(joint.axis.target.to_bits());
        let c = joint.work.unwrap(); out["stride"] = json!(8);
        out["bias"] = json!(c.bias.map(f32::to_bits));
        out["matrix"] = json!(c.inverse.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());
        out["relative"] = json!(c.relative.map(f32::to_bits)); out["impulse"] = json!(c.impulse.map(f32::to_bits));
        out["rows"] = json!(c.rows.map(|side| side.map(|r| json!({"linear":r.linear.map(f32::to_bits),
            "angular":r.angular.map(f32::to_bits),"index":r.index,"response":r.response.map(f32::to_bits)}))));
        for side in 0..2 { if bodies[side].flags & 0x12 != 0 { for i in 0..5 { out["rows"][side][i]["response"] = Value::Null; } } }
        out
    }
    #[test]
    fn native_complete_hinge_callbacks_keep_owned_targets_and_caches_across_steps() {
        let data = data(); let mut joints = BTreeMap::new(); let mut total = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let p = &call["before"];
            let key = (call["scenario"].as_u64().unwrap(), call["joint"].as_u64().unwrap());
            let joint = joints.entry(key).or_insert_with(|| joint(p));
            let mut bodies = std::array::from_fn(|s| body(&p["cores"][s]));
            let phase = if call["phase"] == 0 { Phase::Initial } else { Phase::Cached };
            if matches!(phase, Phase::Initial) {
                let world = prepare::transform(bodies[0].rotation, joint.directions[0]);
                let perpendicular = prepare::transform(bodies[0].rotation, joint.perpendiculars[0]);
                let second = prepare::cross(perpendicular, world);
                let attached = prepare::transform(bodies[1].rotation, joint.perpendiculars[1]);
                assert_eq!(json!([second,perpendicular,attached].into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>()),
                    call["angle_data"]["vectors"], "angle vectors{index}");
                assert_eq!(crate::ivp_hinge_angle::coordinate(perpendicular,second,attached).unwrap().to_bits() as u64,
                    call["angle_data"]["angle"].as_u64().unwrap(), "angle{index}");
            }
            joint.run(&mut bodies, Step { dt: f(&call["dt_bits"]), inverse_dt: f(&call["inverse_dt_bits"]),
                alpha: f(&call["p4"]), beta: f(&call["p5"]) }, phase).unwrap();
            assert_eq!(snapshot(p, &bodies, joint), call["after"], "callback{index}"); total += 1;
        }
        assert_eq!(total, 1764);
    }
    #[test]
    fn native_complete_hinge_groups_keep_source_owned_shared_velocities_and_dispatch() {
        let data = data(); let calls = data["calls"].as_array().unwrap(); let mut total = 0; let mut groups = 0;
        for case in data["cases"].as_array().unwrap() {
            let scenario = case["scenario"].as_u64().unwrap(); let order = case["order"].as_array().unwrap();
            let mut bindings = order.iter().map(|id| {
                let seed = calls.iter().find(|c| c["scenario"].as_u64() == Some(scenario) && &c["joint"] == id).unwrap();
                let id = id.as_u64().unwrap() as usize;
                Binding { bodies: [id+1,id], joint: joint(&seed["before"]) }
            }).collect::<Vec<_>>();
            for tick in 0..3 {
                let expected = calls.iter().filter(|c| c["scenario"].as_u64() == Some(scenario) && c["step"].as_u64() == Some(tick)).collect::<Vec<_>>();
                let mut seeds = vec![None; case["count"].as_u64().unwrap() as usize + 1];
                for c in &expected { for core in c["before"]["cores"].as_array().unwrap() {
                    let id = core["id"].as_u64().unwrap() as usize;
                    if seeds[id].is_none() { seeds[id] = Some(body(core)); }
                }}
                let mut bodies = seeds.into_iter().map(Option::unwrap).collect::<Vec<_>>(); let mut index = 0;
                solve_group_with(&mut bodies, &mut bindings, case["authored_iterations"].as_i64().unwrap().max(0) as u32,
                    f(&expected[0]["dt_bits"]), f(&expected[0]["inverse_dt_bits"]), |slot, phase, step, bodies, bindings| {
                        let c = expected[index]; let b = &bindings[slot];
                        assert_eq!(order[slot], c["joint"]); assert_eq!(u64::from(matches!(phase, Phase::Cached)), c["phase"].as_u64().unwrap());
                        assert_eq!((step.alpha.to_bits(),step.beta.to_bits()),(c["p4"].as_u64().unwrap() as u32,c["p5"].as_u64().unwrap() as u32));
                        assert_eq!(snapshot(&c["before"], &b.bodies.map(|i| bodies[i]), &b.joint), c["after"], "group{scenario}/tick{tick}/callback{index}"); index += 1;
                    }).unwrap();
                assert_eq!(index, expected.len()); total += index;
            }
            groups += 1;
        }
        assert_eq!((groups,total),(21,1764));
    }
    #[test]
    fn invalid_cached_hinge_and_group_binding_do_not_partially_commit() {
        let data = data(); let p = &data["calls"][0]["before"];
        let mut joint = joint(p); let original = joint;
        let mut bodies = std::array::from_fn(|s| body(&p["cores"][s])); let before = bodies;
        let step = Step { dt: 1./60., inverse_dt: 60., alpha: 1., beta: 1. };
        assert!(joint.run(&mut bodies, step, Phase::Cached).is_err());
        assert_eq!(format!("{joint:?}"),format!("{original:?}"));
        assert_eq!(format!("{bodies:?}"),format!("{before:?}"));
        let mut bindings = [Binding { bodies: [0,1], joint }, Binding { bodies: [1,2], joint }];
        assert!(solve_group_with(&mut bodies,&mut bindings,0,step.dt,step.inverse_dt,|_,_,_,_,_| panic!("invalid group dispatched")).is_err());
        assert_eq!(format!("{bodies:?}"),format!("{before:?}"));
        assert!(bindings.iter().all(|b| b.joint.work.is_none()));
    }
}
