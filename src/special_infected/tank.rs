//! Playable Tank: punch + survivor fling, overhead rock throw, frustration,
//! death presentation and the tank music cue. The shared lifecycle (team
//! switch, death/respawn, survivor-bot opposition) lives in
//! special_infected.rs; this module runs after `special_infected::tick`.
//!
//! Retail sources (see local-research/si-agents/tank/NOTES.md):
//! * cvars read from the running retail game (build 10097) over netconsole,
//!   logged in local-research/parity/tank-cvars.log;
//! * server.dll pseudocode: CTankClaw swing/hit FUN_103e3860, FUN_103e43f0,
//!   FUN_103e5800, FUN_103e58f0, FUN_103e3a00 (fling), damage FUN_10266050,
//!   tank health FUN_102661c0, CThrow FUN_10367b60/FUN_10369590, CTankRock
//!   FUN_10368b30/FUN_103694b0/FUN_10369010/FUN_10368cc0/FUN_103682f0,
//!   frustration FUN_1034c610;
//! * hulk.mdl / survivor MDL sequences and attachments.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, EYE, HEIGHT, RADIUS, UNIT},
    game::{Body, CompanionActor},
    game_state::{Companion, Match, Squad},
};
use bevy::prelude::*;
pub const WEAPON: &str = "weapon_tank_claw";
pub const MODEL: &str = "models/infected/hulk.mdl";
pub const VIEW: &str = "models/v_models/weapons/v_claw_hulk.mdl";
/// Thrown debris model (server.dll string next to CTankRock, precache list
/// FUN_103671e0).
pub const ROCK_MODEL: &str = "models/props_debris/concrete_chunk01a.mdl";
/// Retail z_tank_health=4000 times the Versus multiplier 1.5
/// (server FUN_102661c0: easy .75, normal 1, advanced/expert 2, versus 1.5).
/// A player-controlled Tank only exists in Versus-style play.
pub const HEALTH: f32 = 6000.;
/// Retail z_tank_speed = z_tank_speed_vs = 210.
pub const SPEED: f32 = 210.;
pub const PLAYABLE: bool = true;
/// Extra viewmodel (action, activity) pairs this class needs. v_claw_hulk.mdl
/// only has idle/melee/deploy; both punch and the throw swing use melee.
pub const VM_ACTIONS: &[(&str, &str)] = &[
    ("punch", "ACT_VM_PRIMARYATTACK"),
    ("throw", "ACT_VM_PRIMARYATTACK"),
];
pub const CONTROLS: &str = "Tank playtest: spawns immediately (no ghost); left click punches (flings survivors); right click throws a rock.";
/// Viewmodel activity override per special_phase; None keeps the default.
/// 2 = throw wind-up (claws idle), 6 = throw swing (claw melee, retail
/// CThrow plays ACT_VM_PRIMARYATTACK 0.7s after activation).
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    match phase {
        2 => Some("ACT_VM_IDLE"),
        6 => Some("ACT_VM_PRIMARYATTACK"),
        _ => None,
    }
}
/// Lifecycle cue names: hulk soundscripts have no Alert/Death entries; retail
/// uses HulkZombie.Yell and HulkZombie.Die.

// ---- retail values -------------------------------------------------------
/// tank_windup_time: intent to swing.
pub const WINDUP: f32 = 0.5;
/// tank_swing_duration: fist sweep length.
pub const SWING_DURATION: f32 = 0.2;
/// tank_swing_interval / tank_swing_miss_interval / z_tank_attack_interval.
pub const SWING_INTERVAL: f32 = 1.5;
pub const MISS_INTERVAL: f32 = 1.0;
pub const ATTACK_INTERVAL: f32 = 1.5;
/// tank_swing_range, tank_swing_arc (degrees), tank_fist_radius (units).
pub const SWING_RANGE: f32 = 56.;
pub const SWING_ARC: f32 = 180.;
pub const FIST_RADIUS: f32 = 15.;
/// Highest point of the native `Attack` rhand path during the swing (units).
const FIST_TOP: f32 = 120.;
/// vs_tank_damage (Versus damage for both punch and rock).
pub const VS_TANK_DAMAGE: f32 = 24.;
const VERSUS: bool = true;
/// z_tank_throw_force: punch fling speed and rock launch speed (units/s).
pub const THROW_FORCE: f32 = 800.;
/// Fling direction z component before renormalising (_DAT_105d50cc) and the
/// 18-unit lift applied when clear (_DAT_105d35bc).
pub const FLING_UP: f32 = 0.2;
pub const FLING_LIFT: f32 = 18.;
/// z_tank_throw_interval; CThrow starts its timer at interval + 1.
pub const THROW_INTERVAL: f32 = 5.;
/// z_tank_rock_radius: rock detonates when its path passes this close.
pub const ROCK_RADIUS: f32 = 100.;
/// CTankRock gravity scale (entity m_flGravity 0.4) and growing hull
/// (4 → 16 half-extent over 0.5 s, ProximityThink).
pub const ROCK_GRAVITY: f32 = 0.4;
/// Survivor GetUpFrom_TankPunch / GetUpFrom_TankRock durations (all eight
/// survivor MDLs: 1.5 s).
pub const GETUP: f32 = 1.5;
/// z_frustration_lifetime / z_frustration_los_delay / blink percent.
pub const FRUSTRATION_LIFETIME: f32 = 20.;
pub const FRUSTRATION_LOS_DELAY: f32 = 2.;
pub const FRUSTRATION_WARN: f32 = 0.75;
/// sound/music/tank/tank.wav (Event.Tank) length in seconds.
#[cfg_attr(not(test), allow(dead_code))]
pub const MUSIC_LENGTH: f32 = 52.369;
/// Overhead throws (tank_rock_overhead_percent default 100 →
/// ACT_TANK_OVERHEAD_THROW): (sequence, duration, AE_ATTACK_HIT = rock
/// created, AE_ATTACK_END = released), seconds, from hulk.mdl events.
pub const THROWS: [(&str, f32, f32, f32); 3] = [
    ("Throw_02", 3.0, 1.0, 2.4667),
    ("Throw_03", 2.6, 0.8333, 2.0333),
    ("Throw_04", 3.0, 0.9333, 2.6333),
];
/// CThrow default viewmodel swing time (timer 0x460 = 0.7 s).
pub const THROW_VM_SWING: f32 = 0.7;
/// hulk.mdl "debris" attachment (model space: x forward, y left, z up,
/// units) sampled every 1/15 s on the sequence grid, starting at
/// `DEBRIS_START` (the grid sample at/before rock creation) and covering
/// release.
const DEBRIS_START: [f32; 3] = [1.0, 0.8, 0.9333];
const DEBRIS_PATHS: [&[[f32; 3]]; 3] = [
    &[
        [48.3, -11.5, 7.4],
        [43.5, -17.6, 18.7],
        [33.3, -21.7, 31.1],
        [30.8, -22.8, 48.6],
        [24.3, -23.4, 63.2],
        [20.6, -24.0, 69.6],
        [17.6, -23.8, 64.4],
        [14.6, -20.9, 63.2],
        [11.1, -18.0, 68.6],
        [7.0, -14.6, 77.3],
        [8.7, -12.9, 86.0],
        [12.9, -13.6, 96.6],
        [4.0, -15.7, 102.6],
        [-5.7, -24.5, 105.0],
        [-19.1, -31.3, 96.8],
        [-28.4, -27.7, 88.6],
        [-31.5, -26.3, 80.8],
        [-31.6, -25.8, 75.4],
        [-28.6, -34.8, 74.1],
        [-13.2, -37.7, 76.0],
        [20.1, -36.1, 85.6],
        [50.9, -30.3, 92.6],
        [125.8, -19.3, 93.6],
    ],
    &[
        [54.4, -6.3, -3.1],
        [54.4, -5.4, -1.5],
        [54.4, -6.7, -0.6],
        [53.2, -11.5, 1.1],
        [52.3, -26.3, 4.3],
        [43.2, -43.9, 8.8],
        [20.1, -57.0, 9.1],
        [-11.9, -60.6, 11.3],
        [-41.1, -53.0, 14.7],
        [-56.3, -40.3, 20.4],
        [-57.8, -26.9, 22.4],
        [-55.7, -14.8, 22.1],
        [-52.2, -3.9, 22.3],
        [-45.0, 4.2, 22.0],
        [-46.2, 1.1, 20.6],
        [-39.6, -22.3, 21.4],
        [-20.3, -46.1, 22.7],
        [15.9, -61.1, 25.3],
        [67.3, -63.8, 34.3],
        [99.5, -43.4, 40.4],
    ],
    &[
        [20.5, -10.8, -4.1],
        [20.4, -10.7, 4.7],
        [20.9, -10.8, 14.6],
        [21.0, -10.8, 30.1],
        [19.2, -10.1, 49.8],
        [16.3, -8.9, 63.4],
        [13.0, -7.6, 68.5],
        [9.5, -6.2, 70.4],
        [6.3, -4.9, 70.7],
        [3.7, -3.7, 71.0],
        [1.8, -2.6, 72.8],
        [0.1, -1.6, 76.6],
        [-2.1, -0.6, 81.6],
        [-4.5, 0.1, 87.5],
        [-6.7, 0.7, 93.9],
        [-9.1, 1.2, 99.7],
        [-11.8, 1.5, 103.6],
        [-15.3, 1.6, 104.6],
        [-19.6, 1.1, 103.2],
        [-26.8, 0.4, 99.5],
        [-36.6, -0.1, 94.5],
        [-46.5, -0.2, 89.1],
        [-59.3, -0.1, 84.7],
        [-66.6, 0.1, 85.4],
        [-48.3, 0.4, 99.3],
        [-9.6, 0.6, 105.5],
        [36.6, 0.8, 101.8],
    ],
];

// ---- pure mechanics ------------------------------------------------------
/// Retail survivor damage from a Tank punch or rock (server FUN_10266050).
/// `difficulty`: 0 easy, 1 normal, 2 advanced, 3 expert.
pub(crate) fn punch_damage(versus: bool, difficulty: u8, incapacitated: bool) -> f32 {
    if versus {
        return VS_TANK_DAMAGE;
    }
    match (incapacitated, difficulty) {
        (true, 3..) => 150.,
        (true, _) => 75.,
        (false, 3..) => 250.,
        (false, 2) => 33.,
        (false, _) => 24.,
    }
}
/// Punch fling velocity (FUN_103e3a00): horizontal Tank→victim direction,
/// z = 0.2, renormalised, times z_tank_throw_force.
pub(crate) fn fling_velocity(tank: Vec3, victim: Vec3, fallback: Vec3) -> Vec3 {
    let flat = Vec3::new(victim.x - tank.x, 0., victim.z - tank.z);
    let flat = if flat.length_squared() > 1e-8 {
        flat.normalize()
    } else {
        Vec3::new(fallback.x, 0., fallback.z).normalize_or(Vec3::NEG_Z)
    };
    (flat + Vec3::Y * FLING_UP).normalize() * THROW_FORCE * UNIT
}
/// Whether a survivor hull is inside the fist sweep envelope: within
/// tank_swing_range + tank_fist_radius of the Tank's centre (to the hull),
/// inside the 180° tank_swing_arc, and overlapping the fist's height band.
pub(crate) fn in_punch_reach(tank_feet: Vec3, yaw: f32, victim_feet: Vec3, height: f32) -> bool {
    let forward = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let delta = victim_feet - tank_feet;
    let flat = Vec3::new(delta.x, 0., delta.z);
    if flat.length() > (SWING_RANGE + FIST_RADIUS) * UNIT + RADIUS {
        return false;
    }
    let half_arc = (SWING_ARC * 0.5).to_radians().cos();
    if flat.length() > RADIUS && forward.dot(flat.normalize()) < half_arc - 1e-4 {
        return false;
    }
    victim_feet.y <= tank_feet.y + FIST_TOP * UNIT
        && victim_feet.y + height >= tank_feet.y - FIST_RADIUS * UNIT
}
/// Rock collision half-extent: grows from 4 to 16 units over 0.5 s.
pub(crate) fn rock_half_extent(age: f32) -> f32 {
    let units = if age < 0.5 {
        (1. - (0.5 - age) * 2.) * 12. + 4.
    } else {
        16.
    };
    units * UNIT
}
fn closest_on_segment(point: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let delta = to - from;
    let t = ((point - from).dot(delta) / delta.length_squared().max(1e-8)).clamp(0., 1.);
    from + delta * t
}
/// Rotates a hulk.mdl model-space point (x forward, y left, z up, units)
/// into the world for a Tank at `feet` facing `yaw`.
pub(crate) fn model_point(feet: Vec3, yaw: f32, p: [f32; 3]) -> Vec3 {
    feet + Quat::from_rotation_y(yaw + std::f32::consts::FRAC_PI_2)
        * Vec3::new(p[0], p[2], -p[1])
        * UNIT
}
/// A Tank on fire burns until death instead of taking per-second fire
/// damage (see special_infected::ai `tank_burn`).
pub(crate) fn burns_by_duration(model: &str) -> bool {
    model.eq_ignore_ascii_case(MODEL)
}
/// tank_burn_duration / _hard / _expert: seconds a burning Tank lives
/// (Easy and Normal share the base value).
pub(crate) fn burn_duration(difficulty: crate::campaign_settings::Difficulty) -> f32 {
    use crate::campaign_settings::Difficulty::*;
    match difficulty {
        Easy | Normal => 75.,
        Advanced => 80.,
        Expert => 85.,
    }
}
pub(crate) fn debris_at(variant: usize, clock: f32) -> [f32; 3] {
    let path = DEBRIS_PATHS[variant];
    let f = ((clock - DEBRIS_START[variant]) * 15.).clamp(0., (path.len() - 1) as f32);
    let i = (f.floor() as usize).min(path.len() - 1);
    let j = (i + 1).min(path.len() - 1);
    let a = Vec3::from_array(path[i]);
    let b = Vec3::from_array(path[j]);
    a.lerp(b, f - i as f32).to_array()
}
/// Frustration gained per one-second step: 100 / z_frustration_lifetime.
pub(crate) fn frustration_step(frustration: f32, unseen: f32, burning: bool) -> f32 {
    if burning || unseen <= FRUSTRATION_LOS_DELAY {
        frustration
    } else {
        (frustration + 1. / FRUSTRATION_LIFETIME).min(1.)
    }
}

// ---- state ---------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knock {
    Punch,
    Rock,
}
struct Victim {
    index: usize,
    kind: Knock,
    velocity: Vec3,
    airborne: bool,
    getup: f32,
    flight: f32,
}
struct Swing {
    clock: f32,
    hit: Vec<usize>,
    any: bool,
    world: bool,
}
struct Throw {
    clock: f32,
    variant: usize,
    anchor: Vec3,
    rock: Option<u32>,
}
/// z_tank_throw_health: a thrown rock is destroyed by 50 damage (CTankRock
/// ProximityThink 10369010 gives it health and DAMAGE_YES once grown).
pub(crate) const ROCK_HEALTH: f32 = 50.;
/// Survivor shots this tick: origin, direction, reach, damage. Pushed by the
/// shooting code and resolved against the flying AI rocks.
static ROCK_SHOTS: std::sync::Mutex<Vec<(Vec3, Vec3, f32, f32)>> = std::sync::Mutex::new(Vec::new());
pub(crate) fn shot_at_rocks(origin: Vec3, direction: Vec3, reach: f32, damage: f32) {
    if let Ok(mut shots) = ROCK_SHOTS.lock() {
        if shots.len() < 256 {
            shots.push((origin, direction, reach, damage));
        }
    }
}
pub(crate) fn take_rock_shots() -> Vec<(Vec3, Vec3, f32, f32)> {
    ROCK_SHOTS.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
}
pub(crate) struct Rock {
    pub(crate) id: u32,
    pub(crate) health: f32,
    pub(crate) position: Vec3,
    pub(crate) previous: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) age: f32,
    pub(crate) spin: Vec3,
    pub(crate) rotation: Quat,
}
/// Rocks thrown by AI Tanks (special_infected::ai), drawn with the player's.
/// `held`: rocks in an AI Tank's hand before release.
#[derive(Resource, Default)]
pub(crate) struct AiRocks {
    pub(crate) rocks: Vec<Rock>,
    pub(crate) held: Vec<(u32, Vec3, Quat)>,
    pub(crate) serial: u32,
    /// Multiplayer guest: the host's rocks (held and flying), drawn only.
    pub(crate) net: Vec<(u32, Vec3, Quat)>,
}
impl Rock {
    pub(crate) fn launched(id: u32, start: Vec3, velocity: Vec3, spin: Vec3) -> Self {
        Self { id, health: ROCK_HEALTH, position: start, previous: start, velocity, age: 0., spin, rotation: Quat::IDENTITY }
    }
    /// One flight step: gravity (CTankRock m_flGravity 0.4) and spin.
    /// Returns the swept segment end.
    pub(crate) fn advance(&mut self, dt: f32) -> Vec3 {
        self.age += dt;
        self.previous = self.position;
        self.velocity.y -= ROCK_GRAVITY * crate::player_movement::GRAVITY * dt;
        self.rotation = (Quat::from_scaled_axis(self.spin * dt) * self.rotation).normalize();
        self.position + self.velocity * dt
    }
}
pub(crate) fn closest_point(point: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    closest_on_segment(point, from, to)
}
/// AI rock launch velocity toward `target` at z_tank_throw_force with
/// gravity scaled by 0.4: the low ballistic solution, limited to
/// tank_throw_max_loft_angle (30 degrees) above the straight line.
pub(crate) fn rock_launch(start: Vec3, target: Vec3) -> Vec3 {
    let speed = THROW_FORCE * UNIT;
    let gravity = ROCK_GRAVITY * crate::player_movement::GRAVITY;
    let delta = target - start;
    let flat = Vec3::new(delta.x, 0., delta.z);
    let x = flat.length().max(1e-3);
    let y = delta.y;
    let direct = y.atan2(x);
    let disc = speed.powi(4) - gravity * (gravity * x * x + 2. * y * speed * speed);
    let angle = if disc >= 0. {
        ((speed * speed - disc.sqrt()) / (gravity * x)).atan()
    } else {
        direct + 30f32.to_radians()
    }
    .min(direct + 30f32.to_radians());
    let heading = flat / x;
    (heading * angle.cos() + Vec3::Y * angle.sin()) * speed
}
struct Death {
    clock: f32,
    feet: Vec3,
    yaw: f32,
    sequence: &'static str,
}
#[derive(Resource, Default)]
pub struct Tank {
    swing: Option<Swing>,
    next_attack: f32,
    throw: Option<Throw>,
    throw_cooldown: f32,
    rocks: Vec<Rock>,
    victims: Vec<Victim>,
    /// Frustration 0..1 (retail meter 0..100 in steps of 5).
    pub frustration: f32,
    frustration_clock: f32,
    unseen: f32,
    death: Option<Death>,
    rock_serial: u32,
    seed: u32,
    capture_done: bool,
    capture_clock: f32,
}
fn capture_delay() -> f32 {
    let args = std::env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|a| a[0] == "--capture-tank-delay")
        .and_then(|a| a[1].parse().ok())
        .unwrap_or(1.)
}
fn capture_case() -> Option<&'static str> {
    static CASE: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *CASE.get_or_init(|| {
        std::env::args().find_map(|a| match a.as_str() {
            "--capture-tank-punch" => Some("punch"),
            "--capture-tank-throw" => Some("throw"),
            _ => None,
        })
    })
}
impl Tank {
    fn random(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }
    /// The rock held during a throw (world position) and flying rocks.
    fn held_rock(&self, feet: Vec3, yaw: f32) -> Option<(u32, Vec3)> {
        let throw = self.throw.as_ref()?;
        let id = throw.rock?;
        if self.rocks.iter().any(|r| r.id == id) {
            return None;
        }
        Some((
            id,
            model_point(feet, yaw, debris_at(throw.variant, throw.clock)),
        ))
    }
}
/// Map-sound id for the Event.Tank loop, so it can be cut on death.
const MUSIC_SOUND: usize = usize::MAX - 0x7a4b;
fn music_wanted(state: &Match, hunter: &Hunter, playing: bool) -> bool {
    playing && super::active(state, hunter, SpecialClass::Tank)
}
/// Loops Event.Tank while the local Tank is alive and stops it immediately
/// on death, team/class change or leaving gameplay (retail stops the cue).
fn music(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    frontend: Res<crate::game_menu::Frontend>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    sources: Query<(Entity, &crate::game_audio::MapSound)>,
    mut on: Local<bool>,
    ai_tanks: Query<(&super::ai::AiSpecial, &crate::game::Enemy)>,
    time: Res<Time>,
    mut requested: Local<(f32, bool)>,
) {
    // A living AI Tank plays the same cue (Director tank music).
    let ai_tank = ai_tanks.iter().any(|(ai, e)| ai.class == SpecialClass::Tank && e.health > 0.);
    // Event.FinalBattle blocks and stops the "tank" track (its
    // MusicBlockTrackList/MusicStopTrackList).
    let final_battle = sources.iter().any(|(_, sound)| sound.0 == crate::survivor_music::FINAL_MUSIC);
    let wanted = music_wanted(&state, &hunter, frontend.playing()) || frontend.playing() && ai_tank && !final_battle;
    // A request can be lost (dropped from a full audio queue during a horde,
    // or the loop despawned); while the Tank lives, ask again if no music
    // source exists a few seconds after asking (the first ask also loads
    // the wave asynchronously).
    let playing = sources.iter().any(|(_, sound)| sound.0 == MUSIC_SOUND);
    let now = time.elapsed_secs();
    let (requested, started) = &mut *requested;
    *started |= playing;
    // Lost: it played and is gone, or it never started (slow loads allowed).
    let lost = *on && !playing && (*started || now - *requested > 20.);
    if wanted && (!*on || lost) {
        if lost {
            println!("Tank music lost while the Tank lives: requested again");
        }
        *requested = now;
        *started = false;
        sounds.0.push_back(crate::game_audio::Request {
            name: "Event.Tank".into(),
            position: None,
            looping: true,
            gain: 1.,
            map_entity: Some(MUSIC_SOUND), soundlevel: None,
        });
    } else if !wanted && *on {
        sounds.1.push(MUSIC_SOUND);
        for (entity, sound) in &sources {
            if sound.0 == MUSIC_SOUND {
                commands.entity(entity).despawn();
            }
        }
    }
    *on = wanted;
}
pub fn install(app: &mut App) {
    app.init_resource::<Tank>()
        .init_resource::<AiRocks>()
        .add_systems(FixedUpdate, tick.after(super::tick))
        .add_systems(
            Update,
            (draw, frustration_hud, music).after(crate::game::controls),
        )
        .add_systems(
            PostUpdate,
            death_camera.before(bevy::transform::TransformSystems::Propagate),
        );
}

// ---- simulation ----------------------------------------------------------
fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut tank: ResMut<Tank>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut actors: Query<(&CompanionActor, &mut Transform)>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    if !state.infected {
        if !tank.victims.is_empty() || !tank.rocks.is_empty() || tank.death.is_some() {
            for v in std::mem::take(&mut tank.victims) {
                if let Some(m) = squad.0.get_mut(v.index) {
                    m.vertical = 0.;
                }
            }
            *tank = Tank::default();
        }
        return;
    }
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    let collision = collision.for_actor(true, hunter.ghost);
    if tank.seed == 0 {
        tank.seed = 0x9E37_79B9 ^ (state.elapsed.to_bits() | 1);
    }
    // Rocks and flung survivors continue independently of the Tank's life.
    update_rocks(&mut tank, &collision, &mut squad, &mut sounds, dt);
    update_victims(&mut tank, &collision, &mut squad, dt);
    for (actor, mut transform) in &mut actors {
        if let Some(v) = tank.victims.iter().find(|v| v.index == actor.0) {
            let m = &squad.0[v.index];
            transform.translation = m.position;
            if v.kind == Knock::Punch {
                transform.rotation = Quat::from_rotation_y((-m.aim.z).atan2(m.aim.x));
            }
        }
    }
    if state.special_class != SpecialClass::Tank {
        tank.swing = None;
        tank.throw = None;
        tank.death = None;
        return;
    }
    if state.dead {
        tank.swing = None;
        tank.throw = None;
        if tank.death.is_none() {
            let running = Vec2::new(body.velocity.x, body.velocity.z).length() > 100. * UNIT;
            tank.death = Some(Death {
                clock: 0.,
                feet: body.feet,
                yaw: body.yaw,
                sequence: if running {
                    "Death_Running_07"
                } else {
                    "Death_11ab"
                },
            });
            sounds.play("HulkZombie.Thud", Some(body.feet));
        }
        if let Some(death) = &mut tank.death {
            death.clock += dt;
        }
        hunter.ability_progress = Some(0.);
        return;
    }
    tank.death = None;
    if hunter.ghost {
        // Retail Versus gives the Tank to a player already spawned (no ghost).
        hunter.ghost = false;
        state.special_phase = 0;
        tank.next_attack = 0.;
        tank.throw_cooldown = 0.;
        tank.frustration = 0.;
        tank.unseen = 0.;
        // No spawn cue: the Yell comes from the alert mood (`voice`).
    }

    // Capture fixtures: --capture-tank-punch / --capture-tank-throw place the
    // squad in front and trigger the ability once after one second.
    let capture = capture_case();
    // L4D2R_FIXTURE_SQUAD="x y z;x y z": debug, the bots stand at these
    // Source positions instead (hittable-prop tests).
    let fixed_squad = std::env::var("L4D2R_FIXTURE_SQUAD").ok().map(|v| {
        v.split(';')
            .filter_map(|p| {
                let n: Vec<f32> = p.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                (n.len() == 3).then(|| Vec3::from_array(crate::binary::source_position([n[0], n[1], n[2]])))
            })
            .collect::<Vec<_>>()
    });
    if let Some(points) = fixed_squad.as_ref().filter(|_| capture.is_some() && !tank.capture_done) {
        for (member, p) in squad.0.iter_mut().zip(points) {
            member.position = *p;
            member.cooldown = 10.;
        }
    } else if capture.is_some() && !tank.capture_done {
        let direction = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let distance = if capture == Some("throw") { 12. } else { 1.6 };
        for (i, member) in squad.0.iter_mut().enumerate() {
            if !member.state.pinned {
                member.position = collision.walk(
                    body.feet,
                    direction * (distance + i as f32 * 0.9)
                        + Quat::from_rotation_y(body.yaw) * Vec3::X * (i as f32 * 0.5 - 0.7),
                    HEIGHT,
                    true,
                );
                member.cooldown = 10.;
            }
        }
    }
    if capture.is_some() {
        tank.capture_clock += dt;
    }
    let fixture = capture.is_some() && !tank.capture_done && tank.capture_clock >= capture_delay();
    if fixture {
        tank.capture_done = true;
    }
    let punch =
        std::mem::take(&mut input.fire) || input.fire_held || fixture && capture == Some("punch");
    let pressed = std::mem::take(&mut input.shove);
    let throw = pressed || input.shove_held || fixture && capture == Some("throw");
    input.reload = false;
    input.use_item = false;
    tank.next_attack = (tank.next_attack - dt).max(0.);
    tank.throw_cooldown = (tank.throw_cooldown - dt).max(0.);
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let eye = body.feet + Vec3::Y * EYE;
    // --- punch
    if punch && tank.next_attack <= 0. && tank.swing.is_none() && tank.throw.is_none() {
        tank.swing = Some(Swing {
            clock: 0.,
            hit: vec![],
            any: false,
            world: false,
        });
        // FUN_103e43f0: next attack = windup + swing interval.
        tank.next_attack = WINDUP + SWING_INTERVAL;
        state.special_phase = 4;
        state.shot_serial = state.shot_serial.wrapping_add(1);
        sounds.play("HulkZombie.Attack", None);
    }
    if let Some(mut swing) = tank.swing.take() {
        let before = swing.clock;
        swing.clock += dt;
        let window = WINDUP..WINDUP + SWING_DURATION;
        if swing.clock >= window.start && before < window.end {
            for i in 0..squad.0.len() {
                let m = &squad.0[i];
                if m.state.dead
                    || swing.hit.contains(&i)
                    || m.state.pinned && m.state.pinned_by != Some(SpecialClass::Tank)
                    || tank.victims.iter().any(|v| v.index == i)
                {
                    continue;
                }
                let height = if m.state.incap {
                    crate::collision::DUCK_HEIGHT
                } else {
                    HEIGHT
                };
                if !in_punch_reach(body.feet, body.yaw, m.position, height)
                    || collision
                        .trace(eye, m.position + Vec3::Y * height * 0.5, Vec3::ZERO)
                        .fraction
                        < 1.
                {
                    continue;
                }
                swing.hit.push(i);
                swing.any = true;
                punch_survivor(
                    &mut tank,
                    &mut squad,
                    i,
                    body.feet,
                    forward,
                    &collision,
                    &mut sounds,
                );
            }
            if !swing.world && !swing.any {
                swing.world = true;
                if let Some(logic) = logic.as_mut() {
                    let reach = (SWING_RANGE + FIST_RADIUS) * UNIT + RADIUS;
                    // Doors take z_door_pound_damage(60) x 20 (FUN_103e58f0).
                    let broke = logic
                        .shoot_breakable(
                            eye,
                            forward,
                            reach,
                            60. * 20.,
                            l4d2r_game_core::entities::DamageKind::Club,
                            &collision,
                        )
                        .is_some();
                    // Physics props: melee damage force (999.9 damage x
                    // ImpulseScale(75,4)=300, kg·in/s) scaled by the hittable
                    // mass factor (tank_swing_physics_prop_force).
                    punch_prop(logic, eye, forward, reach, &collision);
                    if broke {
                        sounds.play("Claw.Hit", Some(eye + forward * reach));
                    }
                }
            }
            if swing.any {
                tank.next_attack = ATTACK_INTERVAL;
            }
        }
        if before < window.end && swing.clock >= window.end && !swing.any {
            // FUN_103e5800: a miss refires after tank_swing_miss_interval.
            tank.next_attack = (tank.next_attack - (SWING_INTERVAL - MISS_INTERVAL)).max(0.);
        }
        // Keep phase 4 for the native 1.0 s claw melee viewmodel sequence.
        if swing.clock < 1. {
            tank.swing = Some(swing);
        } else if state.special_phase == 4 {
            state.special_phase = 0;
        }
    }
    // --- rock throw
    if throw
        && tank.throw.is_none()
        && tank.swing.is_none()
        && tank.throw_cooldown <= 0.
        && body.grounded
    {
        let variant = (tank.random() % THROWS.len() as u32) as usize;
        tank.throw = Some(Throw {
            clock: 0.,
            variant,
            anchor: body.feet,
            rock: None,
        });
        tank.throw_cooldown = THROW_INTERVAL + 1.;
        state.special_phase = 2;
        sounds.play("HulkZombie.Throw.Pickup", None);
    }
    // CThrow activation (server 10367b60) refused off the ground:
    // HulkZombie.Throw.Fail. z_tank_throw_fail_interval defaults to 0, so
    // no retry delay is set.
    if pressed && tank.throw.is_none() && tank.swing.is_none() && tank.throw_cooldown <= 0. && !body.grounded {
        sounds.play("HulkZombie.Throw.Fail", None);
    }
    if let Some(mut t) = tank.throw.take() {
        let (_, duration, create, release) = THROWS[t.variant];
        let before = t.clock;
        t.clock += dt;
        // The Tank is rooted for the whole throw animation.
        body.feet.x = t.anchor.x;
        body.feet.z = t.anchor.z;
        body.velocity.x = 0.;
        body.velocity.z = 0.;
        if before < THROW_VM_SWING && t.clock >= THROW_VM_SWING {
            state.special_phase = 6;
            state.shot_serial = state.shot_serial.wrapping_add(1);
        }
        if before < create && t.clock >= create {
            tank.rock_serial = tank.rock_serial.wrapping_add(1);
            t.rock = Some(tank.rock_serial);
        }
        if before < release && t.clock >= release {
            let attach = model_point(body.feet, body.yaw, debris_at(t.variant, release));
            // Retail traces eye → debris attachment and spawns at the end.
            let trace = collision.trace(eye, attach, Vec3::ZERO);
            let start = eye.lerp(attach, (trace.fraction - 0.02).max(0.));
            let spin = Vec3::new(
                60_f32.to_radians(),
                ((tank.random() % 241) as f32 - 120.).to_radians(),
                0.,
            );
            tank.rocks.push(Rock {
                id: t.rock.unwrap_or(0),
                health: ROCK_HEALTH,
                position: start,
                previous: start,
                velocity: forward * THROW_FORCE * UNIT
                    + Vec3::new(body.velocity.x, 0., body.velocity.z),
                age: 0.,
                spin,
                rotation: Quat::IDENTITY,
            });
            sounds.play("HulkZombie.Throw", None);
            sounds.play("HulkZombie.Throw.FlyLoop", Some(start));
        }
        if t.clock < duration {
            tank.throw = Some(t);
        } else {
            state.special_phase = 0;
        }
    }
    hunter.ability_progress = Some(1. - tank.throw_cooldown / (THROW_INTERVAL + 1.));
    // --- frustration (FUN_1034c610): +100/z_frustration_lifetime per second
    // once no survivor has had line of sight for z_frustration_los_delay;
    // reset by damaging a survivor. On fire never applies in this playtest.
    let seen = squad.0.iter().any(|m| {
        !m.state.dead
            && collision
                .sight_trace(
                    m.position + Vec3::Y * EYE,
                    body.feet + Vec3::Y * HEIGHT * 0.5,
                )
                .fraction
                >= 1.
    });
    tank.unseen = if seen { 0. } else { tank.unseen + dt };
    tank.frustration_clock += dt;
    if tank.frustration_clock >= 1. {
        tank.frustration_clock -= 1.;
        tank.frustration = frustration_step(tank.frustration, tank.unseen, false);
        if tank.frustration >= 1. {
            // Retail hands the Tank to another player or the AI.
            state.hint = "You lost control of the Tank.".into();
            state.health = 0.;
            state.dead = true;
            tank.frustration = 0.;
        }
    }
}
/// Pushes the first physics prop on the punch ray (mass-scaled force).
pub(crate) fn punch_prop(
    logic: &mut crate::l4d2r_cinematic::MapLogic,
    eye: Vec3,
    direction: Vec3,
    range: f32,
    collision: &Collision,
) {
    let wall = collision
        .trace(eye, eye + direction * range, Vec3::ZERO)
        .fraction
        * range;
    let target = logic
        .physics
        .0
        .keys()
        .filter_map(|&id| {
            if !logic.world.ents.get(id).is_some_and(|e| e.alive) {
                return None;
            }
            let (lo, hi) = logic.bounds(id)?;
            let distance = crate::ray_box(eye, direction, lo, hi)?;
            (distance <= range && distance <= wall + 0.08).then_some((
                id,
                distance,
                logic.physics.center_world(id,&logic.world).unwrap_or((lo+hi)*0.5),
            ))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((id, distance, center)) = target {
        let mass = logic.physics.0[&id].mass;
        // The prop now counts as Tank-hit for 2 s (crush reactions).
        crate::prop_impact::mark_tank_hit(id, logic.world.time);
        // prop_physics OnHitByTank (hittable cars kill their vphysics clip).
        logic.world.fire_output(id, "OnHitByTank", None);
        println!("Tank punch hit prop {id} ({}) mass {mass}", logic.world.ents[id].me.targetname);
        logic.physics.impulse(
            id,
            direction,
            prop_impulse(mass),
            eye + direction * distance - center,
        );
        logic.physics.launch_punched(id);
    }
}
/// Melee damage force for a physics prop of `mass` kg (rewrite kg·m/s).
pub(crate) fn prop_impulse(mass: f32) -> f32 {
    const PROP_FORCE: f32 = 4.; // tank_swing_physics_prop_force
    let mass = mass.clamp(100., 1500.);
    let scale = (mass - 100.) * (PROP_FORCE - 1.) / 1400. + 1.;
    999.9 * 300. * scale * UNIT
}
fn punch_survivor(
    tank: &mut Tank,
    squad: &mut Squad,
    i: usize,
    tank_feet: Vec3,
    forward: Vec3,
    collision: &Collision,
    sounds: &mut crate::game_audio::Queue,
) {
    let m = &mut squad.0[i];
    let was_incap = m.state.incap;
    m.state.damage(punch_damage(VERSUS, 1, was_incap));
    tank.frustration = 0.;
    if was_incap || m.state.incap || m.state.dead {
        // Ground pound on an incapacitated survivor: no fling.
        sounds.play("HulkZombie.PunchIncap", Some(m.position));
        return;
    }
    // Retail 103e58f0: a fist landing on a survivor plays the flesh hit
    // with the punch.
    sounds.play("Claw.HitFlesh", Some(m.position));
    sounds.play("HulkZombie.Punch", Some(m.position));
    let velocity = fling_velocity(tank_feet, m.position, forward);
    let lifted = m.position + Vec3::Y * FLING_LIFT * UNIT;
    if collision.clear_hull(lifted, HEIGHT) {
        m.position = lifted;
    }
    m.vertical = velocity.y;
    m.aim =
        Vec3::new(tank_feet.x - m.position.x, 0., tank_feet.z - m.position.z).normalize_or(Vec3::X);
    super::pin_member(squad, i, SpecialClass::Tank);
    tank.victims.push(Victim {
        index: i,
        kind: Knock::Punch,
        velocity,
        airborne: true,
        getup: GETUP,
        flight: 0.,
    });
}
fn update_victims(tank: &mut Tank, collision: &Collision, squad: &mut Squad, dt: f32) {
    tank.victims.retain_mut(|v| {
        let Some(m) = squad.0.get_mut(v.index) else {
            return false;
        };
        if v.airborne {
            v.flight += dt;
            v.velocity.y -= crate::player_movement::GRAVITY * dt;
            let (next, contact) =
                collision.walk_velocity(m.position, v.velocity, dt, HEIGHT, false);
            let ceiling = v.velocity.y > 0. && contact.y < v.velocity.y * 0.5;
            v.velocity = Vec3::new(
                contact.x,
                if ceiling { 0. } else { v.velocity.y },
                contact.z,
            );
            m.position = next;
            if v.velocity.y <= 0. && collision.grounded(next, HEIGHT) || v.flight > 6. {
                m.position = collision.snap_ground(next, HEIGHT);
                v.airborne = false;
                v.velocity = Vec3::ZERO;
                m.vertical = 0.;
            } else {
                // Non-zero `vertical` marks the airborne pose for animation.
                m.vertical = if v.velocity.y == 0. {
                    -1e-4
                } else {
                    v.velocity.y
                };
            }
            return true;
        }
        m.vertical = 0.;
        v.getup -= dt;
        if v.getup > 0. && !m.state.dead {
            return true;
        }
        if m.state.pinned_by == Some(SpecialClass::Tank) {
            m.state.pinned = false;
            m.state.pinned_by = None;
        }
        false
    });
}
fn update_rocks(
    tank: &mut Tank,
    collision: &Collision,
    squad: &mut Squad,
    sounds: &mut crate::game_audio::Queue,
    dt: f32,
) {
    let mut knocked = vec![];
    tank.rocks.retain_mut(|rock| {
        rock.age += dt;
        rock.previous = rock.position;
        rock.velocity.y -= ROCK_GRAVITY * crate::player_movement::GRAVITY * dt;
        let next = rock.position + rock.velocity * dt;
        rock.rotation = (Quat::from_scaled_axis(rock.spin * dt) * rock.rotation).normalize();
        // CTankRock::ProximityThink: survivors near the swept path.
        let victim = squad
            .0
            .iter()
            .enumerate()
            .filter(|(i, m)| {
                !m.state.dead
                    && !m.state.incap
                    && !knocked.contains(i)
                    && !(m.state.pinned && m.state.pinned_by != Some(SpecialClass::Tank))
            })
            .filter_map(|(i, m)| {
                let center = m.position + Vec3::Y * HEIGHT * 0.5;
                let near = closest_on_segment(center, rock.previous, next);
                (near.distance(center) < ROCK_RADIUS * UNIT
                    && collision.trace(near, center, Vec3::ZERO).fraction >= 1.)
                    .then_some((i, near.distance(center), near))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _, near)) = victim {
            rock.position = near;
            knocked.push(i);
            sounds.play("HulkZombie.ThrownProjectileHit", Some(near));
            return false;
        }
        let half = Vec3::splat(rock_half_extent(rock.age));
        let hit = collision.trace(rock.position, next, half);
        if hit.fraction < 1. && !hit.start_solid || rock.age > 10. {
            rock.position = rock.position.lerp(next, hit.fraction);
            sounds.play("HulkZombie.ThrownProjectileHit", Some(rock.position));
            return false;
        }
        rock.position = next;
        true
    });
    for i in knocked {
        let m = &mut squad.0[i];
        m.state.damage(punch_damage(VERSUS, 1, false));
        tank.frustration = 0.;
        if m.state.dead || m.state.incap {
            continue;
        }
        // FUN_10316eb0(2): knockdown in place with a getup.
        tank.victims.retain(|v| v.index != i);
        m.vertical = 0.;
        super::pin_member(squad, i, SpecialClass::Tank);
        tank.victims.push(Victim {
            index: i,
            kind: Knock::Rock,
            velocity: Vec3::ZERO,
            airborne: false,
            getup: GETUP,
            flight: 0.,
        });
    }
}

/// Survivor victim activity while a Tank punch/rock owns them: Hit_by_
/// TankPunch then the looping Idle_Fall_From_TankPunch while airborne, then
/// GetUpFrom_TankPunch (ACT_TERROR_TANKPUNCH_LAND), or GetUpFrom_TankRock for
/// a rock knockdown. `current`/`clock`/`duration` describe the playing
/// activity; returns the activity key and native candidates.
pub(crate) fn victim_activity(
    member: &Companion,
    current: &str,
    clock: f32,
    duration: f32,
) -> Option<(&'static str, &'static [&'static str])> {
    const HIT: &[&str] = &["ACT_TERROR_HIT_BY_TANKPUNCH", "Hit_by_TankPunch"];
    const FLY: &[&str] = &[
        "ACT_TERROR_IDLE_FALL_FROM_TANKPUNCH",
        "Idle_Fall_From_TankPunch",
    ];
    const LAND: &[&str] = &["ACT_TERROR_TANKPUNCH_LAND", "GetUpFrom_TankPunch"];
    const ROCK: &[&str] = &["ACT_TERROR_TANKROCK_TO_STAND", "GetUpFrom_TankRock"];
    let held = member.state.pinned && member.state.pinned_by == Some(SpecialClass::Tank);
    if held && !member.state.dead {
        if member.vertical != 0. {
            return Some(
                if current == "tank_fly" || current == "tank_hit" && clock >= duration {
                    ("tank_fly", FLY)
                } else {
                    ("tank_hit", HIT)
                },
            );
        }
        return Some(match current {
            "tank_hit" | "tank_fly" | "tank_land" => ("tank_land", LAND),
            _ => ("tank_rock", ROCK),
        });
    }
    match current {
        "tank_land" if clock < duration && !member.state.incap => Some(("tank_land", LAND)),
        "tank_rock" if clock < duration && !member.state.incap => Some(("tank_rock", ROCK)),
        _ => None,
    }
}

// ---- presentation ----------------------------------------------------------
#[derive(Component)]
enum TankDraw {
    Rock(u32),
    Death(usize),
    /// Third-person body during the player's rock throw.
    Throw(usize),
}
fn draw(
    mut commands: Commands,
    state: Res<Match>,
    tank: Res<Tank>,
    ai_rocks: Res<AiRocks>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut draws: Query<(Entity, &TankDraw, &Mesh3d, &mut Transform)>,
) {
    let mut rocks: Vec<(u32, Vec3, Quat)> = tank
        .rocks
        .iter()
        .map(|r| (r.id, r.position, r.rotation))
        .collect();
    if state.infected && state.special_class == SpecialClass::Tank && !state.dead {
        if let Some((id, position)) = tank.held_rock(body.feet, body.yaw) {
            rocks.push((id, position, Quat::from_rotation_y(body.yaw)));
        }
    }
    rocks.extend(ai_rocks.rocks.iter().map(|r| (r.id | 0x8000_0000, r.position, r.rotation)));
    rocks.extend(ai_rocks.held.iter().map(|&(id, p, q)| (id | 0x8000_0000, p, q)));
    rocks.extend(ai_rocks.net.iter().map(|&(id, p, q)| (id | 0x4000_0000, p, q)));
    let death = tank.death.as_ref().filter(|_| state.infected && state.dead);
    let throwing = tank
        .throw
        .as_ref()
        .filter(|_| state.tank_throwing())
        .map(|t| (THROWS[t.variant].0, t.clock));
    let throw_transform = Transform::from_translation(body.feet)
        .with_rotation(Quat::from_rotation_y(body.yaw + std::f32::consts::FRAC_PI_2));
    let mut present_rocks = vec![];
    let mut death_drawn = false;
    let mut throw_drawn = false;
    for (entity, kind, mesh, mut transform) in &mut draws {
        match kind {
            TankDraw::Rock(id) => match rocks.iter().find(|r| r.0 == *id) {
                Some(&(_, position, rotation)) => {
                    transform.translation = position;
                    transform.rotation = rotation;
                    present_rocks.push(*id);
                }
                None => commands.entity(entity).despawn(),
            },
            TankDraw::Death(index) => match death {
                Some(d) => {
                    death_drawn = true;
                    if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
                        library.scripted(MODEL, *index, d.sequence, d.clock, &mut mesh);
                    }
                }
                None => commands.entity(entity).despawn(),
            },
            TankDraw::Throw(index) => match throwing {
                Some((sequence, clock)) => {
                    throw_drawn = true;
                    *transform = throw_transform;
                    if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
                        library.scripted(MODEL, *index, sequence, clock, &mut mesh);
                    }
                }
                None => commands.entity(entity).despawn(),
            },
        }
    }
    if let Some(parts) = models.0.get(&(ROCK_MODEL.into(), 0, 0)) {
        for &(id, position, rotation) in rocks.iter().filter(|r| !present_rocks.contains(&r.0)) {
            for (mesh, material) in parts {
                commands.spawn((
                    TankDraw::Rock(id),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(position).with_rotation(rotation),
                    bevy::camera::visibility::RenderLayers::layer(1),
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
    }
    if let (Some((sequence, clock)), false) = (throwing, throw_drawn) {
        if let Some(parts) = models.0.get(&(MODEL.into(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.scripted(MODEL, index, sequence, clock, &mut owned);
                commands.spawn((
                    TankDraw::Throw(index),
                    Mesh3d(meshes.add(owned)),
                    MeshMaterial3d(material.clone()),
                    throw_transform,
                    bevy::camera::visibility::RenderLayers::layer(1),
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
    }
    if let (Some(d), false) = (death, death_drawn) {
        if let Some(parts) = models.0.get(&(MODEL.into(), 0, 0)) {
            let transform = Transform::from_translation(d.feet)
                .with_rotation(Quat::from_rotation_y(d.yaw + std::f32::consts::FRAC_PI_2));
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.scripted(MODEL, index, d.sequence, d.clock, &mut owned);
                commands.spawn((
                    TankDraw::Death(index),
                    Mesh3d(meshes.add(owned)),
                    MeshMaterial3d(material.clone()),
                    transform,
                    bevy::camera::visibility::RenderLayers::layer(1),
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
    }
}
/// Retail death cam: third person around the dead Tank's body.
fn death_camera(
    state: Res<Match>,
    tank: Res<Tank>,
    collision: Option<Res<Collision>>,
    mut camera: Single<(&Body, &mut Transform), With<crate::viewer::GameCamera>>,
) {
    let (body, transform) = &mut *camera;
    // Rock throw: orbit the rooted Tank while it picks up and hurls the rock.
    if state.tank_throwing() {
        **transform = super::pin_camera(body.feet, body.yaw, body.pitch, collision.as_deref());
        return;
    }
    let Some(death) = tank.death.as_ref() else {
        return;
    };
    if !state.infected || !state.dead || state.special_class != SpecialClass::Tank {
        return;
    }
    **transform = super::pin_camera(death.feet, body.yaw, body.pitch, collision.as_deref());
}
#[derive(Component)]
struct FrustrationRoot;
#[derive(Component)]
struct FrustrationFill;
#[derive(Component)]
struct FrustrationWarning;
/// Retail resource/ui/hud/frustrationmeter.res geometry (640x480 HUD
/// units; HudFrustrationMeter at x 10, vertically centred) and the installed
/// left4dead2_english.txt labels.
fn frustration_hud(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    tank: Res<Tank>,
    frontend: Res<crate::game_menu::Frontend>,
    camera: Query<Entity, With<crate::game::ViewCamera>>,
    mut roots: Query<&mut Node, (With<FrustrationRoot>, Without<FrustrationFill>)>,
    mut fills: Query<&mut Node, (With<FrustrationFill>, Without<FrustrationRoot>)>,
    mut warnings: Query<&mut Visibility, (With<FrustrationWarning>, Without<FrustrationRoot>)>,
) {
    let show = state.infected
        && state.special_class == SpecialClass::Tank
        && !state.dead
        && !hunter.ghost
        && frontend.playing();
    let vh = |units: f32| Val::Vh(units / 4.8);
    if roots.is_empty() {
        if !show {
            return;
        }
        let Some(cam) = camera.iter().next() else {
            return;
        };
        let text = |s: &str, top: f32, size: f32| {
            (
                Text::new(s),
                TextFont {
                    font_size: FontSize::Vh(size / 4.8),
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::ZERO,
                    top: vh(top),
                    ..default()
                },
            )
        };
        commands
            .spawn((
                FrustrationRoot,
                UiTargetCamera(cam),
                Node {
                    position_type: PositionType::Absolute,
                    left: vh(10.),
                    top: Val::Percent(50.),
                    width: vh(300.),
                    height: vh(84.),
                    ..default()
                },
            ))
            .with_children(|root| {
                root.spawn((text("ATTACK THE SURVIVORS", 4., 14.), FrustrationWarning));
                root.spawn((
                    text("You must attack or you will", 22., 10.),
                    FrustrationWarning,
                ));
                root.spawn((
                    text("lose control of the Tank", 34., 10.),
                    FrustrationWarning,
                ));
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::ZERO,
                        top: vh(53.),
                        width: vh(150.),
                        height: vh(8.),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0., 0., 0., 0.6)),
                ))
                .with_children(|bar| {
                    bar.spawn((
                        FrustrationFill,
                        Node {
                            position_type: PositionType::Absolute,
                            right: Val::ZERO,
                            top: Val::ZERO,
                            width: Val::Percent(100.),
                            height: Val::Percent(100.),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.85, 0.85, 0.85)),
                    ));
                });
                root.spawn(text("CONTROL", 63., 10.));
            });
        return;
    }
    for mut node in &mut roots {
        let display = if show { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // The bar shows remaining control (east aligned, shrinking).
    let width = Val::Percent((1. - tank.frustration) * 100.);
    for mut node in &mut fills {
        if node.width != width {
            node.width = width;
        }
    }
    let warn = if tank.frustration >= FRUSTRATION_WARN {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut warnings {
        visibility.set_if_neq(warn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tank_music_plays_only_while_the_spawned_tank_is_alive_in_gameplay() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        let mut hunter = Hunter::default();
        state.infected = true;
        state.special_class = SpecialClass::Tank;
        assert!(music_wanted(&state, &hunter, true));
        assert!(!music_wanted(&state, &hunter, false));
        state.dead = true;
        assert!(!music_wanted(&state, &hunter, true));
        state.dead = false;
        hunter.ghost = true;
        assert!(!music_wanted(&state, &hunter, true));
        hunter.ghost = false;
        state.special_class = SpecialClass::Hunter;
        assert!(!music_wanted(&state, &hunter, true));
        state.special_class = SpecialClass::Tank;
        state.infected = false;
        assert!(!music_wanted(&state, &hunter, true));
    }
    fn member(position: Vec3) -> Companion {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.dead = false;
        Companion {
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
    fn retail_tank_punch_and_rock_damage_follow_server_table() {
        assert_eq!(punch_damage(true, 1, false), 24.);
        assert_eq!(punch_damage(true, 3, true), 24.);
        assert_eq!(punch_damage(false, 1, false), 24.);
        assert_eq!(punch_damage(false, 2, false), 33.);
        assert_eq!(punch_damage(false, 3, false), 250.);
        assert_eq!(punch_damage(false, 0, true), 75.);
        assert_eq!(punch_damage(false, 3, true), 150.);
        assert_eq!(HEALTH, 4000. * 1.5);
    }
    #[test]
    fn punch_fling_is_800_units_away_from_the_tank_with_point_two_lift() {
        let v = fling_velocity(Vec3::ZERO, Vec3::new(1., 0.3, 0.), Vec3::NEG_Z) / UNIT;
        assert!((v.length() - 800.).abs() < 0.01);
        let expected_up = 800. * 0.2 / (1. + 0.04_f32).sqrt();
        assert!((v.y - expected_up).abs() < 0.01);
        assert!(v.x > 780. && v.z.abs() < 1e-3);
        // Overlapping victim falls back to the Tank's facing.
        let v = fling_velocity(Vec3::ZERO, Vec3::ZERO, Vec3::Z) / UNIT;
        assert!(v.z > 780.);
        // Flat-ground flight: 18-unit lift, gravity 800 → about 9-10 m.
        let mut p = Vec3::Y * FLING_LIFT * UNIT;
        let mut v = fling_velocity(Vec3::ZERO, Vec3::X, Vec3::X);
        let dt = 1. / 60.;
        while p.y > 0. {
            v.y -= crate::player_movement::GRAVITY * dt;
            p += v * dt;
        }
        assert!((8.5..10.5).contains(&p.x), "{}", p.x);
    }
    #[test]
    fn punch_reach_covers_the_front_arc_within_swing_range_and_fist_radius() {
        let feet = Vec3::ZERO;
        let forward = Vec3::NEG_Z;
        assert!(in_punch_reach(feet, 0., forward * 80. * UNIT, HEIGHT));
        assert!(!in_punch_reach(feet, 0., forward * 95. * UNIT, HEIGHT));
        // 180° arc: sideways counts, behind does not.
        assert!(in_punch_reach(feet, 0., Vec3::X * 60. * UNIT, HEIGHT));
        assert!(!in_punch_reach(feet, 0., Vec3::Z * 60. * UNIT, HEIGHT));
        // Yaw rotates the arc.
        assert!(in_punch_reach(
            feet,
            std::f32::consts::PI,
            Vec3::Z * 60. * UNIT,
            HEIGHT
        ));
        // Height band: a survivor far below or above is missed.
        assert!(!in_punch_reach(
            feet,
            0.,
            forward * 50. * UNIT + Vec3::Y * 130. * UNIT,
            HEIGHT
        ));
        assert!(!in_punch_reach(
            feet,
            0.,
            forward * 50. * UNIT - Vec3::Y * 100. * UNIT,
            HEIGHT
        ));
    }
    #[test]
    fn burning_tank_lives_for_the_retail_burn_duration() {
        use crate::campaign_settings::Difficulty::*;
        assert_eq!(burn_duration(Normal), 75.);
        assert_eq!(burn_duration(Advanced), 80.);
        assert_eq!(burn_duration(Expert), 85.);
        assert!(burns_by_duration(MODEL));
        assert!(!burns_by_duration("models/infected/common_male01.mdl"));
        // Full Normal health drains in exactly tank_burn_duration.
        let health = super::super::ai::tank_health(Normal);
        let dt = 1. / 30.;
        let mut h = health;
        let mut t = 0.;
        while h > 0. {
            h -= health / burn_duration(Normal) * dt;
            t += dt;
        }
        assert!((t - 75.).abs() < 0.05, "{t}");
    }
    #[test]
    fn ai_rock_launch_reaches_its_target_within_the_loft_limit() {
        let start = Vec3::new(0., 2., 0.);
        for target in [Vec3::new(0., 1., -10.), Vec3::new(8., 0.5, -20.), Vec3::new(-5., 4., -15.)] {
            let velocity = rock_launch(start, target);
            assert!((velocity.length() - THROW_FORCE * UNIT).abs() < 1e-3);
            let direct = (target - start).y.atan2((target - start).xz().length());
            let angle = velocity.y.atan2(velocity.xz().length());
            assert!(angle <= direct + 30f32.to_radians() + 1e-4);
            let mut rock = Rock::launched(1, start, velocity, Vec3::ZERO);
            let mut closest = f32::INFINITY;
            for _ in 0..240 {
                let next = rock.advance(1. / 60.);
                closest = closest.min(closest_point(target, rock.position, next).distance(target));
                rock.position = next;
            }
            assert!(closest < 0.3, "missed {target:?} by {closest}");
        }
    }
    #[test]
    fn rock_throw_is_third_person_without_claws() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        state.special_class = SpecialClass::Tank;
        for (phase, throwing) in [(0, false), (2, true), (4, false), (6, true)] {
            state.special_phase = phase;
            assert_eq!(state.tank_throwing(), throwing, "phase {phase}");
            assert_eq!(state.infected_arms_hidden(), throwing, "phase {phase}");
        }
        state.special_class = SpecialClass::Boomer;
        state.special_phase = 2;
        assert!(!state.tank_throwing());
    }
    #[test]
    fn swing_and_throw_intervals_match_retail_cvars() {
        // Hit: next swing windup+interval; miss refires 0.5 s sooner.
        assert_eq!(WINDUP + SWING_INTERVAL, 2.);
        assert_eq!(
            WINDUP + SWING_INTERVAL - (SWING_INTERVAL - MISS_INTERVAL),
            1.5
        );
        assert_eq!(THROW_INTERVAL + 1., 6.);
        for (_, duration, create, release) in THROWS {
            assert!(THROW_VM_SWING < create && create < release && release < duration);
        }
        for (i, (_, _, create, release)) in THROWS.into_iter().enumerate() {
            let end = DEBRIS_START[i] + (DEBRIS_PATHS[i].len() - 1) as f32 / 15.;
            assert!(DEBRIS_START[i] <= create + 1e-3 && create - DEBRIS_START[i] < 1. / 15.);
            assert!(end + 1e-3 >= release && end - release < 1. / 15., "{i}");
        }
    }
    #[test]
    fn rock_hull_grows_and_hits_within_retail_radius() {
        assert!((rock_half_extent(0.) / UNIT - 4.).abs() < 1e-4);
        assert!((rock_half_extent(0.25) / UNIT - 10.).abs() < 1e-4);
        assert!((rock_half_extent(2.) / UNIT - 16.).abs() < 1e-4);
        let collision = Collision::test_solid_floor();
        let mut tank = Tank::default();
        let mut squad = Squad(vec![member(Vec3::new(0., 0., -10.))]);
        let mut sounds = crate::game_audio::Queue::default();
        // Passing 90 units to the side still detonates on the survivor.
        tank.rocks.push(Rock {
            id: 1,
            health: ROCK_HEALTH,
            position: Vec3::new(90. * UNIT, 1.2, 0.),
            previous: Vec3::new(90. * UNIT, 1.2, 0.),
            velocity: Vec3::NEG_Z * THROW_FORCE * UNIT + Vec3::Y * 3.,
            age: 1.,
            spin: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        });
        for _ in 0..60 {
            update_rocks(&mut tank, &collision, &mut squad, &mut sounds, 1. / 60.);
        }
        assert!(tank.rocks.is_empty());
        assert_eq!(squad.0[0].state.health, 100. - VS_TANK_DAMAGE);
        assert_eq!(squad.0[0].state.pinned_by, Some(SpecialClass::Tank));
        assert_eq!(tank.victims[0].kind, Knock::Rock);
        // Rock knockdown pose, then release after the native getup.
        let m = &squad.0[0];
        assert_eq!(victim_activity(m, "", 0., 0.).unwrap().0, "tank_rock");
        for _ in 0..100 {
            update_victims(&mut tank, &collision, &mut squad, 1. / 60.);
        }
        assert!(!squad.0[0].state.pinned && tank.victims.is_empty());
        assert_eq!(
            victim_activity(&squad.0[0], "tank_rock", 1., 1.5)
                .unwrap()
                .0,
            "tank_rock"
        );
        assert!(victim_activity(&squad.0[0], "tank_rock", 1.6, 1.5).is_none());
        // A rock passing beyond 100 units misses and falls to the floor.
        let mut squad = Squad(vec![member(Vec3::new(0., 0., -10.))]);
        let mut tank = Tank::default();
        tank.rocks.push(Rock {
            id: 2,
            health: ROCK_HEALTH,
            position: Vec3::new(120. * UNIT, 1., 0.),
            previous: Vec3::new(120. * UNIT, 1., 0.),
            velocity: Vec3::NEG_Z * THROW_FORCE * UNIT,
            age: 1.,
            spin: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        });
        for _ in 0..240 {
            update_rocks(&mut tank, &collision, &mut squad, &mut sounds, 1. / 60.);
        }
        assert!(tank.rocks.is_empty());
        assert_eq!(squad.0[0].state.health, 100.);
    }
    #[test]
    fn punched_survivor_flies_lands_and_gets_up_with_native_activities() {
        let collision = Collision::test_solid_floor();
        let mut tank = Tank::default();
        let mut squad = Squad(vec![member(Vec3::new(0., 0.004, -1.5))]);
        let mut sounds = crate::game_audio::Queue::default();
        punch_survivor(
            &mut tank,
            &mut squad,
            0,
            Vec3::new(0., 0.004, 0.),
            Vec3::NEG_Z,
            &collision,
            &mut sounds,
        );
        assert_eq!(squad.0[0].state.health, 100. - VS_TANK_DAMAGE);
        assert!(squad.0[0].state.pinned && squad.0[0].vertical > 0.);
        let m = &squad.0[0];
        assert_eq!(victim_activity(m, "", 0., 0.).unwrap().0, "tank_hit");
        assert_eq!(
            victim_activity(m, "tank_hit", 0.2, 0.1667).unwrap().0,
            "tank_fly"
        );
        let start = squad.0[0].position;
        let mut steps = 0;
        while tank.victims[0].airborne {
            update_victims(&mut tank, &collision, &mut squad, 1. / 60.);
            steps += 1;
            assert!(steps < 120);
        }
        let travelled = (squad.0[0].position - start).length();
        assert!(
            travelled > 7. && squad.0[0].position.z < start.z,
            "{travelled}"
        );
        assert_eq!(squad.0[0].vertical, 0.);
        assert_eq!(
            victim_activity(&squad.0[0], "tank_fly", 1., 4.).unwrap().0,
            "tank_land"
        );
        for _ in 0..95 {
            update_victims(&mut tank, &collision, &mut squad, 1. / 60.);
        }
        assert!(!squad.0[0].state.pinned);
        // Incapacitated survivors are pounded, not flung.
        let mut squad = Squad(vec![member(Vec3::new(0., 0.004, -1.5))]);
        squad.0[0].state.incap = true;
        let mut tank = Tank::default();
        punch_survivor(
            &mut tank,
            &mut squad,
            0,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &collision,
            &mut sounds,
        );
        assert!(!squad.0[0].state.pinned && tank.victims.is_empty());
        assert_eq!(squad.0[0].state.incap_health, 300. - VS_TANK_DAMAGE);
    }
    #[test]
    fn frustration_fills_out_of_sight_in_twenty_seconds_and_not_while_burning() {
        let mut f = 0.;
        for _ in 0..19 {
            f = frustration_step(f, 3., false);
        }
        assert!(f < 1.);
        f = frustration_step(f, 3., false);
        assert!(f >= 1.);
        assert_eq!(frustration_step(0.5, 1., false), 0.5);
        assert_eq!(frustration_step(0.5, 5., true), 0.5);
    }
    #[test]
    fn hittable_prop_force_scales_with_mass_like_retail() {
        let light = prop_impulse(50.);
        assert!((light / UNIT - 999.9 * 300.).abs() < 1.);
        assert!((prop_impulse(1500.) / light - 4.).abs() < 1e-4);
        assert!((prop_impulse(800.) / light - 2.5).abs() < 1e-4);
    }
    #[test]
    fn model_points_follow_tank_yaw() {
        let p = model_point(Vec3::ZERO, 0., [100., 0., 0.]);
        assert!((p - Vec3::NEG_Z * 100. * UNIT).length() < 1e-4);
        let left = model_point(Vec3::ZERO, 0., [0., 100., 50.]);
        assert!((left - Vec3::new(-100., 50., 0.) * UNIT).length() < 1e-4);
        let turned = model_point(Vec3::ZERO, std::f32::consts::FRAC_PI_2, [100., 0., 0.]);
        assert!((turned - Vec3::NEG_X * 100. * UNIT).length() < 1e-4);

        assert_eq!(vm_activity(6), Some("ACT_VM_PRIMARYATTACK"));
    }
    #[test]
    fn installed_tank_assets_have_sequences_sounds_and_hud() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            MODEL,
            VIEW,
            ROCK_MODEL,
            "materials/vgui/hud/pz_charge_tank.vtf",
            "resource/ui/hud/tankhealth.res",
            "resource/ui/hud/frustrationmeter.res",
        ] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let english = String::from_utf8_lossy(
            &assets
                .read("resource/left4dead2_english.txt")
                .unwrap()
                .iter()
                .copied()
                .filter(|b| *b != 0)
                .collect::<Vec<_>>(),
        )
        .to_string();
        for label in [
            "ATTACK THE SURVIVORS",
            "You must attack or you will",
            "lose control of the Tank",
            "\"CONTROL\"",
        ] {
            assert!(english.contains(label), "{label}");
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "hulkzombie.yell",
            "hulkzombie.die",
            "hulkzombie.growl",
            "hulkzombie.pain",
            "hulkzombie.attack",
            "hulkzombie.punch",
            "hulkzombie.punchincap",
            "hulkzombie.thud",
            "hulkzombie.throw",
            "hulkzombie.throw.pickup",
            "hulkzombie.throw.flyloop",
            "hulkzombie.thrownprojectilehit",
            "event.tank",
            "claw.hit",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
        }
        let music = assets.read("sound/music/tank/tank.wav").unwrap();
        // 16-bit stereo 44.1 kHz data chunk length → MUSIC_LENGTH.
        let data = music.windows(4).position(|w| w == b"data").unwrap();
        let bytes = u32::from_le_bytes(music[data + 4..data + 8].try_into().unwrap());
        assert!((bytes as f32 / (44100. * 4.) - MUSIC_LENGTH).abs() < 0.01);
        assert!(!registry.definitions.contains_key("hulkzombie.alert"));
        let assets = std::cell::RefCell::new(assets);
        let load = |path: &str| {
            l4d2_formats::anim::Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok())
                .unwrap()
        };
        let duration = |sk: &l4d2_formats::anim::Skeleton, i: usize| {
            sk.sequence_anim(i).map_or(0., |a| sk.anims()[a].duration())
        };
        let view = load(VIEW);
        for activity in ["ACT_VM_DEPLOY", "ACT_VM_IDLE", "ACT_VM_PRIMARYATTACK"] {
            assert!(view.sequences().iter().any(|s| s.activity_name == activity));
        }
        let melee = view.find_sequence("melee").unwrap();
        assert!((duration(&view, melee) - 1.).abs() < 0.01);
        let hulk = load(MODEL);
        for (i, (name, total, create, release)) in THROWS.into_iter().enumerate() {
            let seq = hulk.find_sequence(name).unwrap();
            let s = &hulk.sequences()[seq];
            assert_eq!(s.activity_name, "ACT_TANK_OVERHEAD_THROW");
            let d = duration(&hulk, seq);
            assert!((d - total).abs() < 0.01, "{name}");
            let event = |n: &str| s.events.iter().find(|e| e.options == n).unwrap().cycle * d;
            assert!((event("AE_ATTACK_HIT") - create).abs() < 0.01, "{name}");
            assert!((event("AE_ATTACK_END") - release).abs() < 0.01, "{name}");
            // Baked debris attachment path matches the installed animation.
            for (k, sample) in DEBRIS_PATHS[i].iter().enumerate() {
                let t = ((DEBRIS_START[i] + k as f32 / 15.) * 15.).round() / 15.;
                let pose = hulk.pose_sequence(seq, t, false);
                let p = hulk
                    .attachment_matrix("debris", &pose)
                    .unwrap()
                    .w_axis
                    .truncate();
                assert!(
                    (Vec3::from_array(p.to_array()) - Vec3::from_array(*sample)).length() < 0.6,
                    "{name} {k} {p:?}"
                );
            }
        }
        for name in ["Death_11ab", "Death_Running_07"] {
            assert!(hulk.find_sequence(name).is_some(), "{name}");
        }
        for character in [
            "gambler",
            "producer",
            "coach",
            "mechanic",
            "namvet",
            "teenangst",
            "biker",
            "manager",
        ] {
            let sk = load(&format!("models/survivors/survivor_{character}.mdl"));
            for (activity, expected) in [
                ("ACT_TERROR_HIT_BY_TANKPUNCH", None),
                ("ACT_TERROR_IDLE_FALL_FROM_TANKPUNCH", None),
                ("ACT_TERROR_TANKPUNCH_LAND", Some(GETUP)),
                ("ACT_TERROR_TANKROCK_TO_STAND", Some(GETUP)),
            ] {
                let i = sk
                    .sequences()
                    .iter()
                    .position(|s| s.activity_name == activity)
                    .unwrap_or_else(|| panic!("{character} {activity}"));
                if let Some(expected) = expected {
                    assert!((duration(&sk, i) - expected).abs() < 0.01);
                }
            }
        }
    }
}
