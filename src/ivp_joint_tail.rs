//! 100af450 queueing, 100af737..100afa2a rescue commands/error counters,
//! and public 10084c60/10084c80/10084ec0 error APIs. Applying point impulses
//! through native pose caches and body activation is a separate stage.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Body { pub id: usize, pub flags: u16, pub mass: f32, pub position: [f32; 3] }
#[derive(Clone, Copy, Debug)]
pub(crate) enum Command {
    PointImpulse { body: usize, point: [f32; 3], impulse: [f32; 3] },
    AngularPush { body: usize, delta: [f32; 3] },
}
#[derive(Clone, Debug)]
pub(crate) struct State {
    pub members: Vec<usize>,
    pub pairs: Vec<[usize; 2]>,
    pub ticks: i32,
    pub minimum: i32,
    pub tolerance: f32,
}
impl State {
    pub(crate) fn new(members: Vec<usize>, minimum: i32, public_tolerance: f32) -> Self {
        Self { members, pairs: Vec::new(), ticks: 0, minimum,
            tolerance: public_tolerance * f32::from_bits(0x3cd013a9) }
    }
    pub(crate) fn in_error(&self) -> bool { self.ticks >= self.minimum }
    pub(crate) fn clear_error(&mut self) { self.ticks = 0; }
    pub(crate) fn set_public_params(&mut self, minimum: i32, public_tolerance: f32) {
        self.minimum = minimum;
        self.tolerance = public_tolerance * f32::from_bits(0x3cd013a9);
    }
    pub(crate) fn queue(&mut self, first: Option<usize>, second: Option<usize>) -> bool {
        if self.pairs.len() >= 4 { return false; }
        let (Some(first),Some(second)) = (first,second) else { return false; };
        let (Some(a),Some(b)) = (self.members.iter().rposition(|&id| id == first),self.members.iter().rposition(|&id| id == second))
            else { return false; };
        // Duplicate and self pairs are retained. Native has no deduplication.
        self.pairs.push([a,b]); true
    }
    pub(crate) fn finish(&mut self, bodies: &[Body], dt: f32, gravity: [f32; 3], maximum: f32) -> Result<Vec<Command>, &'static str> {
        let commands = self.prepare(bodies, dt, gravity)?;
        self.complete(maximum);
        Ok(commands)
    }
    /// Native application occurs before the queue/counter completion below.
    pub(crate) fn prepare(&self, bodies: &[Body], dt: f32, gravity: [f32; 3]) -> Result<Vec<Command>, &'static str> {
        let mut lookup = HashMap::with_capacity(bodies.len());
        for b in bodies { if lookup.insert(b.id,b).is_some() { return Err("duplicate rescue body binding"); } }
        let members = self.members.iter().map(|id| lookup.get(id).copied().ok_or("missing rescue body binding"))
            .collect::<Result<Vec<_>,_>>()?;
        if self.pairs.len() > 4 || self.pairs.iter().flatten().any(|&i| i >= members.len()) {
            return Err("invalid native rescue pair");
        }
        let mut commands = Vec::with_capacity(self.pairs.len()*4);
        if !self.pairs.is_empty() {
            // Native x87 length uses a 53-bit significand. Its doubled length
            // and dt product are rounded to f32 before the mass multiplication.
            let [x,y,z] = gravity.map(f64::from);
            let length = ((y*y+x*x)+z*z).sqrt();
            let base = ((length+length)*f64::from(dt)) as f32;
            let mut mass = 0.;
            for b in members.iter().rev() { if b.flags & 0x12 == 0 { mass = b.mass + mass; } }
            let magnitude = mass * base;
            for &[a,b] in &self.pairs {
                let a = members[a]; let b = members[b];
                let d: [f32; 3] = std::array::from_fn(|i|b.position[i]-a.position[i]);
                let square = (d[0]*d[0]+d[1]*d[1])+d[2]*d[2];
                let (mut direction,length) = if square < f32::from_bits(0x2edbe6ff) { (d,0.) }
                    else { let inverse = 1./f64::from(square).sqrt();
                        (d.map(|v|(f64::from(v)*inverse) as f32),inverse*f64::from(square)) };
                if length <= 0.01 { direction = [1.,0.,0.]; }
                let impulse = direction.map(|v|magnitude*v);
                // Recovery eligibility differs from the mass/static mask.
                // Native tests movement byte <8 and low flag0x10 here.
                if b.flags >> 8 < 8 && b.flags & 0x10 == 0 {
                    commands.push(Command::PointImpulse { body:b.id,point:a.position,impulse });
                    commands.push(Command::AngularPush { body:b.id,delta:[dt,0.,0.] });
                }
                if a.flags >> 8 < 8 && a.flags & 0x10 == 0 {
                    commands.push(Command::PointImpulse { body:a.id,point:a.position,impulse:impulse.map(|v|v * -1.) });
                    commands.push(Command::AngularPush { body:a.id,delta:[-dt,0.,0.] });
                }
            }
        }
        Ok(commands)
    }
    pub(crate) fn complete(&mut self, maximum: f32) {
        self.pairs.clear();
        // COMISS/JBE clears on equality and unordered input. Saturation is
        // minimum+1, while the public query becomes true at minimum itself.
        if maximum > self.tolerance*self.tolerance {
            if self.ticks <= self.minimum { self.ticks = self.ticks.wrapping_add(1); }
        } else { self.ticks = 0; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value,json};
    fn f(v:&Value)->f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn command(c:Command)->Value {
        match c {
            Command::PointImpulse{body,point,impulse} => json!({"kind":0,"body":body,"point":point.map(f32::to_bits),"vector":impulse.map(f32::to_bits)}),
            Command::AngularPush{body,delta} => json!({"kind":1,"body":body,"point":Value::Null,"vector":delta.map(f32::to_bits)}),
        }
    }
    #[test]
    fn native_public_actions_counters_and_all_rescue_commands_match_across_steps() {
        let data:Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-group-tail.json")).unwrap();
        let calls=data["calls"].as_array().unwrap();let mut total=0;let mut commands=0;let mut rejected=0;
        for case in data["cases"].as_array().unwrap() {
            let scenario=case["scenario"].as_u64().unwrap();
            let expected=calls.iter().filter(|c|c["scenario"].as_u64()==Some(scenario)).collect::<Vec<_>>();
            // Object registration order is a constructor/registry input. The
            // public error parameters are authored inputs, not expected output.
            let mut state=State::new(expected[0]["before"]["members"].as_array().unwrap().iter().map(|v|v.as_u64().unwrap() as usize).collect(),(scenario%3) as i32,3.);
            for c in expected {
                let actions=&c["actions"];
                if actions["clear"]==true { state.clear_error(); }
                if actions["set_params"]==true { state.set_public_params(actions["ticks"].as_i64().unwrap() as i32,f(&actions["tolerance"])); }
                for request in actions["queue"].as_array().unwrap() {
                    let ids:[Option<usize>;2]=std::array::from_fn(|i|{let id=request[i].as_u64().unwrap();(id!=99).then_some(id as usize)});
                    rejected+=usize::from(!state.queue(ids[0],ids[1]));
                }
                assert_eq!(state.ticks,c["before"]["ticks"].as_i64().unwrap() as i32);
                assert_eq!(state.minimum,c["before"]["minimum"].as_i64().unwrap() as i32);
                assert_eq!(state.tolerance.to_bits() as u64,c["before"]["tolerance"].as_u64().unwrap());
                assert_eq!(json!(state.in_error()),c["before"]["in_error"]);
                assert_eq!(json!(state.pairs),c["before"]["pairs"]);
                let tail=&c["tail"];
                let bodies=tail["cores"].as_array().unwrap().iter().map(|b|Body { id:b["id"].as_u64().unwrap() as usize,
                    flags:b["flags"].as_u64().unwrap() as u16,mass:f(&b["mass"]),position:std::array::from_fn(|i|f(&b["position"][i])) }).collect::<Vec<_>>();
                // Post-solve body state/error is the explicit component
                // boundary. Native commands/after-counters never feed finish.
                let actual=state.finish(&bodies,f(&tail["dt"]),std::array::from_fn(|i|f(&tail["gravity"][i])),f(&tail["maximum"][0])).unwrap();
                commands+=actual.len();
                assert_eq!(json!(actual.into_iter().map(command).collect::<Vec<_>>()),c["commands"],"rescue group{scenario}/step{}",c["step"]);
                assert_eq!(state.ticks,c["after"]["ticks"].as_i64().unwrap() as i32);
                assert_eq!(json!(state.in_error()),c["after"]["in_error"]);
                assert_eq!(json!(state.pairs),c["after"]["pairs"]);
                total+=1;
            }
        }
        assert_eq!((total,commands,rejected),(120,608,160));
    }
    #[test]
    fn missing_body_or_invalid_rescue_pair_does_not_consume_queue_or_counter() {
        let body=Body{id:0,flags:0,mass:10.,position:[0.;3]};
        let mut state=State::new(vec![0,1],1,3.);state.ticks=1;assert!(state.queue(Some(0),Some(1)));
        let before=state.clone();assert!(state.finish(&[body],1./60.,[0.,9.83,0.],1.).is_err());
        assert_eq!(format!("{state:?}"),format!("{before:?}"));
        let mut state=State::new(vec![0],1,3.);state.pairs.push([0,1]);let before=state.clone();
        assert!(state.finish(&[body],1./60.,[0.,9.83,0.],1.).is_err());
        assert_eq!(format!("{state:?}"),format!("{before:?}"));
    }
}
