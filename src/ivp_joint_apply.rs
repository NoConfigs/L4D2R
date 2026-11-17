//! Ordinary-object 100ea090/100ea010 pending impulses and 100f0460 stamps.
//! Registered sleeping-object activation is provided by ivp_impulse_wake;
//! these ordinary entry points require no registry context.
use crate::ivp_joint_tail::Command;
use crate::ivp_motion::{CoreSample, Pose, Prediction};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub id: usize,
    pub flags: u16,
    pub object_flags: u32,
    pub mass: f32,
    pub inverse: [f32; 4],
    pub current: Pose,
    pub prediction: Prediction,
    pub clock: f64,
    pub stamps: [f64; 2],
    pub pending_angular: [f32; 4],
    pub pending_linear: [f32; 4],
    pub angular: [f32; 4],
    pub linear: [f32; 4],
}
fn target(command: Command) -> usize {
    match command { Command::PointImpulse { body, .. } | Command::AngularPush { body, .. } => body }
}
fn inverse_direction(pose: Pose, v: [f32; 3]) -> [f32; 3] {
    let r = pose.rotation.transpose().to_cols_array();
    std::array::from_fn(|i| (r[3+i] * v[1] + r[i] * v[0]) + r[6+i] * v[2])
}
impl Body {
    ///100e9f90 ordinary direct velocity addition, used by fixed-ledge recovery.
    /// Sleeping-object activation requires the registered owner path instead.
    pub(crate) fn push_velocity(&mut self,delta:[f32;3])->Result<(),&'static str>{
        if delta.iter().any(|v|!v.is_finite()){return Err("invalid direct velocity push");}
        if self.object_flags&0xff==8{return Err("sleeping-object velocity push requires registered wake context");}
        self.stamps=[self.clock;2];
        self.pending_linear[0]=self.pending_linear[0]+delta[0];
        self.pending_linear[1]=delta[1]+self.pending_linear[1];
        self.pending_linear[2]=delta[2]+self.pending_linear[2];Ok(())
    }
    pub(crate) fn apply(&mut self, command: Command) -> Result<Option<CoreSample>, &'static str> {
        if target(command) != self.id { return Err("impulse target does not match bound body"); }
        if self.object_flags & 0xff == 8 { return Err("sleeping-object impulses require registered wake context"); }
        let mut next = *self;
        // Ordinary-object activation records the environment clock twice.
        // This fast path does not change registry membership or core flags.
        next.stamps = [next.clock; 2];
        let sample = next.apply_pending(command);
        *self = next; Ok(sample)
    }
    pub(crate) fn apply_pending(&mut self, command: Command) -> Option<CoreSample> {
        match command {
            Command::AngularPush { delta, .. } => {
                self.pending_angular[0] = self.pending_angular[0] + delta[0];
                self.pending_angular[1] = delta[1] + self.pending_angular[1];
                self.pending_angular[2] = delta[2] + self.pending_angular[2];
                None
            },
            Command::PointImpulse { point, impulse, .. } => {
                let sample = self.prediction.sample_physical_core(self.clock, (self.flags >> 8) as u8, self.current);
                let pose = sample.pose;
                let origin = pose.position.to_array();
                let point = inverse_direction(pose, std::array::from_fn(|i| point[i] - origin[i]));
                let local = inverse_direction(pose, impulse);
                let inverse = self.inverse;
                self.pending_angular[0] = self.pending_angular[0] + (local[2] * point[1] - local[1] * point[2]) * inverse[0];
                self.pending_angular[1] = self.pending_angular[1] + inverse[1] * (local[0] * point[2] - point[0] * local[2]);
                self.pending_angular[2] = self.pending_angular[2] + inverse[2] * (point[0] * local[1] - local[0] * point[1]);
                for i in 0..3 { self.pending_linear[i] = self.pending_linear[i] + impulse[i] * inverse[3]; }
                Some(sample)
            },
        }
    }
}
pub(crate) fn apply_with(
    bodies: &mut [Body], commands: &[Command],
    mut observe: impl FnMut(usize, Command, Option<CoreSample>, &[Body]),
) -> Result<(), &'static str> {
    let mut lookup = HashMap::with_capacity(bodies.len());
    for (index,b) in bodies.iter().enumerate() {
        if lookup.insert(b.id,index).is_some() { return Err("duplicate impulse body binding"); }
    }
    for &command in commands {
        let &index = lookup.get(&target(command)).ok_or("missing impulse body binding")?;
        if bodies[index].object_flags & 0xff == 8 { return Err("sleeping-object impulses require registered wake context"); }
    }
    for &command in commands {
        let index = lookup[&target(command)];
        let sample = bodies[index].apply(command)?;
        observe(index,command,sample,bodies);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bevy::prelude::{Vec3, Mat3};
    use serde_json::{Value,json};
    fn data() -> Value { serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-apply.json")).unwrap() }
    fn f(v:&Value)->f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn d(v:&Value)->f64 { f64::from_bits(v.as_u64().unwrap()) }
    pub(crate) fn body(p:&Value)->Body {
        Body { id:p["id"].as_u64().unwrap() as usize,flags:p["flags"].as_u64().unwrap() as u16,
            object_flags:p["object_flags"].as_u64().unwrap() as u32,mass:f(&p["mass"]),
            inverse:std::array::from_fn(|i|f(&p["inverse"][i])),
            current:Pose { position:Vec3::from_array(std::array::from_fn(|i|f(&p["position"][i]))),
                rotation:Mat3::from_cols_array(&std::array::from_fn(|i|f(&p["rotation"][i]))).transpose() },
            prediction:Prediction { start:Vec3::from_array(std::array::from_fn(|i|f(&p["start"][i]))),
                velocity:Vec3::from_array(std::array::from_fn(|i|f(&p["velocity"][i]))),
                rotation_start:std::array::from_fn(|i|f(&p["q0"][i])),rotation_end:std::array::from_fn(|i|f(&p["q1"][i])),
                time:d(&p["cache_time"]),inverse_dt:f(&p["inverse_dt"]) },clock:d(&p["clock"]),
            stamps:std::array::from_fn(|i|d(&p["stamps"][i])),
            pending_angular:std::array::from_fn(|i|f(&p["pending_angular"][i])),pending_linear:std::array::from_fn(|i|f(&p["pending_linear"][i])),
            angular:std::array::from_fn(|i|f(&p["angular"][i])),linear:std::array::from_fn(|i|f(&p["linear"][i])) }
    }
    pub(crate) fn snapshot(b:Body)->Value {
        json!({"id":b.id,"flags":b.flags,"object_flags":b.object_flags,"mass":b.mass.to_bits(),
            "position":b.current.position.to_array().map(f32::to_bits),"rotation":b.current.rotation.transpose().to_cols_array().map(f32::to_bits),
            "inverse":b.inverse.map(f32::to_bits),"pending_angular":b.pending_angular.map(f32::to_bits),"pending_linear":b.pending_linear.map(f32::to_bits),
            "angular":b.angular.map(f32::to_bits),"linear":b.linear.map(f32::to_bits),
            "start":b.prediction.start.to_array().map(f32::to_bits),"velocity":b.prediction.velocity.to_array().map(f32::to_bits),
            "q0":b.prediction.rotation_start.map(f32::to_bits),"q1":b.prediction.rotation_end.map(f32::to_bits),
            "cache_time":b.prediction.time.to_bits(),"inverse_dt":b.prediction.inverse_dt.to_bits(),
            "clock":b.clock.to_bits(),"stamps":b.stamps.map(f64::to_bits)})
    }
    pub(crate) fn check_pose(sample:CoreSample,p:&Value) {
        assert_eq!(json!(sample.pose.rotation.transpose().to_cols_array().map(f32::to_bits)),p["rotation"]);
        assert_eq!(json!(sample.pose.position.to_array().map(f32::to_bits)),p["position"]);
        if !p["quaternion"].is_null() { assert_eq!(json!(sample.quaternion.unwrap().map(f32::to_bits)),p["quaternion"]); }
    }
    pub(crate) fn command(c:Command)->Value {
        match c {
            Command::PointImpulse{body,point,impulse}=>json!({"kind":0,"body":body,"point":point.map(f32::to_bits),"vector":impulse.map(f32::to_bits)}),
            Command::AngularPush{body,delta}=>json!({"kind":1,"body":body,"point":Value::Null,"vector":delta.map(f32::to_bits)}),
        }
    }
    #[test]
    fn native_physical_core_sampler_matches_equal_inactive_and_extrapolated_pose_bits() {
        let data=data();let mut total=0;let mut copied=0;
        for case in data["cases"].as_array().unwrap() { for sample in case["samples"].as_array().unwrap() {
            let b=body(&sample["input"]);let pose=b.prediction.sample_physical_core(d(&sample["now"]),(b.flags>>8) as u8,b.current);
            copied+=usize::from(pose.quaternion.is_none());check_pose(pose,sample);total+=1;
        }}
        assert_eq!((total,copied),(375,99));
    }
    #[test]
    fn native_recovery_generation_and_application_own_pending_velocities_and_stamps() {
        let data=data();let calls=data["calls"].as_array().unwrap();let mut total=0;let mut applications=0;let mut poses=0;
        for case in data["cases"].as_array().unwrap() {
            let scenario=case["scenario"].as_u64().unwrap();
            let expected=calls.iter().filter(|c|c["scenario"].as_u64()==Some(scenario)).collect::<Vec<_>>();
            let mut state=crate::ivp_joint_tail::State::new(expected[0]["before"]["members"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect(),(scenario%3) as i32,3.);
            for c in expected {
                let actions=&c["actions"];
                if actions["clear"]==true { state.clear_error(); }
                if actions["set_params"]==true { state.set_public_params(actions["ticks"].as_i64().unwrap() as i32,f(&actions["tolerance"])); }
                for request in actions["queue"].as_array().unwrap() { let ids:[Option<usize>;2]=std::array::from_fn(|i|{let id=request[i].as_u64().unwrap();(id!=99).then_some(id as usize)});state.queue(ids[0],ids[1]); }
                let tail=&c["tail"];let mut bodies=tail["cores"].as_array().unwrap().iter().map(body).collect::<Vec<_>>();
                let inputs=bodies.iter().map(|b|crate::ivp_joint_tail::Body{id:b.id,flags:b.flags,mass:b.mass,position:b.current.position.to_array()}).collect::<Vec<_>>();
                let generated=state.finish(&inputs,f(&tail["dt"]),std::array::from_fn(|i|f(&tail["gravity"][i])),f(&tail["maximum"][0])).unwrap();
                let mut index=0;
                apply_with(&mut bodies,&generated,|slot,cmd,sample,bodies| {
                    let native=&c["commands"][index];let mut args=native.clone();for k in ["before","after","pose"] { args.as_object_mut().unwrap().remove(k); }
                    assert_eq!(command(cmd),args,"command group{scenario}/step{}/command{index}",c["step"]);
                    assert_eq!(snapshot(bodies[slot]),native["after"],"application group{scenario}/step{}/command{index}",c["step"]);
                    if let Some(sample)=sample { check_pose(sample,&native["pose"]);poses+=1; } else { assert!(native["pose"].is_null()); }
                    index+=1;
                }).unwrap();
                assert_eq!(index,c["commands"].as_array().unwrap().len());applications+=index;
                for (b,native) in bodies.iter().zip(c["after"]["cores"].as_array().unwrap()) { assert_eq!(snapshot(*b),*native); }
                assert_eq!(state.ticks,c["after"]["ticks"].as_i64().unwrap() as i32);total+=1;
            }
        }
        assert_eq!((total,applications,poses),(120,608,304));
    }
    #[test]
    fn invalid_later_impulse_binding_or_special_object_cannot_partially_apply_batch() {
        let data=data();let p=&data["calls"][0]["tail"]["cores"];let seed=[body(&p[0]),body(&p[1])];
        for special in [false,true] {
            let mut bodies=seed;if special { bodies[1].object_flags=(bodies[1].object_flags&!0xff)|8; }
            let before=bodies;let commands=[Command::AngularPush{body:bodies[0].id,delta:[1.,0.,0.]},
                Command::AngularPush{body:if special { bodies[1].id } else { 99 },delta:[1.,0.,0.]}];
            assert!(apply_with(&mut bodies,&commands,|_,_,_,_|panic!("invalid batch applied")).is_err());
            for i in 0..2 { assert_eq!(snapshot(bodies[i]),snapshot(before[i])); }
        }
    }
}
