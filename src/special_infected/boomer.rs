//! Playable Boomer: vomit stream, "IT" survivors (bile) and the death blast.
//!
//! Retail sources (see local-research/si-agents/boomer/NOTES.md):
//! running L4D2 build 10097 cvars and owned server.dll pseudocode
//! (CVomit FUN_103722f0/FUN_103728a0, OnVomitedUpon FUN_10314f80, IT mob
//! FUN_104abb20, death blast FUN_10336010/FUN_103824e0, stagger
//! FUN_1032ce80, player max speed FUN_102c9940).
//!
//! The shared lifecycle (team switch, ghost spawn, death/respawn,
//! survivor-bot opposition) lives in special_infected.rs; this module runs
//! after `super::tick`.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, DUCK_HEIGHT, HEIGHT, RADIUS, UNIT},
    game::Body,
    game_state::{Match, Squad},
};
use bevy::prelude::*;
use l4d2r_game_core::{
    entities::{Ent, EntityWorld, Kind as EntityKind},
    mapents::MapEnt,
};
use std::sync::atomic::{AtomicBool, Ordering};
pub const WEAPON: &str = "weapon_boomer_claw";
pub const MODEL: &str = "models/infected/boomer.mdl";
/// Retail z_female_boomer_spawn_chance=25 picks this for infected players
/// (CTerrorPlayer model selection FUN_1031a3b0); it only changes $gender here.
#[allow(dead_code)]
pub const FEMALE_MODEL: &str = "models/infected/boomette.mdl";
pub const VIEW: &str = "models/v_models/weapons/v_claw_boomer.mdl";
/// Retail z_exploding_health.
pub const HEALTH: f32 = 50.;
/// Retail z_exploding_speed.
pub const SPEED: f32 = 175.;
pub const SOUND: &str = "BoomerZombie";
pub const PLAYABLE: bool = true;
pub const VM_ACTIONS: &[(&str, &str)] = &[("vomit", "ACT_VM_VOMIT")];
pub const CONTROLS: &str =
    "Boomer playtest: left click vomits (30s recharge); right click claws; dying explodes.";
/// z_vomit_duration: seconds of continuous puking.
pub const VOMIT_DURATION: f32 = 1.5;
/// z_vomit_interval: the ability timer restarts at the start and end of a puke.
pub const VOMIT_INTERVAL: f32 = 30.;
/// z_vomit_fatigue=3000 (stamina ms, set every puking tick, recovers 1000/s).
pub const VOMIT_FATIGUE: f32 = 3.;
/// FUN_102c9940: a Boomer with stamina above 2500ms has max speed 1.
pub const FATIGUE_LIMIT: f32 = 2.5;
/// FUN_103728a0 per-tick traced vomit: forward*2100 + RandomVector(+-z_vomit_vecrand*2100).
pub const STREAM_SPEED: f32 = 2100.;
pub const STREAM_VECRAND: f32 = 0.05;
/// Per 30Hz step: velocity *= 0.86, then 110 units/s downward.
pub const STREAM_DRAG: f32 = 0.86;
pub const STREAM_FALL: f32 = 110.;
/// Trace lifetime RandomFloat(0.9, 1.1) * 0.5 seconds.
pub const STREAM_LIFE: f32 = 0.5;
pub const RETAIL_TICK: f32 = 1. / 30.;
/// survivor_it_duration.
pub const IT_DURATION: f32 = 20.;
/// z_notice_it_range: wanderers within this range go after an IT survivor.
pub const NOTICE_IT_RANGE: f32 = 1500.;
/// sb_vomit_blind_time: survivor bots cannot see for this long after bile.
pub const BLIND_TIME: f32 = 5.;
/// z_exploding_outer_radius (hidden cvar, default 200). FUN_10336010's
/// inner radius (outer * 0.9 on Normal) only scales the push force; any
/// exposed survivor inside the outer radius is biled and staggered.
pub const EXPLODE_OUTER: f32 = 200.;
/// survivor_max_tongue_stagger_duration / _distance used by FUN_1032ce80.
pub const STAGGER_DURATION: f32 = 1.5;
pub const STAGGER_DISTANCE: f32 = 200.;
/// boomer_pz_claw_dmg.
pub const CLAW_DAMAGE: f32 = 4.;
/// z_female_boomer_spawn_chance (percent).
pub const FEMALE_CHANCE: u32 = 25;
const STREAM_FX: &str = "boomer_vomit";
const EXPLODE_FX: &str = "boomer_explode";
const IT_FX: &str = "boomer_vomit_survivor";
const EXPLODE_SLOTS: usize = 2;
const IT_SLOTS: usize = 4;
static FEMALE: AtomicBool = AtomicBool::new(false);
/// Voice cue with the current Boomer/Boomette `$gender` (see game_audio).
pub fn voice(name: &str) -> String {
    if FEMALE.load(Ordering::Relaxed) {
        format!("{name}#female")
    } else {
        name.into()
    }
}
/// Shared lifecycle cues. Retail has no BoomerZombie.Growl/Death: the idle
/// voice (Groan) and the Detonate blast are played by this module.

/// Viewmodel activity override per special_phase; None keeps the default.
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    (phase == 2).then_some("ACT_VM_VOMIT")
}
#[derive(Resource)]
pub struct Boomer {
    /// Seconds of vomit stream left.
    pub vomit: f32,
    /// Ability timer (z_vomit_interval) remaining.
    pub recharge: f32,
    /// Stamina seconds; above FATIGUE_LIMIT the Boomer cannot move.
    pub fatigue: f32,
    pub female: bool,
    pub stream_hits: u32,
    claw: f32,
    rolled: bool,
    exploded: bool,
    explode_fx: Vec<(usize, f32)>,
    rng: u32,
    capture_clock: f32,
}
impl Default for Boomer {
    fn default() -> Self {
        Self {
            vomit: 0.,
            recharge: 0.,
            fatigue: 0.,
            female: false,
            stream_hits: 0,
            claw: 0.,
            rolled: false,
            exploded: false,
            explode_fx: vec![],
            rng: 0x9e37_79b9,
            capture_clock: 0.,
        }
    }
}
impl Boomer {
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }
    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.random()
    }
    /// HUD ability meter (0..1).
    pub fn progress(&self) -> f32 {
        if self.vomit > 0. {
            0.
        } else {
            (1. - self.recharge / VOMIT_INTERVAL).clamp(0., 1.)
        }
    }
    /// Starts a puke when the ability timer allows it.
    pub fn try_vomit(&mut self) -> bool {
        if self.vomit > 0. || self.recharge > 0. {
            return false;
        }
        self.vomit = VOMIT_DURATION;
        self.recharge = VOMIT_INTERVAL;
        self.fatigue = VOMIT_FATIGUE;
        true
    }
    /// Advances timers; returns true on the tick the stream ends.
    pub fn advance(&mut self, dt: f32) -> bool {
        self.claw = (self.claw - dt).max(0.);
        if self.vomit > 0. {
            self.fatigue = VOMIT_FATIGUE;
            self.vomit -= dt;
            if self.vomit <= 1e-4 {
                self.vomit = 0.;
                // FUN_103728a0 restarts the ability timer when the puke ends.
                self.recharge = VOMIT_INTERVAL;
                return true;
            }
            return false;
        }
        self.recharge = (self.recharge - dt).max(0.);
        self.fatigue = (self.fatigue - dt).max(0.);
        false
    }
    /// Max speed collapses to 1 unit/s while stamina exceeds 2500ms.
    pub fn frozen(&self) -> bool {
        self.fatigue > FATIGUE_LIMIT + 1e-4
    }
}
/// One traced vomit "particle" in rewrite metres: (from, to) segments.
pub fn stream_segments(eye: Vec3, forward: Vec3, spread: Vec3, life: f32) -> Vec<(Vec3, Vec3)> {
    let mut velocity = forward.normalize_or_zero() * STREAM_SPEED + spread;
    let mut previous = eye;
    let mut current = eye + velocity * RETAIL_TICK * UNIT;
    let mut t = RETAIL_TICK;
    let mut segments = vec![];
    while t <= life + 1e-6 {
        velocity = velocity * STREAM_DRAG - Vec3::Y * STREAM_FALL;
        segments.push((previous, current));
        previous = current;
        current += velocity * RETAIL_TICK * UNIT;
        t += RETAIL_TICK;
    }
    segments
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StreamHit {
    Survivor(usize),
    World(Vec3),
}
fn hull(m: &crate::game_state::Companion) -> (Vec3, Vec3) {
    let height = if m.state.incap { DUCK_HEIGHT } else { HEIGHT };
    (
        m.position - Vec3::new(RADIUS, 0., RADIUS),
        m.position + Vec3::new(RADIUS, height, RADIUS),
    )
}
fn segment_box(from: Vec3, to: Vec3, (min, max): (Vec3, Vec3)) -> Option<f32> {
    let delta = to - from;
    let (mut enter, mut exit) = (0_f32, 1_f32);
    for axis in 0..3 {
        if delta[axis].abs() < 1e-7 {
            if from[axis] < min[axis] || from[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - from[axis]) / delta[axis];
            let b = (max[axis] - from[axis]) / delta[axis];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return None;
            }
        }
    }
    Some(enter)
}
/// First survivor or world contact along the traced stream.
pub fn stream_hit(
    collision: &Collision,
    segments: &[(Vec3, Vec3)],
    squad: &Squad,
) -> Option<StreamHit> {
    for &(from, to) in segments {
        let world = collision.trace(from, to, Vec3::ZERO);
        let wall = if world.start_solid {
            0.
        } else {
            world.fraction
        };
        let survivor = squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.state.dead)
            .filter_map(|(i, m)| segment_box(from, to, hull(m)).map(|f| (i, f)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = survivor.filter(|(_, f)| *f <= wall) {
            return Some(StreamHit::Survivor(i));
        }
        if wall < 1. {
            return Some(StreamHit::World(from + (to - from) * wall));
        }
    }
    None
}
/// Survivors within the blast's outer radius with a clear line from its centre.
pub fn blast_victims(collision: &Collision, center: Vec3, squad: &Squad) -> Vec<usize> {
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .filter(|(_, m)| {
            let (min, max) = hull(m);
            center.clamp(min, max).distance(center) <= EXPLODE_OUTER * UNIT
                && collision
                    .trace(center, (min + max) * 0.5, Vec3::ZERO)
                    .fraction
                    >= 0.99
        })
        .map(|(i, _)| i)
        .collect()
}
/// Source yaw (degrees) of a rewrite-space horizontal direction.
fn source_yaw(v: Vec3) -> f32 {
    (-v.z).atan2(v.x).to_degrees()
}
/// FUN_1032ce80 chooses the stagger animation from the angle between the
/// survivor's facing and the push direction (+-45/135 degree sectors).
pub fn stagger_activity(facing: Vec3, push: Vec3) -> &'static str {
    let mut relative = source_yaw(facing) - source_yaw(push);
    while relative > 180. {
        relative -= 360.;
    }
    while relative < -180. {
        relative += 360.;
    }
    if relative.abs() < 45. {
        "ACT_TERROR_SHOVED_FORWARD"
    } else if relative > 45. && relative < 135. {
        "ACT_TERROR_SHOVED_RIGHTWARD"
    } else if relative < -45. && relative > -135. {
        "ACT_TERROR_SHOVED_LEFTWARD"
    } else {
        "ACT_TERROR_SHOVED_BACKWARD"
    }
}
/// Stagger travel speed (m/s) for the remaining time: a linear decay whose
/// integral over STAGGER_DURATION is STAGGER_DISTANCE.
pub fn stagger_speed(remaining: f32) -> f32 {
    2. * STAGGER_DISTANCE * UNIT / STAGGER_DURATION * (remaining / STAGGER_DURATION).clamp(0., 1.)
}
/// Nearest living IT survivor within z_notice_it_range of a common.
pub fn it_target(squad: &Squad, from: Vec3) -> Option<(usize, Vec3)> {
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead && m.state.vomited > 0.)
        .map(|(i, m)| (i, m.position, m.position.distance(from)))
        .filter(|(_, _, d)| *d <= NOTICE_IT_RANGE * UNIT)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, p, _)| (i, p))
}
/// OnVomitedUpon: (re)starts IT for survivor_it_duration. Returns true for a
/// survivor who was not already IT (retail then grabs wanderers/calls a mob).
pub fn bile(member: &mut crate::game_state::Companion) -> bool {
    let fresh = member.state.vomited <= 0.;
    member.state.vomited = IT_DURATION;
    fresh
}
/// Blind/stagger hold, timers and stagger travel for every survivor bot.
pub fn advance_survivor(member: &mut crate::game_state::Companion, collision: &Collision, dt: f32) {
    if member.state.dead {
        member.state.vomited = 0.;
        member.state.stagger = 0.;
        return;
    }
    if member.state.vomited > 0. {
        let blind = member.state.vomited - (IT_DURATION - BLIND_TIME);
        if blind > 0. {
            member.cooldown = member.cooldown.max(blind);
        }
        member.state.vomited = (member.state.vomited - dt).max(0.);
    }
    if member.state.stagger > 0. {
        if !member.state.incap && !member.state.pinned {
            let speed = stagger_speed(member.state.stagger - dt * 0.5);
            let delta = member.state.stagger_dir * speed * dt;
            member.position = collision.walk(
                member.position,
                delta,
                HEIGHT,
                collision.grounded(member.position, HEIGHT),
            );
        }
        member.cooldown = member.cooldown.max(member.state.stagger);
        member.state.stagger = (member.state.stagger - dt).max(0.);
    }
}
fn fx_name(kind: &str, slot: usize) -> String {
    format!("__rewrite_boomer_fx_{kind}_{slot}")
}
/// Pooled native PCF emitters (boomer_fx.pcf), created with the map world.
pub fn prepare(world: &mut EntityWorld) {
    for (kind, effect, count) in [
        ("stream", STREAM_FX, 1),
        ("explode", EXPLODE_FX, EXPLODE_SLOTS),
        ("it", IT_FX, IT_SLOTS),
    ] {
        for slot in 0..count {
            let name = fx_name(kind, slot);
            if !world.find(&name).is_empty() {
                continue;
            }
            let index = world.ents.len();
            world.add_runtime_entity(Ent {
                me: MapEnt {
                    index,
                    classname: "info_particle_system".into(),
                    targetname: name.clone(),
                    origin: l4d2r_math::Vec3::ZERO,
                    angles: l4d2r_math::Vec3::ZERO,
                    mdl: None,
                    brush: None,
                    ent: l4d2_formats::bsp::Entity(vec![
                        ("classname".into(), "info_particle_system".into()),
                        ("targetname".into(), name),
                        ("effect_name".into(), effect.to_string()),
                    ]),
                },
                conns: vec![],
                kind: EntityKind::Particle {
                    active: false,
                    started: 0.,
                },
                alive: true,
                templated: false,
                parent: None,
            });
        }
    }
}
fn source_origin(p: Vec3) -> l4d2r_math::Vec3 {
    l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT
}
/// Activates (restart) / moves (keep) / stops (None) a pooled emitter.
fn set_fx(
    world: &mut EntityWorld,
    kind: &str,
    slot: usize,
    at: Option<(Vec3, Vec3)>,
    restart: bool,
) -> Option<usize> {
    let id = world.find(&fx_name(kind, slot)).first().copied()?;
    let time = world.time;
    let e = world.ents.get_mut(id)?;
    match at {
        Some((position, forward)) => {
            e.me.origin = source_origin(position);
            if forward != Vec3::ZERO {
                let f = forward.normalize();
                e.me.angles = l4d2r_math::Vec3::new(
                    -f.y.clamp(-1., 1.).asin().to_degrees(),
                    source_yaw(f),
                    0.,
                );
            }
            let started = match e.kind {
                EntityKind::Particle {
                    active: true,
                    started,
                } if !restart => started,
                _ => time,
            };
            e.kind = EntityKind::Particle {
                active: true,
                started,
            };
        }
        None => {
            if matches!(e.kind, EntityKind::Particle { active: true, .. }) {
                e.kind = EntityKind::Particle {
                    active: false,
                    started: 0.,
                };
            }
        }
    }
    Some(id)
}
pub fn install(app: &mut App) {
    app.init_resource::<Boomer>()
        .add_systems(FixedUpdate, tick.after(super::tick));
}
/// Marks a survivor IT and, on a fresh bile, runs the retail IT mob call.
fn vomit_member(
    squad: &mut Squad,
    index: usize,
    enemies: &Query<(&Transform, &crate::game::Enemy)>,
    director: &mut Option<ResMut<crate::source_director::Director>>,
) {
    let Some(member) = squad.0.get_mut(index) else {
        return;
    };
    if bile(member) {
        let grabbed = enemies
            .iter()
            .filter(|(t, e)| {
                e.health > 0. && t.translation.distance(member.position) <= NOTICE_IT_RANGE * UNIT
            })
            .count();
        if let Some(director) = director.as_mut() {
            // The Director's own spawn loop announces the queued mob.
            let queued = director.it_mob(grabbed);
            println!(
                "Boomer IT: survivor {index} vomited; {grabbed} wanderers grabbed, {queued} mob queued"
            );
        }
    }
}
pub fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut boomer: ResMut<Boomer>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    enemies: Query<(&Transform, &crate::game::Enemy)>,
    mut director: Option<ResMut<crate::source_director::Director>>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    // Survivor-side bile/stagger effects outlive the Boomer that caused them.
    {
        let survivors = collision.for_actor(false, false);
        for member in &mut squad.0 {
            advance_survivor(member, &survivors, dt);
        }
    }
    let collision = collision.for_actor(true, hunter.ghost);
    if let Some(logic) = logic.as_mut() {
        for (slot, member) in squad.0.iter().take(IT_SLOTS).enumerate() {
            let at = (member.state.vomited > 0. && !member.state.dead)
                .then_some((member.position + Vec3::Y * HEIGHT * 0.5, Vec3::ZERO));
            set_fx(&mut logic.world, "it", slot, at, false);
        }
        for (id, left) in &mut boomer.explode_fx {
            *left -= dt;
            if *left <= 0. {
                if let Some(e) = logic.world.ents.get_mut(*id) {
                    e.kind = EntityKind::Particle {
                        active: false,
                        started: 0.,
                    };
                }
            }
        }
    }
    boomer.explode_fx.retain(|(_, left)| *left > 0.);
    let mine = state.infected && state.special_class == SpecialClass::Boomer;
    // Opt-in render fixture: spawn at once with the squad ahead, puke at
    // 0.5s and die at 3.5s (exercises stream, IT, blast and stagger).
    let capture = mine && std::env::args().any(|a| a == "--capture-boomer");
    if capture && hunter.ghost {
        hunter.ghost = false;
        state.special_phase = 0;
        let ahead = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let side = Quat::from_rotation_y(body.yaw) * Vec3::X;
        for (i, member) in squad.0.iter_mut().enumerate() {
            member.position = body.feet + ahead * (3. + i as f32 * 0.6) + side * (i as f32 - 1.5);
            member.cooldown = 10.;
        }
    }
    if !mine || hunter.ghost {
        boomer.vomit = 0.;
        boomer.fatigue = 0.;
        boomer.exploded = false;
        if let Some(logic) = logic.as_mut() {
            set_fx(&mut logic.world, "stream", 0, None, false);
        }
        if mine && !boomer.rolled {
            // Retail rolls the Boomette model per spawn (no mission flag here).
            boomer.female = boomer.random() * 100. < FEMALE_CHANCE as f32;
            FEMALE.store(boomer.female, Ordering::Relaxed);
            boomer.rolled = true;
        }
        if mine {
            hunter.ability_progress = Some(boomer.progress());
        }
        return;
    }
    boomer.rolled = false;
    let eye = body.feet + Vec3::Y * body.stance.eye();
    if state.dead {
        boomer.vomit = 0.;
        if let Some(logic) = logic.as_mut() {
            set_fx(&mut logic.world, "stream", 0, None, false);
        }
        if !boomer.exploded {
            boomer.exploded = true;
            let center = body.feet + Vec3::Y * HEIGHT * 0.5;
            sounds.play(&format!("{SOUND}.Detonate"), Some(center));
            if let Some(logic) = logic.as_mut() {
                let used = boomer.explode_fx.iter().map(|f| f.0).collect::<Vec<_>>();
                let slot = (0..EXPLODE_SLOTS)
                    .find(|s| {
                        logic
                            .world
                            .find(&fx_name("explode", *s))
                            .first()
                            .is_some_and(|id| !used.contains(id))
                    })
                    .unwrap_or(0);
                if let Some(id) = set_fx(
                    &mut logic.world,
                    "explode",
                    slot,
                    Some((center, Vec3::ZERO)),
                    true,
                ) {
                    boomer.explode_fx.push((id, 3.));
                }
            }
            let victims = blast_victims(&collision, center, &squad);
            for &i in &victims {
                vomit_member(&mut squad, i, &enemies, &mut director);
                let m = &mut squad.0[i];
                let mut push = (m.position - center) * Vec3::new(1., 0., 1.);
                if push.length_squared() < 1e-6 {
                    push = -m.aim * Vec3::new(1., 0., 1.);
                }
                m.state.stagger = STAGGER_DURATION;
                m.state.stagger_dir = push.normalize_or_zero();
            }
            println!(
                "Boomer exploded: {} survivors biled/staggered",
                victims.len()
            );
        }
        hunter.ability_progress = Some(boomer.progress());
        return;
    }
    boomer.exploded = false;
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    boomer.capture_clock += dt;
    let scripted = capture && boomer.capture_clock >= 0.5 && boomer.capture_clock < 0.6;
    if capture && boomer.capture_clock >= 3.5 {
        state.damage(HEALTH * 2.);
    }
    let fire = std::mem::take(&mut input.fire) || input.fire_held || scripted;
    let claw = std::mem::take(&mut input.shove) || input.shove_held;
    input.reload = false;
    input.use_item = false;
    let ended = boomer.advance(dt);
    if fire && boomer.try_vomit() {
        sounds.play(voice("Vomit.Use"), None);
        state.shot_serial = state.shot_serial.wrapping_add(1);
        if let Some(logic) = logic.as_mut() {
            set_fx(
                &mut logic.world,
                "stream",
                0,
                Some((eye + forward * 0.3 - Vec3::Y * 0.1, forward)),
                true,
            );
        }
    }
    if boomer.vomit > 0. {
        let spread = Vec3::new(
            boomer.between(-1., 1.),
            boomer.between(-1., 1.),
            boomer.between(-1., 1.),
        ) * STREAM_SPEED
            * STREAM_VECRAND;
        let life = STREAM_LIFE * boomer.between(0.9, 1.1);
        let segments = stream_segments(eye, forward, spread, life);
        if let Some(StreamHit::Survivor(i)) = stream_hit(&collision, &segments, &squad) {
            boomer.stream_hits += 1;
            vomit_member(&mut squad, i, &enemies, &mut director);
        }
        if let Some(logic) = logic.as_mut() {
            set_fx(
                &mut logic.world,
                "stream",
                0,
                Some((eye + forward * 0.3 - Vec3::Y * 0.1, forward)),
                false,
            );
        }
    } else if ended {
        if let Some(logic) = logic.as_mut() {
            set_fx(&mut logic.world, "stream", 0, None, false);
        }
    }
    if boomer.frozen() {
        state.special_phase = 2;
        body.velocity.x = 0.;
        body.velocity.z = 0.;
    } else if state.special_phase == 2 {
        state.special_phase = 0;
    }
    if state.special_phase != 2 && claw && boomer.claw <= 0. {
        let victim = squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.state.dead)
            .find(|(_, m)| {
                let delta = m.position + Vec3::Y - eye;
                delta.length() < 75. * UNIT
                    && forward.dot(delta.normalize_or_zero()) > 0.6
                    && collision
                        .trace(eye, m.position + Vec3::Y, Vec3::ZERO)
                        .fraction
                        >= 1.
            })
            .map(|(i, _)| i);
        // Retail: Claw.Swing every swing, the impact sound on contact, and
        // BoomerZombie.Attack through the voice timer (`voice`).
        sounds.play("Claw.Swing", None);
        super::voice::clawed(&mut hunter);
        if let Some(i) = victim {
            squad.0[i].state.damage(CLAW_DAMAGE);
            sounds.play("Claw.HitFlesh", None);
        } else if logic
            .as_mut()
            .is_some_and(|l| l.pound_obstacle(body.feet, forward, &collision))
        {
            sounds.play("Claw.Hit", None);
        }
        boomer.claw = 0.6;
        state.special_phase = 4;
        state.shot_serial = state.shot_serial.wrapping_add(1);
    } else if state.special_phase == 4 && boomer.claw <= 0. {
        state.special_phase = 0;
    }
    // Idle Groan/Voice by mood: see `voice`. Quiet while puking.
    if boomer.vomit > 0. {
        super::voice::hold(&mut hunter, boomer.vomit);
    }
    hunter.ability_progress = Some(boomer.progress());
}
#[cfg(test)]
mod tests {
    use super::*;
    fn member(position: Vec3) -> crate::game_state::Companion {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.dead = false;
        crate::game_state::Companion {
            state,
            position,
            spawn: position,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::X,
        }
    }
    #[test]
    fn retail_boomer_constants_match_running_game_cvars() {
        assert_eq!(HEALTH, 50.);
        assert_eq!(SPEED, 175.);
        assert_eq!(SpecialClass::Boomer.health(), 50.);
        assert_eq!(SpecialClass::Boomer.speed(), 175.);
        assert!(SpecialClass::Boomer.playable());
        assert_eq!(VOMIT_DURATION, 1.5);
        assert_eq!(VOMIT_INTERVAL, 30.);
        assert_eq!(IT_DURATION, 20.);
        assert_eq!(vm_activity(2), Some("ACT_VM_VOMIT"));
        assert_eq!(vm_activity(4), None);
    }
    #[test]
    fn vomit_freezes_for_fatigue_and_recharges_from_the_end_of_the_puke() {
        let mut b = Boomer::default();
        assert_eq!(b.progress(), 1.);
        assert!(b.try_vomit());
        assert!(!b.try_vomit());
        let dt = 1. / 30.;
        let mut t = 0.;
        let mut ended = None;
        let mut thawed = None;
        while t < 40. {
            if b.advance(dt) {
                ended = Some(t + dt);
            }
            t += dt;
            if ended.is_some() && thawed.is_none() && !b.frozen() {
                thawed = Some(t);
            }
            if b.recharge <= 0. && b.vomit <= 0. {
                break;
            }
        }
        let ended = ended.unwrap();
        assert!((ended - 1.5).abs() < 0.04, "{ended}");
        // Stamina 3000ms decays 1000/s to the 2500ms speed threshold.
        assert!((thawed.unwrap() - 2.0).abs() < 0.05, "{thawed:?}");
        assert!((t - 31.5).abs() < 0.05, "{t}");
        assert_eq!(b.progress(), 1.);
        assert!(b.try_vomit());
    }
    #[test]
    fn traced_stream_reaches_about_450_units_and_drops() {
        let segs = stream_segments(Vec3::ZERO, Vec3::NEG_Z, Vec3::ZERO, STREAM_LIFE);
        assert_eq!(segs.len(), 15);
        let end = segs.last().unwrap().1;
        let reach = -end.z / UNIT;
        assert!(reach > 430. && reach < 470., "{reach}");
        assert!(end.y < -20. * UNIT);
        let short = stream_segments(Vec3::ZERO, Vec3::NEG_Z, Vec3::ZERO, 0.45);
        assert!(short.len() < segs.len());
    }
    #[test]
    fn stream_bile_hits_first_survivor_and_walls_block_it() {
        let mut squad = Squad(vec![
            member(Vec3::new(0., 0., -5.)),
            member(Vec3::new(0., 0., -8.)),
        ]);
        let free = Collision::default();
        let eye = Vec3::Y * 1.5;
        let segs = stream_segments(eye, Vec3::NEG_Z, Vec3::ZERO, STREAM_LIFE);
        assert_eq!(
            stream_hit(&free, &segs, &squad),
            Some(StreamHit::Survivor(0))
        );
        squad.0[0].state.dead = true;
        assert_eq!(
            stream_hit(&free, &segs, &squad),
            Some(StreamHit::Survivor(1))
        );
        let mut walled = Collision::test_solid_floor();
        walled.test_add_box(Vec3::new(-3., 0., -3.2), Vec3::new(3., 4., -3.));
        assert!(matches!(
            stream_hit(&walled, &segs, &squad),
            Some(StreamHit::World(_))
        ));
        // Out of reach (about 11 m).
        let far = Squad(vec![member(Vec3::new(0., 0., -14.))]);
        assert_eq!(stream_hit(&free, &segs, &far), None);
    }
    #[test]
    fn bile_marks_it_attracts_commons_and_blinds_bots() {
        let mut squad = Squad(vec![member(Vec3::ZERO), member(Vec3::X * 3.)]);
        assert!(it_target(&squad, Vec3::X * 10.).is_none());
        assert!(bile(&mut squad.0[0]));
        assert!(!bile(&mut squad.0[0]));
        assert_eq!(it_target(&squad, Vec3::X * 10.).unwrap().0, 0);
        assert!(it_target(&squad, Vec3::X * (NOTICE_IT_RANGE * UNIT + 1.)).is_none());
        let c = Collision::test_solid_floor();
        let dt = 1. / 30.;
        advance_survivor(&mut squad.0[0], &c, dt);
        assert!(squad.0[0].cooldown > 4.9);
        for _ in 0..(6. / dt) as usize {
            squad.0[0].cooldown = (squad.0[0].cooldown - dt).max(0.);
            advance_survivor(&mut squad.0[0], &c, dt);
        }
        // Blind (sb_vomit_blind_time) is over, IT continues.
        assert!(squad.0[0].cooldown < 0.1);
        assert!(squad.0[0].state.vomited > 13.);
        for _ in 0..(15. / dt) as usize {
            advance_survivor(&mut squad.0[0], &c, dt);
        }
        assert_eq!(squad.0[0].state.vomited, 0.);
        assert!(it_target(&squad, Vec3::ZERO).is_none());
    }
    #[test]
    fn blast_biles_and_staggers_only_exposed_survivors_in_radius() {
        let squad = Squad(vec![
            member(Vec3::new(4., 0., 0.)),
            member(Vec3::new(6., 0., 0.)),
            member(Vec3::new(-4., 0., 0.)),
        ]);
        let mut c = Collision::test_solid_floor();
        c.test_add_box(Vec3::new(-2.2, 0., -3.), Vec3::new(-2., 4., 3.));
        let victims = blast_victims(&c, Vec3::Y * HEIGHT * 0.5, &squad);
        assert_eq!(victims, vec![0]);
        let mut m = member(Vec3::ZERO);
        m.state.stagger = STAGGER_DURATION;
        m.state.stagger_dir = Vec3::X;
        let free = Collision::test_solid_floor();
        let dt = 1. / 30.;
        while m.state.stagger > 0. {
            advance_survivor(&mut m, &free, dt);
        }
        let travelled = m.position.x / UNIT;
        assert!((travelled - STAGGER_DISTANCE).abs() < 8., "{travelled}");
    }
    #[test]
    fn stagger_direction_uses_retail_sectors() {
        assert_eq!(
            stagger_activity(Vec3::X, Vec3::X),
            "ACT_TERROR_SHOVED_FORWARD"
        );
        assert_eq!(
            stagger_activity(Vec3::X, -Vec3::X),
            "ACT_TERROR_SHOVED_BACKWARD"
        );
        // Facing +X (Source yaw 0), pushed toward rewrite +Z = Source -Y (right).
        assert_eq!(
            stagger_activity(Vec3::X, Vec3::Z),
            "ACT_TERROR_SHOVED_RIGHTWARD"
        );
        assert_eq!(
            stagger_activity(Vec3::X, -Vec3::Z),
            "ACT_TERROR_SHOVED_LEFTWARD"
        );
    }
    #[test]
    fn boomer_voices_resolve_gender() {
        FEMALE.store(false, Ordering::Relaxed);
        assert_eq!(voice("BoomerZombie.Pain"), "BoomerZombie.Pain");
        FEMALE.store(true, Ordering::Relaxed);
        assert_eq!(voice("BoomerZombie.Groan"), "BoomerZombie.Groan#female");
        FEMALE.store(false, Ordering::Relaxed);
    }
    #[test]
    fn owned_boomer_assets_have_activities_sounds_and_effects() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            MODEL,
            FEMALE_MODEL,
            VIEW,
            "materials/vgui/hud/pz_charge_boomer.vtf",
        ] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "boomerzombie.alert",
            "boomerzombie.pain",
            "boomerzombie.groan",
            "boomerzombie.attack",
            "boomerzombie.detonate",
            "vomit.use",
            "claw.hitflesh",
            "claw.swing",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            for gender in ["male", "female"] {
                let wave = sound.waves[0].replace("$gender", gender);
                assert!(!assets.read(&wave).unwrap().is_empty(), "{wave}");
            }
        }
        // Retail has no Growl/Death Boomer cues; tick must not request them.
        assert!(registry.definitions.get("boomerzombie.growl").is_none());
        assert!(registry.definitions.get("boomerzombie.death").is_none());
        let pcf =
            crate::source_pcf::parse(&assets.read("particles/boomer_fx.pcf").unwrap()).unwrap();
        for fx in [STREAM_FX, EXPLODE_FX, IT_FX] {
            assert!(
                pcf.iter()
                    .any(|e| e.kind == "DmeParticleSystemDefinition" && e.name == fx),
                "{fx}"
            );
        }
        let assets = std::cell::RefCell::new(assets);
        let read = |p: &str| assets.borrow_mut().read(p).ok();
        let sk = l4d2_formats::anim::Skeleton::load_with(VIEW, read).unwrap();
        for activity in [
            "ACT_VM_DEPLOY",
            "ACT_VM_IDLE",
            "ACT_VM_PRIMARYATTACK",
            "ACT_VM_VOMIT",
        ] {
            assert!(
                sk.sequences().iter().any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
        let survivor =
            l4d2_formats::anim::Skeleton::load_with("models/survivors/survivor_producer.mdl", read)
                .unwrap();
        for activity in [
            "ACT_TERROR_SHOVED_FORWARD",
            "ACT_TERROR_SHOVED_BACKWARD",
            "ACT_TERROR_SHOVED_LEFTWARD",
            "ACT_TERROR_SHOVED_RIGHTWARD",
        ] {
            assert!(
                survivor
                    .sequences()
                    .iter()
                    .any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
    }
}
