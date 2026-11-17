//! Fog-controller I/O and volume registration. Rendering samples transitions
//! between server ticks; completed targets are committed by the server clock.
use crate::entities::Ent;
use std::collections::{BTreeMap, HashMap};

#[derive(Default)]
pub struct FogSystem {
    pub volumes: Vec<usize>,
    pub revision: u64,
    pending: HashMap<usize, BTreeMap<String, String>>,
    transitions: HashMap<usize, Transition>,
}
#[derive(Clone, Copy)]
struct Transition { finish: f32, lerp_end: f32, duration: f32 }

fn numbers(value: &str) -> Option<Vec<f32>> {
    value.split_whitespace().map(|s| s.parse::<f32>().ok().filter(|v| v.is_finite())).collect()
}
pub fn set_key(entity: &mut Ent, key: &str, value: String) {
    entity.me.ent.0.retain(|(k,_)| !k.eq_ignore_ascii_case(key));
    entity.me.ent.0.push((key.to_lowercase(),value));
}
impl FogSystem {
    pub fn new(entities: &[Ent]) -> Self {
        let mut system = Self::default();
        for (id,entity) in entities.iter().enumerate() { system.changed(id,entity); }
        system
    }
    pub fn changed(&mut self, id: usize, entity: &Ent) {
        if !matches!(entity.me.classname.as_str(), "fog_volume" | "env_fog_controller") {return;}
        self.revision = self.revision.wrapping_add(1);
        if entity.me.classname == "fog_volume" {
            let active = entity.alive && entity.me.kv_i32("StartDisabled") != Some(1);
            if !active { self.volumes.retain(|&i| i != id); }
            else if !self.volumes.contains(&id) { self.volumes.push(id); }
        }
        if !entity.alive { self.pending.remove(&id); self.transitions.remove(&id); }
    }
    pub fn input(&mut self, id: usize, entity: &mut Ent, input: &str, param: &str, now: f32) -> bool {
        if entity.me.classname != "env_fog_controller" {return false;}
        if input == "startfogtransition" {
            let duration = entity.me.kv_f32("foglerptime").filter(|v| v.is_finite()).unwrap_or(0.).max(0.);
            // Owned server.dll 100d6100: end = curtime + duration + double .1;
            // server think commits changed fields at curtime + duration.
            self.transitions.insert(id,Transition {finish: now+duration, lerp_end: (now as f64+duration as f64+0.1) as f32,duration});
            self.revision = self.revision.wrapping_add(1);
            return true;
        }
        let key = match input {
            "setcolorlerpto" => "fogcolor", "setcolorsecondarylerpto" => "fogcolor2",
            "setstartdistlerpto" => "fogstart", "setenddistlerpto" => "fogend",
            "setmaxdensitylerpto" => "fogmaxdensity", "set2dskyboxfogfactorlerpto" => "skyboxfogfactor",
            "set2dskyboxfogfactor" => "skyboxfogfactor", "setangles" => "angles",
            _ => return false,
        };
        let Some(mut values) = numbers(param) else {return true;};
        let color = key.starts_with("fogcolor");
        if (color && !(3..=4).contains(&values.len())) || (key == "angles" && values.len() != 3)
            || (!color && key != "angles" && values.len() != 1) {return true;}
        if color { for v in &mut values { *v = v.clamp(0.,255.).floor(); } }
        let value = values.iter().map(f32::to_string).collect::<Vec<_>>().join(" ");
        if input.ends_with("lerpto") { self.pending.entry(id).or_default().insert(key.into(),value); }
        else {
            set_key(entity,key,value);
            if key == "angles" {
                entity.me.angles = glam::Vec3::from_slice(&values[..3]);
                set_key(entity,"use_angles","1".into());
            }
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }
    fn commit(&mut self, id: usize, entity: &mut Ent) {
        if let Some(values) = self.pending.remove(&id) {
            for (key,value) in values {set_key(entity,&key,value);}
        }
        self.transitions.remove(&id);
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn tick(&mut self, entities: &mut [Ent], now: f32) {
        if self.transitions.is_empty() {return;}
        let finished: Vec<_> = self.transitions.iter().filter(|(_,t)| now >= t.finish).map(|(&id,_)| id).collect();
        for id in finished { if let Some(entity) = entities.get_mut(id) { self.commit(id,entity); } }
    }
    /// Values are in Source units/colour bytes. Native FLerp deliberately has
    /// a short negative fraction before the .1-second safety margin elapses.
    pub fn sample(&self, id: usize, entity: &Ent, key: &str, now: f32) -> Option<[f32;3]> {
        // skyboxfogfactor has no keyvalue default: live retail reads 0 on every
        // Hotel controller (m_fog.skyboxFogFactor, 14 Nov probe).
        let default = if key.starts_with("fogcolor") {"255 255 255"} else if key == "fogmaxdensity" {"1"} else {"0"};
        let from = numbers(entity.me.kv(key).unwrap_or(default))?;
        let mut base=[0.;3];
        for (out,value) in base.iter_mut().zip(from) {*out=value;}
        self.sample_from(id,key,base,now)
    }
    pub fn sample_from(&self, id: usize, key: &str, from: [f32;3], now: f32) -> Option<[f32;3]> {
        let transition = self.transitions.get(&id)?;
        let to = numbers(self.pending.get(&id)?.get(key)?)?;
        let t = if now >= transition.finish || transition.duration <= 0. {1.}
            else {1.-(transition.lerp_end-now)/transition.duration};
        let mut out = [0.;3];
        for (i,(&a,&b)) in from.iter().zip(&to).take(3).enumerate() {out[i]=a+(b-a)*t;}
        Some(out)
    }
}
