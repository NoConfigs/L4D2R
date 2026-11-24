//! func_precipitation's particle types (L4D2 CClient_Precipitation): rain,
//! ash, rainstorm, bugs and smoke drawn around the player while the eye is
//! inside a volume of that type. Their PCFs read the player from control
//! point 2 (Position Within Sphere Random, Movement Lock to Control Point),
//! so each type is one runtime info_particle_system whose control points
//! and origin follow the camera.
use bevy::prelude::*;
use l4d2r_game_core::{
    entities::{Ent, Kind},
    mapents::MapEnt,
};

/// Effect per func_precipitation "preciptype" (4-8, the particle types).
fn effect(kind: u32) -> Option<&'static str> {
    Some(match kind {
        4 => "rain",
        5 => "ash",
        6 => "rain_storm",
        7 => "bugs_precipitation",
        8 => "smoke_precipitation",
        _ => return None,
    })
}

const CONTROL: &str = "__rewrite_precip_cp";
/// CP1: the near emission point above and ahead of the eye
/// (C_Precipitation's clamped offset), where rain and its mist start.
const AHEAD: &str = "__rewrite_precip_ahead";
const AHEAD_UP: f32 = 180.;
const AHEAD_FORWARD: f32 = 120.;
pub(crate) const PREFIX: &str = "__rewrite_precip_";

#[derive(Resource, Default)]
pub struct Volumes {
    /// (volume entity, emitter entity) per func_precipitation.
    list: Vec<(usize, usize)>,
}

fn runtime(logic: &mut crate::l4d2r_cinematic::MapLogic, classname: &str, name: &str, keys: Vec<(String, String)>, kind: Kind) -> usize {
    if let Some(&id) = logic.world.find(name).first() {
        return id;
    }
    let index = logic.world.ents.len();
    let mut ent = vec![("classname".into(), classname.into()), ("targetname".into(), name.into())];
    ent.extend(keys);
    logic.world.add_runtime_entity(Ent {
        me: MapEnt {
            index,
            classname: classname.into(),
            targetname: name.into(),
            origin: l4d2r_math::Vec3::ZERO,
            angles: l4d2r_math::Vec3::ZERO,
            mdl: None,
            brush: None,
            ent: l4d2_formats::bsp::Entity(ent),
        },
        conns: vec![],
        kind,
        alive: true,
        templated: false,
        parent: None,
    });
    index
}

pub fn setup(mut commands: Commands, mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>) {
    let volumes: Vec<(usize, u32)> = logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.alive && e.me.classname == "func_precipitation" && e.me.brush.is_some())
        .filter_map(|(id, e)| e.me.kv("preciptype").and_then(|v| v.trim().parse().ok()).map(|k| (id, k)))
        .filter(|(_, k)| effect(*k).is_some())
        .collect();
    let mut list = vec![];
    if !volumes.is_empty() {
        runtime(&mut logic, "info_particle_target", CONTROL, vec![], Kind::None);
        runtime(&mut logic, "info_particle_target", AHEAD, vec![], Kind::None);
    }
    for (volume, kind) in volumes {
        let fx = effect(kind).unwrap();
        let keys = (1..=5)
            .map(|i| (format!("cpoint{i}"), if i == 1 { AHEAD } else { CONTROL }.to_string()))
            .chain([("effect_name".into(), fx.to_string())])
            .collect();
        let emitter = runtime(&mut logic, "info_particle_system", &format!("{PREFIX}{fx}"), keys, Kind::Particle { active: false, started: 0. });
        list.push((volume, emitter));
    }
    if !list.is_empty() {
        println!("Precipitation: {} volume(s)", list.len());
    }
    commands.insert_resource(Volumes { list });
}

pub fn update(
    volumes: Option<Res<Volumes>>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
) {
    let forward = camera.iter().next().map(|c| c.forward().as_vec3()).unwrap_or(Vec3::NEG_Z);
    let Some(volumes) = volumes else { return };
    if volumes.list.is_empty() {
        return;
    }
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    let source = l4d2r_math::Vec3::new(eye.x, -eye.z, eye.y) / 0.0254;
    let now = logic.world.time;
    if let Some(&cp) = logic.world.find(CONTROL).first() {
        logic.world.ents[cp].me.origin = source;
    }
    if let Some(&cp) = logic.world.find(AHEAD).first() {
        let flat = l4d2r_math::Vec3::new(forward.x, -forward.z, 0.).normalize_or_zero();
        logic.world.ents[cp].me.origin = source + flat * AHEAD_FORWARD + l4d2r_math::Vec3::new(0., 0., AHEAD_UP);
    }
    // One emitter can serve several volumes of its type: on while the eye
    // is inside any of them, at the densest one's density. Density is the
    // volume's render alpha / 100 (C_Precipitation); the storm maps start
    // their "rain" at 0 and raise it with Alpha inputs.
    let mut density: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
    for &(volume, emitter) in &volumes.list {
        let within = logic.world.ents[volume].alive && logic.bounds(volume).is_some_and(|(lo, hi)| {
            let (lo, hi) = (Vec3::from_array(lo.to_array()), Vec3::from_array(hi.to_array()));
            eye.cmpge(lo.min(hi)).all() && eye.cmple(lo.max(hi)).all()
        });
        let amount = if within { logic.world.ents[volume].me.kv_f32("renderamt").unwrap_or(100.) / 100. } else { 0. };
        let d = density.entry(emitter).or_default();
        *d = d.max(amount);
    }
    for (emitter, amount) in density {
        let e = &mut logic.world.ents[emitter];
        e.me.origin = source;
        e.me.ent.0.retain(|(k, _)| k != "precip_density");
        e.me.ent.0.push(("precip_density".into(), amount.to_string()));
        let on = amount > 0.;
        let active = matches!(e.kind, Kind::Particle { active: true, .. });
        if on != active {
            e.kind = Kind::Particle { active: on, started: now };
        }
    }
}
