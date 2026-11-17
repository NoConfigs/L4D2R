//! Recovered ragdoll numerical callbacks, error reporting and shared sweeps.
//! Physical core poses/properties and constructed joint parameters are inputs;
//! body construction, controller/world contacts and integration stay separate.
use crate::{ivp_joint_angular as angular, ivp_joint_frame as frame, ivp_joint_linear as linear};
pub(crate) use linear::Phase;
#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub flags: u16,
    pub inverse: [f32; 4],
    pub rotation: [[f32; 3]; 3],
    pub position: [f32; 3],
    pub object_offset: Option<[f32; 3]>,
    pub angular: [f32; 4],
    pub linear: [f32; 4],
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step { pub dt: f32, pub inverse_dt: f32, pub alpha: f32, pub beta: f32 }
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GroupError { pub maximum: [f32; 4] }
impl GroupError {
    fn report(&mut self, error: [f32; 4]) {
        let square = (error[1] * error[1] + error[0] * error[0]) + error[2] * error[2];
        // 100aeeb0 MAXPS selects its second operand on ties/unordered input.
        self.maximum = self.maximum.map(|old| if old > square { old } else { square });
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Work {
    pub coordinates: frame::Coordinates,
    pub rows: [angular::Row; 3],
    pub linear: Option<linear::Cache>,
    /// Unused SIMD gamma lane is deliberately not retained as semantic state.
    pub factors: [f32; 3],
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Joint {
    pub frames: [[[f32; 3]; 3]; 2],
    pub anchors: [[f32; 3]; 2],
    pub axes: [angular::Axis; 3],
    pub limit_enabled: [bool; 3],
    pub linear_enabled: bool,
    pub damping: f32,
    pub tau: f32,
    pub work: Option<Work>,
}
impl Joint {
    pub(crate) fn run(&mut self, bodies: &mut [Body; 2], group: Option<&mut GroupError>, step: Step, phase: Phase) -> Result<(), &'static str> {
        // A caller contract failure must not leave motor targets or one core
        // committed without the rest of this callback.
        let mut next = *self;
        let mut end = *bodies;
        let mut next_error = group.as_deref().copied();
        next.run_inner(&mut end, next_error.as_mut(), step, phase)?;
        *self = next; *bodies = end;
        if let Some(group) = group { *group = next_error.unwrap(); }
        Ok(())
    }
    fn run_inner(&mut self, bodies: &mut [Body; 2], group: Option<&mut GroupError>, step: Step, phase: Phase) -> Result<(), &'static str> {
        let initial = matches!(phase, Phase::Initial);
        let mut work = if initial {
            Work { coordinates: frame::from_core_frames(bodies.map(|b| b.rotation), self.frames),
                rows: [angular::Row::default(); 3], linear: None, factors: [0.; 3] }
        } else { self.work.ok_or("cached joint callback lacks preparation")? };
        let directions = work.coordinates.directions;
        work.factors = [step.alpha, step.beta, directions[0][3]];
        let cores = bodies.map(|b| angular::CoreInput { flags: b.flags,
            inverse_inertia: [b.inverse[0], b.inverse[1], b.inverse[2]], rotation: b.rotation });
        angular::prepare_rows(&cores, self.limit_enabled, directions.map(|v| [v[0], v[1], v[2]]), &mut work.rows)?;
        let mut velocity = bodies.map(|b| [b.angular[0], b.angular[1], b.angular[2]]);
        angular::solve(&mut velocity, &mut self.axes, &work.rows, work.coordinates.value,
            [step.alpha, step.beta, 0., work.factors[2]], step.dt, step.inverse_dt)?;
        for side in 0..2 { bodies[side].angular = [velocity[side][0], velocity[side][1], velocity[side][2], 0.]; }
        if initial && self.linear_enabled {
            let geometry = std::array::from_fn(|side| {
                let b = bodies[side];
                linear::Geometry { flags: b.flags, inverse: b.inverse, rotation: b.rotation,
                    position: b.position, object_offset: b.object_offset, anchor: self.anchors[side] }
            });
            work.linear = Some(linear::prepare(&geometry));
            if let Some(group) = group { group.report(work.linear.unwrap().error); }
        }
        let mut cores = bodies.map(|b| linear::Core { flags: b.flags, angular: b.angular, linear: b.linear, inverse_mass: b.inverse[3] });
        linear::solve(&mut cores, self.linear_enabled, work.linear.as_ref(), self.damping, self.tau,
            step.alpha, step.beta, step.inverse_dt, phase)?;
        for side in 0..2 { bodies[side].angular = cores[side].angular; bodies[side].linear = cores[side].linear; }
        self.work = Some(work);
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding { pub bodies: [usize; 2], pub joint: Joint }

pub(crate) fn solve_group_with(
    bodies: &mut [Body], bindings: &mut [Binding], additional: u32, dt: f32, inverse_dt: f32,
    mut observe: impl FnMut(usize, Phase, Step, &[Body], &[Binding], &GroupError),
) -> Result<GroupError, &'static str> {
    if bindings.iter().any(|b| b.bodies[0] == b.bodies[1] || b.bodies.iter().any(|&i| i >= bodies.len())) {
        return Err("invalid joint core binding");
    }
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        return Err("native reciprocal estimate backend unavailable");
    }
    // Native 100af4d0 clears the group's maximum error on entry.
    let mut error = GroupError::default();
    for batch in crate::ivp_joint_group::Group::new(additional).batches() {
        for slot in 0..bindings.len() {
            let slot = if batch.reverse { bindings.len() - 1 - slot } else { slot };
            let ids = bindings[slot].bodies;
            let mut pair = [bodies[ids[0]], bodies[ids[1]]];
            let phase = if batch.prepare { Phase::Initial } else { Phase::Cached };
            let step = Step { dt, inverse_dt, alpha: batch.p4, beta: batch.p5 };
            bindings[slot].joint.run(&mut pair, Some(&mut error), step, phase)?;
            bodies[ids[0]] = pair[0]; bodies[ids[1]] = pair[1];
            observe(slot, phase, step, bodies, bindings, &error);
        }
    }
    Ok(error)
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
pub(crate) mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    fn data() -> Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-callback.json")).unwrap() }
    fn f(v: &Value) -> f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    pub(crate) fn body(c: &Value) -> Body {
        Body { flags: c["flags"].as_u64().unwrap() as u16, inverse: std::array::from_fn(|i| f(&c["inverse"][i])),
            rotation: std::array::from_fn(|r| std::array::from_fn(|i| f(&c["rotation"][r * 3 + i]))),
            position: std::array::from_fn(|i| f(&c["position"][i])),
            object_offset: if c["object_flags"].as_u64().unwrap() & 0x800 == 0 { Some(std::array::from_fn(|i| f(&c["offset"][i]))) } else { None },
            angular: std::array::from_fn(|i| f(&c["angular"][i])), linear: std::array::from_fn(|i| f(&c["linear"][i])) }
    }
    pub(crate) fn joint(p: &Value) -> Joint {
        Joint { frames: std::array::from_fn(|side| std::array::from_fn(|axis| std::array::from_fn(|i| f(&p["frames"][side * 12 + axis * 4 + i])))),
            anchors: std::array::from_fn(|side| std::array::from_fn(|i| f(&p["cores"][side]["anchor"][i]))),
            axes: std::array::from_fn(|i| { let a = &p["axes"][i]; angular::Axis { motor: a["motor"].as_u64().unwrap() != 0,
                min: f(&a["min"]), max: f(&a["max"]), tau: f(&a["tau"]), force: f(&a["force"]), velocity: f(&a["velocity"]), target: f(&a["target"]) } }),
            limit_enabled: std::array::from_fn(|i| p["axes"][i]["enabled"].as_u64().unwrap() & 1 != 0),
            linear_enabled: p["enabled"].as_u64().unwrap() != 0, damping: f(&p["damping"]), tau: f(&p["tau"]), work: None }
    }
    fn work(w: Option<Work>) -> Value {
        let Some(w) = w else { return Value::Null; };
        let linear = w.linear.map(|c| c.error.into_iter().chain(c.jacobian.into_iter().flatten().flatten())
            .chain(c.response.into_iter().flatten().flatten()).chain(c.inverse.into_iter().flatten()).map(f32::to_bits).collect::<Vec<_>>());
        let rows = w.rows.map(|r| if r.mode != 1 { json!({"mode":r.mode}) } else {
            json!({"mode":1,"ja":r.jacobian[0].map(f32::to_bits),"jb":r.jacobian[1].map(f32::to_bits),
                "ra":r.response[0].map(f32::to_bits),"rb":r.response[1].map(f32::to_bits),"k":r.k.to_bits(),"inverse":r.inverse.to_bits(),"mask":r.mask})
        });
        json!({"linear":linear,"factors":w.factors.map(f32::to_bits),"coordinates":w.coordinates.value.map(f32::to_bits),
            "directions":w.coordinates.directions.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>(),"rows":rows})
    }
    pub(crate) fn snapshot(before: &Value, j: &Joint, pair: &[Body; 2], group: Option<GroupError>) -> Value {
        let mut actual = before.clone();
        for side in 0..2 {
            actual["cores"][side]["flags"] = json!(pair[side].flags);
            actual["cores"][side]["inverse"] = json!(pair[side].inverse.map(f32::to_bits));
            actual["cores"][side]["rotation"] = json!(pair[side].rotation.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());
            actual["cores"][side]["position"] = json!(pair[side].position.map(f32::to_bits));
            actual["cores"][side]["offset"] = json!(pair[side].object_offset.unwrap_or([0.; 3]).map(f32::to_bits));
            actual["cores"][side]["anchor"] = json!(j.anchors[side].map(f32::to_bits));
            assert_eq!(pair[side].object_offset.is_some(), before["cores"][side]["object_flags"].as_u64().unwrap() & 0x800 == 0);
            actual["cores"][side]["angular"] = json!(pair[side].angular.map(f32::to_bits));
            actual["cores"][side]["linear"] = json!(pair[side].linear.map(f32::to_bits));
        }
        for i in 0..3 {
            let a = j.axes[i];
            actual["axes"][i] = json!({"enabled":u32::from(j.limit_enabled[i]),"motor":u32::from(a.motor),
                "min":a.min.to_bits(),"max":a.max.to_bits(),"tau":a.tau.to_bits(),"force":a.force.to_bits(),
                "velocity":a.velocity.to_bits(),"target":a.target.to_bits()});
        }
        for side in 0..2 { for axis in 0..3 { for i in 0..3 {
            actual["frames"][side * 12 + axis * 4 + i] = json!(j.frames[side][axis][i].to_bits());
        }}}
        actual["enabled"] = json!(u32::from(j.linear_enabled));
        actual["damping"] = json!(j.damping.to_bits()); actual["tau"] = json!(j.tau.to_bits());
        actual["work"] = work(j.work);
        actual["group_error"] = json!(group.map(|g| g.maximum.map(f32::to_bits)));
        actual
    }
    #[test]
    fn native_complete_callbacks_keep_owned_targets_and_cache_across_sweeps_and_steps() {
        let data = data(); let mut joints = BTreeMap::new(); let mut total = 0; let mut initial = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let before = &call["before"];
            let key = (call["scenario"].as_u64().unwrap(), call["joint"].as_u64().unwrap());
            let j = joints.entry(key).or_insert_with(|| joint(before));
            for i in 0..3 { assert_eq!(j.axes[i].target.to_bits(), before["axes"][i]["target"].as_u64().unwrap() as u32, "incoming target {index}/{i}"); }
            let prepare = call["phase"].as_u64().unwrap() == 0;
            if !prepare { assert_eq!(work(j.work), before["work"], "owned incoming cache {index}"); }
            let mut pair = std::array::from_fn(|side| body(&before["cores"][side]));
            let mut group = before["group_error"].as_array().map(|v| GroupError { maximum: std::array::from_fn(|i| f(&v[i])) });
            let step = Step { dt: f(&call["dt_bits"]), inverse_dt: f(&call["inverse_dt_bits"]), alpha: f(&call["p4"]), beta: f(&call["p5"]) };
            j.run(&mut pair, group.as_mut(), step, if prepare { initial += 1; Phase::Initial } else { Phase::Cached }).unwrap();
            assert_eq!(snapshot(before, j, &pair, group), call["after"], "complete callback {index}");
            total += 1;
        }
        assert_eq!((total, initial, joints.len()), (1368, 132, 44));
    }
    #[test]
    fn native_all_ragdoll_groups_share_source_velocities_and_owned_dispatch_order() {
        let data = data(); let calls = data["calls"].as_array().unwrap(); let mut groups = 0; let mut compared = 0;
        for case in data["cases"].as_array().unwrap() {
            // Mixed hinge groups retain coverage in the callback test above;
            // an autonomous mixed-kernel group still needs the native hinge.
            if case["types"].as_array().unwrap().iter().any(|v| v.as_u64().unwrap() != 0) { continue; }
            let scenario = case["scenario"].as_u64().unwrap();
            let order = case["order"].as_array().unwrap();
            let mut bindings = order.iter().map(|id| {
                let id = id.as_u64().unwrap();
                let seed = calls.iter().find(|c| c["scenario"].as_u64() == Some(scenario) && c["joint"].as_u64() == Some(id)).unwrap();
                Binding { bodies: [id as usize + 1, id as usize], joint: joint(&seed["before"]) }
            }).collect::<Vec<_>>();
            for tick in 0..3 {
                let expected = calls.iter().filter(|c| c["scenario"].as_u64() == Some(scenario) && c["step"].as_u64() == Some(tick)).collect::<Vec<_>>();
                // Controlled per-PSI inputs, sampled once on first touch. No
                // later native velocity or cache is fed back into the group.
                let mut inputs = vec![None; case["count"].as_u64().unwrap() as usize + 1];
                for c in &expected { for core in c["before"]["cores"].as_array().unwrap() {
                    let id = core["id"].as_u64().unwrap() as usize;
                    if inputs[id].is_none() { inputs[id] = Some(body(core)); }
                }}
                let mut bodies = inputs.into_iter().map(Option::unwrap).collect::<Vec<_>>();
                let mut index = 0;
                let final_error = solve_group_with(&mut bodies, &mut bindings, case["authored_iterations"].as_i64().unwrap().max(0) as u32,
                    f(&expected[0]["dt_bits"]), f(&expected[0]["inverse_dt_bits"]),
                    |slot, phase, event, bodies, bindings, error| {
                        let c = expected[index]; let binding = &bindings[slot];
                        assert_eq!(order[slot].as_u64().unwrap(), c["joint"].as_u64().unwrap());
                        assert_eq!(u64::from(matches!(phase, Phase::Cached)), c["phase"].as_u64().unwrap());
                        assert_eq!((event.alpha.to_bits(), event.beta.to_bits()), (c["p4"].as_u64().unwrap() as u32, c["p5"].as_u64().unwrap() as u32));
                        for side in 0..2 { assert_eq!(binding.bodies[side] as u64, c["before"]["cores"][side]["id"].as_u64().unwrap()); }
                        let pair = binding.bodies.map(|id| bodies[id]);
                        assert_eq!(snapshot(&c["before"], &binding.joint, &pair, Some(*error)), c["after"], "group {scenario} tick{tick} callback{index}");
                        index += 1;
                    }).unwrap();
                assert_eq!(json!(final_error.maximum.map(f32::to_bits)), expected.last().unwrap()["after"]["group_error"]);
                assert_eq!(index, expected.len()); compared += index;
            }
            groups += 1;
        }
        assert_eq!((groups, compared), (12, 774));
    }
    #[test]
    fn invalid_cached_calls_and_group_bindings_cannot_partially_commit() {
        let data = data(); let first = &data["calls"][0]["before"];
        let mut j = joint(first); let mut pair = std::array::from_fn(|side| body(&first["cores"][side]));
        let mut error = GroupError::default();
        let before = snapshot(first, &j, &pair, Some(error));
        let step = Step { dt: 1. / 60., inverse_dt: 60., alpha: 1., beta: 1. };
        assert!(j.run(&mut pair, Some(&mut error), step, Phase::Cached).is_err());
        assert_eq!(snapshot(first, &j, &pair, Some(error)), before);
        for bad in [[0, 0], [0, 2]] {
            let mut bindings = [Binding { bodies: [0, 1], joint: j }, Binding { bodies: bad, joint: j }];
            let mut visited = 0;
            assert!(solve_group_with(&mut pair, &mut bindings, 0, step.dt, step.inverse_dt,
                |_, _, _, _, _, _| visited += 1).is_err());
            assert_eq!(visited, 0);
            assert_eq!(snapshot(first, &bindings[0].joint, &pair, Some(error)), before);
        }
    }
}
