//! Registered impulse activation. Queue ownership stays in the unit registry;
//! draining/physical sleep eligibility belongs to the external PSI lifecycle.
use crate::{ivp_joint_apply::Body, ivp_joint_tail::Command, ivp_motion::CoreSample, ivp_unit_registry::Registry};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding { pub body: usize, pub current: usize, pub real: usize }

pub(crate) fn apply_with(
    bodies: &mut [Body], bindings: &[Binding], registry: &mut Registry,
    commands: &[Command],
    mut observe: impl FnMut(usize, Command, Option<CoreSample>, &[Body], &Registry),
) -> Result<(), &'static str> {
    let mut lookup = HashMap::new();
    for (i, body) in bodies.iter().enumerate() {
        if lookup.insert(body.id, i).is_some() { return Err("duplicate impulse body"); }
    }
    let mut bound = HashMap::new(); let mut currents = HashSet::new();
    for &binding in bindings {
        let &index = lookup.get(&binding.body).ok_or("unknown bound impulse body")?;
        if bound.insert(binding.body, binding).is_some() || !currents.insert(binding.current) {
            return Err("duplicate/shared current core binding is not recovered");
        }
        registry.queued(binding.real)?;
        if registry.queued(binding.current)? != (bodies[index].flags & 4 != 0) {
            return Err("incoherent registered queued flag");
        }
    }
    if bound.len() != bodies.len() { return Err("missing current core binding"); }
    for &command in commands {
        let id = match command { Command::PointImpulse { body, .. } | Command::AngularPush { body, .. } => body };
        if !bound.contains_key(&id) { return Err("missing impulse target binding"); }
    }
    let mut next = bodies.to_vec(); let mut queue = registry.clone();
    for &command in commands {
        let id = match command { Command::PointImpulse { body, .. } | Command::AngularPush { body, .. } => body };
        let index = lookup[&id];
        let sample = if next[index].object_flags & 0xff == 8 {
            queue.enqueue_wake(bound[&id].real)?;
            for body in &mut next {
                body.flags = (body.flags & !4) | if queue.queued(bound[&body.id].current)? { 4 } else { 0 };
            }
            next[index].apply_pending(command)
        } else { next[index].apply(command)? };
        observe(index, command, sample, &next, &queue);
    }
    bodies.copy_from_slice(&next); *registry = queue; Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ivp_joint_apply::tests::{body, snapshot, check_pose, command};
    use serde_json::{Value, json};
    fn data() -> Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-impulse-wake.json")).unwrap() }
    fn setup(input: &Value) -> (Vec<Body>, Registry, Vec<Binding>) {
        let bodies = input["cores"].as_array().unwrap().iter().map(body).collect::<Vec<_>>();
        let mut registry = Registry::new(99);
        for b in &bodies { registry.register_core(b.id, b.flags & 2 != 0).unwrap(); }
        for b in &bodies { if b.flags >> 8 == 1 { registry.wake(b.id).unwrap(); } }
        registry.begin_psi().unwrap();
        for id in input["pending"].as_array().unwrap() { registry.enqueue_wake(id.as_u64().unwrap() as usize).unwrap(); }
        let bindings = bodies.iter().enumerate().map(|(i,b)| Binding { body:b.id,
            current:input["current"][i].as_u64().unwrap() as usize, real:input["real"][i].as_u64().unwrap() as usize }).collect();
        (bodies,registry,bindings)
    }
    fn check(bodies: &[Body], registry: &Registry, expected: &Value) {
        assert_eq!(json!(registry.pending_cores()), expected["pending"]);
        for (b,e) in bodies.iter().zip(expected["cores"].as_array().unwrap()) { assert_eq!(snapshot(*b), *e); }
    }
    #[test]
    fn native_sleeping_impulse_bursts_preserve_queue_pending_pose_and_clock_bits() {
        let data = data(); let mut counts = [0usize;3]; let mut total = 0;
        for case in data["cases"].as_array().unwrap() {
            let scenario = case["scenario"].as_u64().unwrap() as usize;
            let mut state = None;
            for op in case["operations"].as_array().unwrap() {
                let kind = op["kind"].as_u64().unwrap();
                // World steps and caller-accepted sleep requests are controlled
                // boundaries, not reproduced by this impulse application test.
                if kind >= 2 { state = None; continue; }
                let (bodies, registry, bindings) = state.get_or_insert_with(|| setup(&op["before"]));
                check(bodies, registry, &op["before"]);
                let id = op["body"].as_u64().unwrap() as usize;
                let b = bodies.iter().find(|b|b.id==id).unwrap();
                let vector = [0.125 * (scenario+1) as f32, -0.25, 0.5];
                let generated = if kind == 0 { let origin=b.current.position.to_array();
                    Command::PointImpulse { body:id, point:std::array::from_fn(|i|origin[i]+[0.3,-0.4,0.2][i]), impulse:vector }
                } else { Command::AngularPush { body:id, delta:vector } };
                let c=command(generated); for key in ["kind","body","point","vector"] { assert_eq!(c[key],op[key]); }
                counts[match b.object_flags&0xff {8=>0,1=>1,16=>2,_=>panic!("unexpected object kind")}] += 1;
                apply_with(bodies, bindings, registry, &[generated], |_,_,sample,_,_| {
                    if let Some(sample)=sample { check_pose(sample,&op["pose"]); }
                }).unwrap();
                check(bodies, registry, &op["after"]); total+=1;
            }
        }
        assert_eq!((total,counts),(112,[74,22,16]));
    }
    #[test]
    fn invalid_late_binding_or_command_cannot_partially_queue_or_apply() {
        let data=data(); let (mut bodies,mut registry,mut bindings)=setup(&data["cases"][0]["initial"]);
        let before=bodies.iter().copied().map(snapshot).collect::<Vec<_>>();
        let valid=Command::AngularPush { body:0, delta:[1.,2.,3.] };
        bindings[2].real=12345;
        assert!(apply_with(&mut bodies,&bindings,&mut registry,&[valid],|_,_,_,_,_|panic!()).is_err());
        bindings[2].real=2;
        let invalid=Command::AngularPush { body:12345, delta:[1.;3] };
        assert!(apply_with(&mut bodies,&bindings,&mut registry,&[valid,invalid],|_,_,_,_,_|panic!()).is_err());
        assert_eq!(bodies.iter().copied().map(snapshot).collect::<Vec<_>>(),before);
        assert!(registry.pending_cores().is_empty());
    }
}
