//! Playable Spitter: spit projectile, spreading acid pool (retail entity
//! `insect_swarm`, a `CInferno` subtype), death puddle, claw melee and simple
//! survivor-bot acid avoidance.
//!
//! Numbers come from the user's running retail game (cvars) and the owned
//! server.dll decompilation; see local-research/si-agents/spitter/NOTES.md.
//! The shared lifecycle (team switch, ghost spawn, death/respawn, survivor-bot
//! opposition) lives in special_infected.rs.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, DUCK_HEIGHT, HEIGHT, RADIUS, UNIT},
    game::{Body, CompanionActor},
    game_state::{Match, Squad},
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use l4d2r_game_core::{
    entities::{Ent, EntityWorld, Kind as EntityKind},
    mapents::MapEnt,
};
pub const WEAPON: &str = "weapon_spitter_claw";
pub const MODEL: &str = "models/infected/spitter.mdl";
pub const VIEW: &str = "models/weapons/arms/v_spitter_arms.mdl";
/// Retail z_spitter_health = 100.
pub const HEALTH: f32 = 100.;
/// Retail z_spitter_speed = 210.
pub const SPEED: f32 = 210.;
pub const PLAYABLE: bool = true;
/// The spit uses the viewmodel's ACT_VM_VOMIT sequence (the only ability
/// activity in v_spitter_arms.mdl); the claw uses ACT_VM_PRIMARYATTACK.
pub const VM_ACTIONS: &[(&str, &str)] =
    &[("spit", "ACT_VM_VOMIT"), ("claw", "ACT_VM_PRIMARYATTACK")];
pub const CONTROLS: &str =
    "Spitter playtest: left click spits acid (20s recharge); right click claws.";
/// special_phase used while the spit animation plays. Phase 2 would freeze
/// player movement (Hunter flight); the retail Spitter keeps walking.
pub const SPIT_PHASE: u8 = 6;
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    (phase == SPIT_PHASE).then_some("ACT_VM_VOMIT")
}

// Retail cvars (build 10097 netconsole `find spit`, `find inferno`, `find claw`).
/// z_spit_interval: recharge after a spit.
pub const SPIT_INTERVAL: f32 = 20.;
/// z_spit_latency: delay from attack press to projectile launch.
pub const SPIT_LATENCY: f32 = 0.3;
/// spitter_spitting (ACT_TERROR_SPITTER_SPIT, 1.4 s) runs as the main
/// activity until its cycle passes 0.99 (server 1033f450).
pub const SPIT_ANIMATION: f32 = 1.4;
pub const SPIT_ANIMATION_END: f32 = 0.99;
/// z_spit_velocity (Source units/sec).
pub const SPIT_VELOCITY: f32 = 900.;
/// z_spit_detonate_delay: a resting projectile detonates after this.
pub const DETONATE_DELAY: f32 = 0.1;
/// z_spit_spread_delay: CInsectSwarm spread think interval.
pub const SPREAD_DELAY: f32 = 0.2;
/// spitter_pz_claw_dmg, claw_range, claw_range_down, claw_swing_*.
pub const CLAW_DAMAGE: f32 = 4.;
pub const CLAW_RANGE: f32 = 52.;
pub const CLAW_RANGE_DOWN: f32 = 70.;
pub const CLAW_SWING_DURATION: f32 = 0.2;
pub const CLAW_HIT_INTERVAL: f32 = 1.;
pub const CLAW_MISS_INTERVAL: f32 = 0.5;
// Retail inferno cvars used by CInsectSwarm.
/// CInsectSwarm::Spawn (103862e0) sets the flame limit to 10 (it would
/// otherwise be inferno_max_flames = 32). Retail AI spits on open ground
/// grew 1, 2, 4, 8, 10 flames at 0.2s spacing (spitter-live-ai-open.log).
pub const MAX_FLAMES: usize = 10;
/// The Spitter's death puddle sets its flame limit to 2 (server 10336010).
pub const DEATH_FLAMES: usize = 2;
/// inferno_child_spawn_max_depth.
const MAX_DEPTH: u8 = 4;
/// inferno_per_flame_spawn_duration.
const PER_FLAME_SPAWN_DURATION: f32 = 5.;
/// inferno_max_range.
const MAX_RANGE: f32 = 500.;
/// inferno_acid_spawn_angle (degrees either side of the parent direction).
const ACID_SPAWN_ANGLE: f32 = 120.;
/// inferno_max_trace_per_tick (development-only cvar; binary default 16).
const TRACE_BUDGET: i32 = 16;
// Constants read from the owned server.dll.
/// CSpitterProjectile gravity scale (0x10605a40) times sv_gravity 800.
const PROJECTILE_GRAVITY: f32 = 0.75 * 800.;
/// The projectile flies with MOVETYPE_FLY for 1s, then gets gravity and
/// keeps 30% of its horizontal speed (think 103e2830).
const STRAIGHT_FLIGHT: f32 = 1.;
const LATE_HORIZONTAL: f32 = 0.3;
/// Below this speed (sqrt of 25) the projectile counts as resting.
const REST_SPEED: f32 = 5.;
/// Spawn point: eye + forward * 24 units (103664a0).
const MOUTH_OFFSET: f32 = 24.;
/// Child flame distance RandomFloat(50, 75) (CInferno spread 10384b40).
const SPAWN_DISTANCE: [f32; 2] = [50., 75.];
/// Flame width 30 / height 80 (CInferno::Spawn 10386130); the damage
/// radius is width * 2 around the flame centre at half height.
const FLAME_WIDTH: f32 = 30.;
const FLAME_HEIGHT: f32 = 80.;
/// Flame lifetime 7s (CInsectSwarm vtable 0x334, constant 0x105d8700).
pub const LIFETIME: f32 = 7.;
/// CInferno damage think interval 0.2s (0x105d50cc).
pub const DAMAGE_INTERVAL: f32 = 0.2;
/// Damage fade-in countdown of 2s started in CInferno::Spawn.
pub const DAMAGE_RAMP: f32 = 2.;
/// CInsectSwarm damage-per-second table indexed by whole seconds of pool
/// age with linear interpolation (10382f30, table 0x107b4b04). This is the
/// Versus value; non-Versus modes halve the integer-second base value.
pub const DPS: [f32; 9] = [0., 5., 10., 20., 30., 30., 20., 7., 0.];
/// Survivor run speed used while a bot walks out of acid.
const ESCAPE_SPEED: f32 = 220.;
/// Effect emitter pools.
const PROJECTILE_FX: usize = 2;
const FLAME_FX: usize = 12;
const PROJECTILE_EFFECT: &str = "spitter_projectile";
const POOL_EFFECT: &str = "spitter_areaofdenial";
fn u(v: f32) -> f32 {
    v * UNIT
}
/// Damage per second of a pool `age` seconds old (before the fade-in).
pub fn dps(age: f32) -> f32 {
    if !(age >= 0.) {
        return 0.;
    }
    let i = age as usize;
    if i + 1 >= DPS.len() {
        return 0.;
    }
    DPS[i] + (DPS[i + 1] - DPS[i]) * (age - i as f32)
}
/// Damage applied by one 0.2s think of a pool `age` seconds old. Survivor
/// health is an integer in retail: the fractional part is lost, which the
/// retail trace (local-research/parity/spitter-acid-coop.log) confirms.
pub fn tick_damage(age: f32) -> f32 {
    (dps(age) * DAMAGE_INTERVAL * (age / DAMAGE_RAMP).clamp(0., 1.) + 1e-4).floor()
}
#[derive(Clone, Debug)]
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}
#[derive(Clone, Debug)]
pub struct Projectile {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub gravity: bool,
    rest: f32,
    fx: Option<usize>,
    /// Multiplayer guest: a copy of the host's spit, drawn only (the host's
    /// "acid" message makes the pool).
    pub net: bool,
}
#[derive(Debug, PartialEq)]
pub enum Flight {
    Flying,
    Bounced,
    Detonate(Vec3),
}
impl Projectile {
    pub fn new(position: Vec3, velocity: Vec3) -> Self {
        Self {
            position,
            velocity,
            age: 0.,
            gravity: false,
            rest: 0.,
            fx: None,
            net: false,
        }
    }
    /// Swept flight with survivor/world contact (CSpitterProjectile touch
    /// 103e25a0): survivors and ground (normal z > 0.5) detonate, ceilings
    /// drop it, walls bounce it and switch on gravity.
    pub fn step(&mut self, collision: &Collision, survivors: &[(Vec3, f32)], dt: f32) -> Flight {
        let count = (dt / (1. / 120.)).ceil().clamp(1., 16.) as usize;
        let step = dt / count as f32;
        let mut result = Flight::Flying;
        for _ in 0..count {
            self.age += step;
            if !self.gravity && self.age >= STRAIGHT_FLIGHT {
                self.gravity = true;
                self.velocity.x *= LATE_HORIZONTAL;
                self.velocity.z *= LATE_HORIZONTAL;
            }
            let before = self.velocity;
            if self.gravity {
                self.velocity.y -= u(PROJECTILE_GRAVITY) * step;
            }
            let travel = (before + self.velocity) * 0.5 * step;
            let end = self.position + travel;
            if let Some(f) = survivors
                .iter()
                .filter_map(|(feet, height)| segment_box(self.position, end, *feet, *height))
                .min_by(|a, b| a.total_cmp(b))
            {
                return Flight::Detonate(self.position + travel * f);
            }
            let hit = collision.physics_trace(self.position, end, Vec3::splat(u(2.)));
            self.position += travel * hit.fraction;
            if hit.start_solid || hit.fraction < 1. {
                let n = hit.normal;
                if n.y > 0.5 || n.length_squared() < 0.5 {
                    return Flight::Detonate(self.position);
                }
                self.gravity = true;
                if n.y < 0. {
                    self.velocity =
                        Vec3::new(self.velocity.x * 0.5, u(-10.), self.velocity.z * 0.5);
                } else {
                    let into = self.velocity.dot(n);
                    if into < 0. {
                        // Approximate MOVETYPE_FLYGRAVITY bounce.
                        self.velocity -= n * into * 1.5;
                    }
                    result = Flight::Bounced;
                }
                self.position += n * u(0.1);
            }
            if self.velocity.length() < u(REST_SPEED) {
                self.rest += step;
                if self.rest > DETONATE_DELAY {
                    return Flight::Detonate(self.position);
                }
            } else {
                self.rest = 0.;
            }
            if self.age > 10. {
                return Flight::Detonate(self.position);
            }
        }
        result
    }
}
/// Entry fraction of a segment into a survivor hull (feet-anchored AABB).
fn segment_box(from: Vec3, to: Vec3, feet: Vec3, height: f32) -> Option<f32> {
    let min = feet - Vec3::new(RADIUS, 0., RADIUS);
    let max = feet + Vec3::new(RADIUS, height, RADIUS);
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
#[derive(Clone, Debug)]
pub struct Flame {
    pub position: Vec3,
    pub normal: Vec3,
    parent: Option<usize>,
    depth: u8,
    pending: u32,
    spawn_until: f32,
    fx: Option<usize>,
}
impl Flame {
    fn center(&self) -> Vec3 {
        self.position + Vec3::Y * u(FLAME_HEIGHT * 0.5)
    }
    /// CInferno::IsTouching (10383cd0) for a survivor hull: nearest hull
    /// point within width*2 of the flame centre, at most a third of that
    /// above the flame base, and a clear line for players.
    fn touches(&self, feet: Vec3, height: f32, collision: Option<&Collision>) -> bool {
        let radius = u(FLAME_WIDTH * 2.);
        let lo = feet - Vec3::new(RADIUS, 0., RADIUS);
        let hi = feet + Vec3::new(RADIUS, height, RADIUS);
        if self.position.clamp(lo, hi).y - self.position.y > radius / 3. {
            return false;
        }
        let center = self.center();
        let near = center.clamp(lo, hi);
        near.distance_squared(center) < radius * radius
            && collision.is_none_or(|c| c.trace(center, near, Vec3::ZERO).fraction >= 0.99)
    }
}
#[derive(Clone, Debug)]
pub struct Pool {
    pub origin: Vec3,
    pub age: f32,
    pub flames: Vec<Flame>,
    pub max_flames: usize,
    pub id: u64,
    spread_clock: f32,
    damage_clock: f32,
    rotate: usize,
    sound: Option<usize>,
}
impl Pool {
    pub fn new(id: u64, position: Vec3, normal: Vec3, max_flames: usize) -> Self {
        Self {
            origin: position,
            age: 0.,
            flames: vec![Flame {
                position,
                normal,
                parent: None,
                depth: 0,
                pending: 0,
                spawn_until: PER_FLAME_SPAWN_DURATION,
                fx: None,
            }],
            max_flames,
            id,
            spread_clock: SPREAD_DELAY,
            damage_clock: 0.,
            rotate: 0,
            sound: None,
        }
    }
    pub fn alive(&self) -> bool {
        self.age < LIFETIME
    }
    /// CInferno::CreateFire (103841e0): flame limit and spacing.
    fn add(&mut self, position: Vec3, normal: Vec3, parent: usize) -> bool {
        if self.flames.len() >= self.max_flames.min(64) {
            return false;
        }
        let spacing = u(FLAME_WIDTH * 2.);
        if self
            .flames
            .iter()
            .any(|f| f.center().distance_squared(position) < spacing * spacing)
        {
            return false;
        }
        let depth = self.flames[parent].depth + 1;
        self.flames.push(Flame {
            position,
            normal,
            parent: Some(parent),
            depth,
            pending: 0,
            // Children inherit the parent's spawn duration.
            spawn_until: self.age + PER_FLAME_SPAWN_DURATION,
            fx: None,
        });
        true
    }
    /// One CInferno spread think (10384b40).
    fn spread(&mut self, collision: &Collision, rng: &mut Rng) {
        for f in &mut self.flames {
            if self.age < f.spawn_until {
                // inferno_initial_spawn_interval (0.05) is shorter than the
                // 0.2s think, so every think adds one pending child.
                f.pending += 1;
            }
        }
        let mut budget = TRACE_BUDGET;
        let count = self.flames.len();
        let mut last = self.rotate;
        for k in 0..count {
            if budget < 1 {
                break;
            }
            let i = (self.rotate + k) % count;
            last = i;
            let flame = self.flames[i].clone();
            if flame.pending == 0 || flame.depth + 1 >= MAX_DEPTH {
                continue;
            }
            self.flames[i].pending -= 1;
            for _ in 0..4 {
                let mut direction = if let Some(p) = flame.parent {
                    let d = (flame.position - self.flames[p].position).normalize_or_zero();
                    let yaw = d.z.atan2(d.x)
                        + rng.range(-ACID_SPAWN_ANGLE, ACID_SPAWN_ANGLE).to_radians();
                    let pitch = d.y.atan2(d.xz().length());
                    Vec3::new(
                        pitch.cos() * yaw.cos(),
                        pitch.sin(),
                        pitch.cos() * yaw.sin(),
                    )
                } else {
                    let yaw = rng.range(-std::f32::consts::PI, std::f32::consts::PI);
                    Vec3::new(yaw.cos(), 0., yaw.sin())
                };
                direction -= flame.normal * direction.dot(flame.normal);
                let mut candidate =
                    flame.position + direction * u(rng.range(SPAWN_DISTANCE[0], SPAWN_DISTANCE[1]));
                let offset = candidate - self.origin;
                if offset.length() > u(MAX_RANGE) {
                    candidate = self.origin + offset.normalize() * u(MAX_RANGE);
                }
                let start = Vec3::new(candidate.x, candidate.y + u(50.), candidate.z);
                let end = Vec3::new(candidate.x, flame.position.y - u(200.), candidate.z);
                let down = collision.trace(start, end, Vec3::ZERO);
                budget -= 1;
                if down.fraction >= 1. && !down.start_solid {
                    continue;
                }
                let ground = start + (end - start) * down.fraction;
                let sight = collision.trace(
                    flame.position + Vec3::Y * u(30.),
                    ground + Vec3::Y * u(30.),
                    Vec3::ZERO,
                );
                budget -= 1;
                if sight.fraction >= 1. && self.add(ground, down.normal.normalize_or(Vec3::Y), i) {
                    break;
                }
            }
        }
        self.rotate = last + 1;
    }
    /// Advances the pool; returns the survivor damage of a 0.2s damage
    /// think when one happens this step.
    fn advance(&mut self, collision: &Collision, rng: &mut Rng, dt: f32) -> Option<f32> {
        self.age += dt;
        self.spread_clock -= dt;
        while self.spread_clock <= 0. {
            self.spread_clock += SPREAD_DELAY;
            if self.alive() {
                self.spread(collision, rng);
            }
        }
        self.damage_clock -= dt;
        if self.damage_clock <= 0. && self.alive() {
            self.damage_clock += DAMAGE_INTERVAL;
            return Some(tick_damage(self.age));
        }
        None
    }
    pub fn touching(&self, feet: Vec3, height: f32, collision: Option<&Collision>) -> bool {
        self.alive()
            && self
                .flames
                .iter()
                .any(|f| f.touches(feet, height, collision))
    }
    /// Direction a survivor should walk to leave this pool, if inside it.
    pub fn escape(&self, feet: Vec3, height: f32) -> Option<Vec3> {
        let margin = u(FLAME_WIDTH * 2. + 20.);
        let near = self
            .flames
            .iter()
            .filter(|f| (f.position - feet).xz().length() < margin + RADIUS);
        let mut away = Vec3::ZERO;
        let mut inside = false;
        for f in near {
            inside |= f.touches(feet, height, None) || (f.position - feet).xz().length() < margin;
            let d = (feet - f.position) * Vec3::new(1., 0., 1.);
            away += d.normalize_or_zero() / d.length().max(u(4.));
        }
        if !inside || !self.alive() {
            return None;
        }
        if away.length_squared() < 1e-6 {
            away = ((feet - self.origin) * Vec3::new(1., 0., 1.)).normalize_or(Vec3::X);
        }
        Some(away.normalize_or(Vec3::X))
    }
}
/// Local Spitter ability and acid state.
/// Stop ids of the per-pool SpitterZombie.Acid loops.
const ACID_SOUND_BASE: usize = 0x5917_0000;
const BURN_SOUND_BASE: usize = usize::MAX / 2;
#[derive(Resource)]
pub struct Spit {
    /// z_spit_interval recharge remaining.
    pub cooldown: f32,
    /// Countdown to projectile launch after the attack press.
    pub latency: Option<f32>,
    phase_clock: f32,
    claw: Option<f32>,
    pub projectiles: Vec<Projectile>,
    pub pools: Vec<Pool>,
    rng: Rng,
    next_id: u64,
    was_dead: bool,
    epoch: f32,
    /// Team at the last tick: acid is cleared on a team switch, but keeps
    /// simulating for AI Spitters while the player is a survivor.
    infected_team: bool,
    /// Whether the survivor player / each bot stood in acid last tick
    /// (retail OnEnteredSpit: Event.SpitterBurn and GooedBySpitter).
    goo_player: bool,
    burn_sound: Option<usize>,
    burn_serial: usize,
    goo_bots: Vec<bool>,
    capture_fired: bool,
    /// Multiplayer: spits launched (mouth, velocity) and pools made (ground,
    /// normal, flames) since net_flow last sent them.
    pub net_launches: Vec<(Vec3, Vec3)>,
    pub net_pools: Vec<(Vec3, Vec3, usize)>,
}
impl Default for Spit {
    fn default() -> Self {
        Self {
            cooldown: 0.,
            latency: None,
            phase_clock: 0.,
            claw: None,
            projectiles: vec![],
            pools: vec![],
            rng: Rng(0x9e37_79b9_7f4a_7c15),
            next_id: 1,
            was_dead: false,
            epoch: 0.,
            infected_team: false,
            goo_player: false,
            burn_sound: None,
            burn_serial: 0,
            goo_bots: vec![],
            capture_fired: false,
            net_launches: vec![],
            net_pools: vec![],
        }
    }
}
impl Spit {
    /// One owned cue for combined pool contact, with an explicit retail-style
    /// exit fade. A fresh id on re-entry prevents cancelled async loads from
    /// becoming an old cue underneath the new one.
    fn burn_audio(&mut self, inside: bool, sounds: &mut crate::game_audio::Queue) -> Vec<(usize, bool)> {
        if inside == self.goo_player { return vec![]; }
        let mut stops = vec![];
        if let Some(id) = self.burn_sound {
            sounds.1.push(id);
            stops.push((id, !inside));
        }
        if inside {
            self.burn_serial = self.burn_serial.wrapping_add(1);
            let id = BURN_SOUND_BASE.wrapping_add(self.burn_serial);
            self.burn_sound = Some(id);
            sounds.0.push_back(crate::game_audio::Request {
                name: "Event.SpitterBurn".into(), position: None, looping: false,
                gain: 1., map_entity: Some(id), soundlevel: None,
            });
        }
        self.goo_player = inside;
        stops
    }
    fn cancel_audio(&self, sounds: &mut crate::game_audio::Queue) -> Vec<usize> {
        let ids: Vec<_> = self.pools.iter().filter_map(|p|p.sound)
            .chain(self.burn_sound).collect();
        sounds.1.extend(ids.iter().copied());
        ids
    }
    /// HUD meter: 0 while spitting, then the z_spit_interval recharge.
    pub fn progress(&self) -> f32 {
        if self.latency.is_some() {
            0.
        } else {
            (1. - self.cooldown / SPIT_INTERVAL).clamp(0., 1.)
        }
    }
    /// Attack press: true when a spit starts.
    pub fn try_spit(&mut self) -> bool {
        if self.cooldown > 0. || self.latency.is_some() {
            return false;
        }
        self.latency = Some(SPIT_LATENCY);
        true
    }
    /// Retail aim: trace along the view, launch from the mouth toward the
    /// aimed point at z_spit_velocity (CSpitAbility 10366900).
    pub fn launch(&mut self, collision: &Collision, eye: Vec3, forward: Vec3) {
        let mouth_end = eye + forward * u(MOUTH_OFFSET);
        let mouth = eye + (mouth_end - eye) * collision.trace(eye, mouth_end, Vec3::ZERO).fraction;
        let far = eye + forward * u(16384.);
        let aim = eye + (far - eye) * collision.trace(eye, far, Vec3::ZERO).fraction;
        let direction = (aim - mouth).try_normalize().unwrap_or(forward);
        self.projectiles
            .push(Projectile::new(mouth, direction * u(SPIT_VELOCITY)));
        self.net_launches.push((mouth, direction * u(SPIT_VELOCITY)));
        self.cooldown = SPIT_INTERVAL;
    }
    /// Multiplayer guest: the host's spit in flight (drawn only).
    pub fn add_net_projectile(&mut self, position: Vec3, velocity: Vec3) {
        let mut p = Projectile::new(position, velocity);
        p.net = true;
        self.projectiles.push(p);
    }
    /// Multiplayer guest: the host's acid pool (no trace; the host found the
    /// ground).
    pub fn add_net_pool(&mut self, ground: Vec3, normal: Vec3, max_flames: usize) {
        let id = self.next_id;
        self.next_id += 1;
        self.pools.push(Pool::new(id, ground, normal.normalize_or(Vec3::Y), max_flames.min(MAX_FLAMES)));
    }
    /// Detonation: trace down from the impact for the pool surface (103e2a30).
    pub fn detonate(
        &mut self,
        collision: &Collision,
        at: Vec3,
        max_flames: usize,
    ) -> Option<usize> {
        let start = at + Vec3::Y * u(10.);
        let end = at - Vec3::Y * u(4000.);
        let hit = collision.trace(start, end, Vec3::ZERO);
        if hit.fraction >= 1. {
            return None;
        }
        let ground = start + (end - start) * hit.fraction;
        let id = self.next_id;
        self.next_id += 1;
        self.pools.push(Pool::new(
            id,
            ground,
            hit.normal.normalize_or(Vec3::Y),
            max_flames,
        ));
        self.net_pools.push((ground, hit.normal.normalize_or(Vec3::Y), max_flames));
        Some(self.pools.len() - 1)
    }
    /// Advances pools and returns (pool index, damage) for each damage think.
    fn advance_pools(&mut self, collision: &Collision, dt: f32) -> Vec<(usize, f32)> {
        let mut out = vec![];
        for (i, pool) in self.pools.iter_mut().enumerate() {
            if let Some(d) = pool.advance(collision, &mut self.rng, dt) {
                out.push((i, d));
            }
        }
        out
    }
}
/// Flame limit for a spit landing (103e2a30): a full pool outside
/// checkpoints; in one, none until survivors leave the start area, then 2.
/// Retail 103e2a30 in a CHECKPOINT area: Survival/Scavenge (102d9a40 /
/// 102d9ab0) keep the full pool; otherwise no pool until the survivors have
/// left the start area (1025c9e0), then a 2-flame one.
fn checkpoint_flames(in_checkpoint: bool, left_safe_area: bool, survival_or_scavenge: bool) -> Option<usize> {
    match (in_checkpoint, survival_or_scavenge, left_safe_area) {
        (false, _, _) | (true, true, _) => Some(MAX_FLAMES),
        (true, false, false) => None,
        (true, false, true) => Some(DEATH_FLAMES),
    }
}
fn member_height(m: &crate::game_state::Companion) -> f32 {
    if m.state.incap { DUCK_HEIGHT } else { HEIGHT }
}
/// Applies one damage think of `pool` to every touching survivor bot.
fn damage_survivors(
    pool: &Pool,
    damage: f32,
    squad: &mut Squad,
    collision: Option<&Collision>,
) -> usize {
    let mut hurt = 0;
    for member in &mut squad.0 {
        if member.state.dead {
            continue;
        }
        if pool.touching(member.position, member_height(member), collision) {
            if damage > 0. {
                member.state.damage(damage);
            }
            hurt += 1;
        }
    }
    hurt
}
/// Simple bot avoidance: a free survivor bot inside acid walks straight
/// out of it. No nav-mesh path search (retail SurvivorEscapeSpit).
fn escape_acid(pools: &[Pool], squad: &mut Squad, collision: &Collision, dt: f32) -> Vec<usize> {
    let mut moved = vec![];
    for (i, member) in squad.0.iter_mut().enumerate() {
        if member.state.dead
            || member.state.incap
            || member.state.pinned
            || crate::game_state::bots_stopped()
        {
            continue;
        }
        let Some(direction) = pools
            .iter()
            .filter_map(|p| p.escape(member.position, HEIGHT))
            .reduce(|a, b| a + b)
            .and_then(|d| d.try_normalize())
        else {
            continue;
        };
        member.position = collision.walk(
            member.position,
            direction * u(ESCAPE_SPEED) * dt,
            HEIGHT,
            collision.grounded(member.position, HEIGHT),
        );
        member.aim = direction;
        moved.push(i);
    }
    moved
}
fn claw_victim(
    squad: &Squad,
    collision: &Collision,
    eye: Vec3,
    forward: Vec3,
    pitch: f32,
) -> Option<usize> {
    // claw_range, extended toward claw_range_down when looking down.
    let range = u(CLAW_RANGE + (CLAW_RANGE_DOWN - CLAW_RANGE) * (-pitch.sin()).max(0.));
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .filter_map(|(i, m)| {
            let lo = m.position - Vec3::new(RADIUS, 0., RADIUS);
            let hi = m.position + Vec3::new(RADIUS, member_height(m), RADIUS);
            let near = eye.clamp(lo, hi);
            let center = m.position + Vec3::Y * member_height(m) * 0.6;
            let distance = near.distance(eye);
            (distance < range
                && forward.dot((center - eye).normalize_or_zero()) > 0.6
                && collision.trace(eye, center, Vec3::ZERO).fraction >= 0.99)
                .then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
fn source_origin(p: Vec3) -> l4d2r_math::Vec3 {
    l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT
}
fn fx_name(kind: &str, slot: usize) -> String {
    format!("__rewrite_spitter_fx_{kind}_{slot}")
}
/// Pooled native PCF emitters (spitter_fx.pcf), created with the map world.
pub fn prepare(world: &mut EntityWorld) {
    for (kind, effect, count) in [
        ("projectile", PROJECTILE_EFFECT, PROJECTILE_FX),
        ("pool", POOL_EFFECT, FLAME_FX),
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
fn set_fx(world: &mut EntityWorld, id: usize, position: Option<Vec3>, restart: bool) {
    let time = world.time;
    if let Some(e) = world.ents.get_mut(id) {
        match position {
            Some(p) => {
                e.me.origin = source_origin(p);
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
                e.kind = EntityKind::Particle {
                    active: false,
                    started: 0.,
                }
            }
        }
    }
}
fn free_fx(world: &EntityWorld, kind: &str, count: usize, used: &[usize]) -> Option<usize> {
    (0..count)
        .flat_map(|slot| world.find(&fx_name(kind, slot)))
        .find(|id| !used.contains(id))
}
pub fn install(app: &mut App) {
    app.init_resource::<Spit>()
        .add_systems(FixedUpdate, tick.after(super::tick))
        .add_systems(Update, draw.after(crate::game::controls))
        .add_systems(PostUpdate, crate::game_audio::fade_out);
    if std::env::args().any(|a| a == "--capture-spitter-audio") {
        app.add_systems(FixedUpdate, audio_fixture.before(tick));
    }
}
/// Opt-in playback regression through the real acid/contact/audio systems.
fn audio_fixture(
    state: Res<Match>, collision: Res<Collision>, mut spit: ResMut<Spit>,
    mut body: Single<&mut Body>, playing: Query<&crate::game_audio::MapSound>,
    mut phase: Local<u8>, mut origin: Local<Vec3>,
) {
    if state.paused { return; }
    match *phase {
        0 if state.elapsed >= 0.5 => {
            *origin = body.feet;
            spit.detonate(&collision, *origin + Vec3::Y * u(1.), DEATH_FLAMES);
            println!("Acid audio fixture: enter"); *phase = 1;
        }
        1 if state.elapsed >= 1.5 => {
            body.feet = *origin + Vec3::X * 4.; body.velocity = Vec3::ZERO;
            println!("Acid audio fixture: leave"); *phase = 2;
        }
        2 if state.elapsed >= 2. => {
            body.feet = *origin; body.velocity = Vec3::ZERO;
            println!("Acid audio fixture: re-enter during previous fade"); *phase = 3;
        }
        3 if state.elapsed >= 3. => {
            body.feet = *origin + Vec3::X * 4.; body.velocity = Vec3::ZERO;
            println!("Acid audio fixture: leave again"); *phase = 4;
        }
        4 if state.elapsed >= 8.5 => {
            let voices = playing.iter().filter(|s|s.0 >= ACID_SOUND_BASE).count();
            println!("Acid audio fixture: complete pools={} contact={} owned_voices={voices}",
                spit.pools.len(), spit.goo_player);
            *phase = 5;
        }
        _ => {}
    }
}
fn capture() -> bool {
    static CAPTURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CAPTURE.get_or_init(|| std::env::args().any(|a| a == "--capture-spitter"))
}
#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut spit: ResMut<Spit>,
    mut input: ResMut<crate::game::Input>,
    body: Single<&Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut actors: Query<(&CompanionActor, &mut Transform)>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    animations: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    rooms: (
        Option<Res<crate::l4d2r_navigation::Navigation>>,
        Option<Res<crate::source_director::Director>>,
    ),
) {
    let spit = &mut *spit;
    let stop_sound = |commands: &mut Commands, id: usize| {
        for (entity, sound) in &playing {
            if sound.0 == id {
                commands.entity(entity).despawn();
            }
        }
    };
    let reset = state.infected != spit.infected_team || state.elapsed < spit.epoch;
    spit.epoch = state.elapsed;
    if reset {
        if !spit.pools.is_empty() || !spit.projectiles.is_empty() {
            if let Some(logic) = logic.as_mut() {
                for p in &spit.projectiles {
                    if let Some(id) = p.fx {
                        set_fx(&mut logic.world, id, None, false);
                    }
                }
                for pool in &spit.pools {
                    for f in &pool.flames {
                        if let Some(id) = f.fx {
                            set_fx(&mut logic.world, id, None, false);
                        }
                    }
                }
            }

        }
        for id in spit.cancel_audio(&mut sounds) { stop_sound(&mut commands, id); }
        *spit = Spit {
            next_id: spit.next_id,
            burn_serial: spit.burn_serial,
            epoch: state.elapsed,
            infected_team: state.infected,
            ..default()
        };
    }
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    let spitter = state.infected && state.special_class == SpecialClass::Spitter;
    // A survivor player is hit by AI Spitter acid like the bots.
    let player = (!state.infected && !state.dead)
        .then(|| (body.feet, if state.incap { DUCK_HEIGHT } else { HEIGHT }));
    // Death puddle (server 10336010): trace 240 units down from the
    // origin, insect_swarm limited to two flames.
    let dead = spitter && state.dead;
    if dead && !spit.was_dead {
        spit.latency = None;
        let from = body.feet + Vec3::Y * u(1.);
        let to = body.feet - Vec3::Y * u(240.);
        let hit = collision.trace(from, to, Vec3::ZERO);
        if hit.fraction < 1. {
            let at = from + (to - from) * hit.fraction;
            spit.detonate(&collision, at + Vec3::Y * u(1.), DEATH_FLAMES);
        }
    }
    spit.was_dead = dead;
    if capture() && spitter && hunter.ghost && !state.dead {
        hunter.ghost = false;
        state.special_phase = 0;
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let side = Quat::from_rotation_y(body.yaw) * Vec3::X;
        for (i, member) in squad.0.iter_mut().enumerate() {
            member.position = collision
                .supported_spawn_near(
                    body.feet + forward * 7. + side * (i as f32 - 1.5) * 1.2,
                    HEIGHT,
                )
                .unwrap_or(member.position);
            member.cooldown = 30.;
        }
    }
    let active = super::active(&state, &hunter, SpecialClass::Spitter);
    spit.cooldown = (spit.cooldown - dt).max(0.);
    if active {
        hunter.ability_progress = Some(spit.progress());
        let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
        let eye = body.feet + Vec3::Y * body.stance.eye();
        let auto = capture() && !spit.capture_fired && state.elapsed > 1.5;
        let fire = std::mem::take(&mut input.fire) || input.fire_held || auto;
        let claw = std::mem::take(&mut input.shove) || input.shove_held;
        input.reload = false;
        input.use_item = false;
        if fire && spit.claw.is_none() && spit.try_spit() {
            spit.capture_fired |= auto;
            state.special_phase = SPIT_PHASE;
            state.shot_serial = state.shot_serial.wrapping_add(1);
            spit.phase_clock = animations
                .as_ref()
                .map_or(0.8, |a| a.duration(VIEW, &["ACT_VM_VOMIT"]))
                .max(SPIT_LATENCY);
            // Voice through the timer (FUN_103666f0 -> FUN_10347590).
            super::voice::speak(&mut hunter, &mut sounds, "SpitterZombie.Spit", 0.);
        } else if claw && hunter.cooldown <= 0. && spit.latency.is_none() {
            spit.claw = Some(CLAW_SWING_DURATION);
            hunter.cooldown = CLAW_MISS_INTERVAL;
            spit.phase_clock = CLAW_MISS_INTERVAL;
            state.special_phase = 4;
            state.shot_serial = state.shot_serial.wrapping_add(1);
            sounds.play("Claw.Swing", None);
        }
        if let Some(left) = spit.latency.as_mut() {
            *left -= dt;
            if *left <= 0. {
                spit.latency = None;
                spit.launch(&collision, eye, forward);
            }
        }
        if let Some(left) = spit.claw.as_mut() {
            *left -= dt;
            if *left <= 0. {
                spit.claw = None;
                if let Some(i) = claw_victim(&squad, &collision, eye, forward, body.pitch) {
                    squad.0[i].state.damage(CLAW_DAMAGE);
                    hunter.cooldown = CLAW_HIT_INTERVAL - CLAW_SWING_DURATION;
                    sounds.play("Claw.HitFlesh", Some(squad.0[i].position));
                } else if logic
                    .as_mut()
                    .is_some_and(|l| l.pound_obstacle(body.feet, forward, &collision))
                {
                    sounds.play("Claw.Hit", None);
                }
            }
        }
        spit.phase_clock -= dt;
        if matches!(state.special_phase, 4 | SPIT_PHASE)
            && spit.phase_clock <= 0.
            && spit.latency.is_none()
        {
            state.special_phase = 0;
        }
        // Idle SpitterZombie.Voice and Recognize: see `voice`.
    } else if spitter {
        spit.latency = None;
        spit.claw = None;
        if !state.dead && !hunter.ghost {
            hunter.ability_progress = Some(spit.progress());
        }
    }
    // Projectiles keep flying after the Spitter dies, like the retail entity.
    let survivors: Vec<_> = squad
        .0
        .iter()
        .filter(|m| !m.state.dead)
        .map(|m| (m.position, member_height(m)))
        .chain(player)
        .collect();
    let mut index = 0;
    while index < spit.projectiles.len() {
        if spit.projectiles[index].fx.is_none() {
            if let Some(logic) = logic.as_mut() {
                let used: Vec<_> = spit.projectiles.iter().filter_map(|p| p.fx).collect();
                spit.projectiles[index].fx =
                    free_fx(&logic.world, "projectile", PROJECTILE_FX, &used);
            }
        }
        let flight = spit.projectiles[index].step(&collision, &survivors, dt);
        let position = spit.projectiles[index].position;
        if let (Some(logic), Some(id)) = (logic.as_mut(), spit.projectiles[index].fx) {
            set_fx(&mut logic.world, id, Some(position), false);
        }
        match flight {
            Flight::Flying => index += 1,
            Flight::Bounced => {
                sounds.play("SpitProjectile.Bounce", Some(position));
                index += 1;
            }
            Flight::Detonate(at) => {
                let shot = spit.projectiles.remove(index);
                if let (Some(logic), Some(id)) = (logic.as_mut(), shot.fx) {
                    set_fx(&mut logic.world, id, None, false);
                }
                // A copy of the host's spit: the host's game makes the pool.
                if shot.net {
                    continue;
                }
                // Retail 103e2a30: spit landing in a CHECKPOINT area makes
                // no pool before survivors have left the start area and a
                // 2-flame pool after; Survival keeps the full pool.
                let in_checkpoint = rooms
                    .0
                    .as_ref()
                    .and_then(|n| n.checkpoint_near(at))
                    .unwrap_or(false);
                let left = rooms.1.as_ref().is_none_or(|d| d.left_safe_area());
                match checkpoint_flames(in_checkpoint, left, crate::survival::active()) {
                    Some(flames) => {
                        spit.detonate(&collision, at, flames);
                    }
                    None => {}
                }
            }
        }
    }
    // Online guest: acid damage comes from the host's game ("dmg").
    let guest = crate::net_play::guest();
    for (i, damage) in spit.advance_pools(&collision, dt) {
        if guest {
            continue;
        }
        damage_survivors(&spit.pools[i], damage, &mut squad, Some(&collision));
        if let Some((feet, height)) = player {
            if damage > 0. && spit.pools[i].touching(feet, height, Some(&collision)) {
                state.damage(damage);
            }
        }
    }
    // Visual emitters and the SpitterZombie.Acid loop.
    if let Some(logic) = logic.as_mut() {
        let mut used: Vec<usize> = spit
            .pools
            .iter()
            .flat_map(|p| p.flames.iter().filter_map(|f| f.fx))
            .collect();
        for pool in &mut spit.pools {
            if !pool.alive() {
                continue;
            }
            for flame in &mut pool.flames {
                if flame.fx.is_none() {
                    if let Some(id) = free_fx(&logic.world, "pool", FLAME_FX, &used) {
                        used.push(id);
                        flame.fx = Some(id);
                        set_fx(&mut logic.world, id, Some(flame.position), true);
                    }
                }
            }
        }
    }
    // The SpitterZombie.Acid sizzle loops for every pool under its own stop
    // id (it used to borrow the first flame's particle slot and stayed silent
    // whenever no slot was free).
    for pool in &mut spit.pools {
        if pool.alive() && pool.sound.is_none() {
            let id = ACID_SOUND_BASE.wrapping_add(pool.id as usize);
            pool.sound = Some(id);
            sounds.0.push_back(crate::game_audio::Request {
                name: "SpitterZombie.Acid".into(),
                position: Some(pool.origin),
                looping: true,
                gain: 1.,
                map_entity: Some(id), soundlevel: None,
            });
        }
    }
    // Retail OnEnteredSpit: stepping into acid plays the Event.SpitterBurn
    // sting for that player and the survivor's GooedBySpitter line.
    let in_acid = |feet: Vec3, height: f32| {
        spit.pools
            .iter()
            .any(|p| p.alive() && p.touching(feet, height, Some(&collision)))
    };
    let player_in = player.is_some_and(|(feet, height)| in_acid(feet, height));
    let mut gooed = vec![];
    if player_in && !spit.goo_player {
        if let Some(session) = session.as_ref() {
            gooed.push(session.survivor.character().to_string());
        }
    }
    let mut bots = vec![false; squad.0.len()];
    for (i, member) in squad.0.iter().enumerate() {
        bots[i] = !member.state.dead && in_acid(member.position, member_height(member));
        if bots[i] && !spit.goo_bots.get(i).copied().unwrap_or(false) {
            gooed.push(
                member
                    .model
                    .trim_start_matches("models/survivors/survivor_")
                    .trim_end_matches(".mdl")
                    .to_string(),
            );
        }
    }
    for (id, fade) in spit.burn_audio(player_in, &mut sounds) {
        for (entity, sound) in &playing {
            if sound.0 == id {
                if fade { commands.entity(entity).insert(crate::game_audio::FadeOut::new(1.)); }
                else { commands.entity(entity).despawn(); }
            }
        }
    }
    spit.goo_bots = bots;
    for who in gooed {
        crate::source_dialogue::request(&who, "GooedBySpitter");
    }
    let mut ended = vec![];
    spit.pools.retain(|p| {
        if p.alive() {
            true
        } else {
            ended.push(p.clone());
            false
        }
    });
    for pool in ended {
        sounds.play("SpitterZombie.Acid.fadeout", Some(pool.origin));
        if let Some(id) = pool.sound.filter(|id| *id != usize::MAX) {
            sounds.1.push(id);
            stop_sound(&mut commands, id);
        }
        if let Some(logic) = logic.as_mut() {
            for f in &pool.flames {
                if let Some(id) = f.fx {
                    set_fx(&mut logic.world, id, None, false);
                }
            }
        }
    }
    let moved = escape_acid(&spit.pools, &mut squad, &collision, dt);
    if !moved.is_empty() {
        for (actor, mut transform) in &mut actors {
            if moved.contains(&actor.0) {
                transform.translation = squad.0[actor.0].position;
            }
        }
    }
}
#[derive(Component)]
struct AcidDraw(u64, usize);
/// Translucent ground area per flame, so the acid footprint stays readable
/// where the native PCF operator subset is incomplete.
fn draw(
    mut commands: Commands,
    spit: Res<Spit>,
    state: Res<Match>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    draws: Query<(Entity, &AcidDraw)>,
    mut mesh: Local<Option<Handle<Mesh>>>,
    mut pool_materials: Local<Vec<(u64, Handle<StandardMaterial>)>>,
) {
    // The old discs were a debug footprint, not the retail acid material.
    // Native PCF layers now own the normal presentation.
    if !std::env::args().any(|arg| arg == "--debug-spitter-footprint") {
        return;
    }
    let wanted = |id: u64, flame: usize| {
        state.infected
            && spit
                .pools
                .iter()
                .any(|p| p.id == id && flame < p.flames.len() && p.alive())
    };
    for (entity, d) in &draws {
        if !wanted(d.0, d.1) {
            commands.entity(entity).despawn();
        }
    }
    pool_materials.retain(|(id, _)| spit.pools.iter().any(|p| p.id == *id));
    if !state.infected {
        return;
    }
    let mesh = mesh
        .get_or_insert_with(|| meshes.add(Circle::new(u(FLAME_WIDTH * 1.4))))
        .clone();
    for pool in &spit.pools {
        let handle = if let Some((_, h)) = pool_materials.iter().find(|(id, _)| *id == pool.id) {
            h.clone()
        } else {
            let h = materials.add(StandardMaterial {
                base_color: Color::srgba(0.42, 0.85, 0.05, 0.),
                emissive: LinearRgba::rgb(0.08, 0.25, 0.0),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                depth_bias: 50.,
                ..default()
            });
            pool_materials.push((pool.id, h.clone()));
            h
        };
        if let Some(mut m) = materials.get_mut(&handle) {
            let fade = (pool.age / 0.3).min(1.) * ((LIFETIME - pool.age) / 1.).clamp(0., 1.);
            m.base_color.set_alpha(0.3 * fade);
        }
        for (i, flame) in pool.flames.iter().enumerate() {
            if draws.iter().any(|(_, d)| d.0 == pool.id && d.1 == i) {
                continue;
            }
            let rotation = Quat::from_rotation_arc(Vec3::Z, flame.normal.normalize_or(Vec3::Y));
            commands.spawn((
                AcidDraw(pool.id, i),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(handle.clone()),
                Transform::from_translation(flame.position + flame.normal * u(0.6))
                    .with_rotation(rotation),
                RenderLayers::layer(1),
                bevy::light::NotShadowCaster,
            ));
        }
    }
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
    fn retail_damage_curve_interpolates_and_fades_in() {
        assert_eq!(dps(0.), 0.);
        assert!((dps(1.5) - 7.5).abs() < 1e-4);
        assert!((dps(4.5) - 30.).abs() < 1e-4);
        assert!((dps(6.5) - 13.5).abs() < 1e-4);
        assert_eq!(dps(7.), 7.);
        assert_eq!(dps(8.), 0.);
        assert_eq!(dps(-1.), 0.);
        // 2s fade-in: one second in is half of 5 dps for 0.2s, truncated.
        assert_eq!(tick_damage(1.), 0.);
        assert_eq!(tick_damage(4.), 6.);
        // Retail DropSpit trace on c1m1_hotel (spitter-acid-coop.log): health
        // losses per 0.2s think from the first damaging think at ~1.6s.
        let retail = [
            1., 1., 2., 2., 2., 3., 3., 4., 4., 4., 5., 5., 6., 6., 6., 6., 6., 6., 5., 5., 4., 4.,
            4., 3., 2.,
        ];
        let ours: Vec<f32> = (0..35)
            .map(|i| tick_damage(i as f32 * DAMAGE_INTERVAL))
            .collect();
        assert!(ours[..8].iter().all(|d| *d == 0.));
        assert_eq!(&ours[8..8 + retail.len()], &retail);
        assert_eq!(ours.iter().sum::<f32>(), 102.);
    }
    #[test]
    fn spit_flies_straight_for_one_second_then_drops_at_retail_speed() {
        let free = Collision::default();
        let mut p = Projectile::new(Vec3::Y * 50., Vec3::NEG_Z * u(SPIT_VELOCITY));
        for _ in 0..59 {
            assert_eq!(p.step(&free, &[], 1. / 60.), Flight::Flying);
        }
        assert!((p.position.y - 50.).abs() < 1e-4);
        assert!((-p.position.z / u(SPIT_VELOCITY) - 59. / 60.).abs() < 1e-3);
        p.step(&free, &[], 2. / 60.);
        assert!(p.gravity);
        assert!((p.velocity.z / u(SPIT_VELOCITY) + LATE_HORIZONTAL).abs() < 1e-4);
        assert!(p.velocity.y < 0.);
    }
    #[test]
    fn spit_detonates_on_ground_and_on_survivor_contact() {
        let floor = Collision::test_floor();
        let mut p = Projectile::new(Vec3::Y * 1.5, Vec3::new(0., -1., -1.).normalize() * u(900.));
        let mut result = Flight::Flying;
        for _ in 0..120 {
            result = p.step(&floor, &[], 1. / 60.);
            if result != Flight::Flying {
                break;
            }
        }
        let Flight::Detonate(at) = result else {
            panic!("{result:?}");
        };
        assert!(at.y.abs() < 0.1);
        let mut p = Projectile::new(Vec3::Y, Vec3::NEG_Z * u(900.));
        let survivor = [(Vec3::new(0., 0., -5.), HEIGHT)];
        let mut hit = None;
        for _ in 0..60 {
            if let Flight::Detonate(at) = p.step(&floor, &survivor, 1. / 60.) {
                hit = Some(at);
                break;
            }
        }
        let at = hit.expect("survivor contact");
        assert!((at.z + 5. - RADIUS).abs() < 0.05, "{at}");
    }
    #[test]
    fn acid_music_has_one_owned_voice_and_exit_cancels_pending_loads() {
        let mut spit = Spit::default();
        let mut sounds = crate::game_audio::Queue::default();
        assert!(spit.burn_audio(false, &mut sounds).is_empty());
        assert!(spit.burn_audio(true, &mut sounds).is_empty());
        let first = spit.burn_sound.unwrap();
        assert_eq!(sounds.0.len(), 1);
        assert_eq!(sounds.0[0].name, "Event.SpitterBurn");
        assert!(!sounds.0[0].looping);
        assert_eq!(sounds.0[0].map_entity, Some(first));
        // Continuous contact, including a transition between overlapping pools,
        // must not keep restarting the 25-second music track.
        for _ in 0..120 { assert!(spit.burn_audio(true, &mut sounds).is_empty()); }
        assert_eq!(sounds.0.len(), 1);
        assert_eq!(spit.burn_audio(false, &mut sounds), [(first, true)]);
        assert!(sounds.1.contains(&first));
        assert!(spit.burn_audio(false, &mut sounds).is_empty());
        assert_eq!(spit.burn_audio(true, &mut sounds), [(first, false)]);
        assert_ne!(spit.burn_sound, Some(first));
        assert_eq!(sounds.0.len(), 2);
        assert_eq!(sounds.0[1].map_entity, spit.burn_sound);
    }
    #[test]
    fn spit_in_a_checkpoint_fizzles_until_survivors_leave_then_makes_a_small_pool() {
        assert_eq!(checkpoint_flames(false, false, false), Some(MAX_FLAMES));
        assert_eq!(checkpoint_flames(true, false, false), None);
        assert_eq!(checkpoint_flames(true, true, false), Some(2));
        // Survival/Scavenge: the full pool, even before leaving the start.
        assert_eq!(checkpoint_flames(true, false, true), Some(MAX_FLAMES));
        assert_eq!(checkpoint_flames(true, true, true), Some(MAX_FLAMES));
    }
    #[test]
    fn acid_reset_cancels_both_sizzle_and_music_even_before_they_load() {
        let mut spit = Spit::default();
        let mut sounds = crate::game_audio::Queue::default();
        let mut pool = Pool::new(1, Vec3::ZERO, Vec3::Y, MAX_FLAMES);
        let loop_id = ACID_SOUND_BASE + 1;
        pool.sound = Some(loop_id);
        spit.pools.push(pool);
        spit.burn_audio(true, &mut sounds);
        let music = spit.burn_sound.unwrap();
        let stopped = spit.cancel_audio(&mut sounds);
        assert_eq!(stopped, [loop_id, music]);
        assert!(sounds.1.contains(&loop_id) && sounds.1.contains(&music));
    }
    #[test]
    fn acid_pool_spreads_to_the_flame_limit_with_spacing_and_range() {
        let floor = Collision::test_floor();
        let mut spit = Spit::default();
        let i = spit
            .detonate(&floor, Vec3::new(0., 0.3, 0.), MAX_FLAMES)
            .unwrap();
        assert!(spit.pools[i].origin.y.abs() < 0.01);
        let mut counts = vec![];
        for step in 1..=90 {
            spit.advance_pools(&floor, 1. / 30.);
            if step % 6 == 1 && step > 1 {
                counts.push(spit.pools[0].flames.len());
            }
        }
        // Retail open-ground growth: 1, 2, 4, 8, 10 at 0.2s spacing.
        assert_eq!(counts[0], 2, "{counts:?}");
        assert!(counts[1] >= 3 && counts[1] <= 4, "{counts:?}");
        assert!(counts[4] == MAX_FLAMES, "{counts:?}");
        let pool = &spit.pools[0];
        assert!(pool.flames.len() == MAX_FLAMES, "{}", pool.flames.len());
        for (a, f) in pool.flames.iter().enumerate() {
            assert!(f.position.distance(pool.origin) <= u(MAX_RANGE) + 1e-3);
            assert!(f.depth < MAX_DEPTH);
            for g in &pool.flames[a + 1..] {
                assert!(g.center().distance(f.position) >= u(FLAME_WIDTH * 2.) - 1e-4);
            }
        }
        for _ in 0..200 {
            spit.advance_pools(&floor, 1. / 30.);
        }
        assert!(!spit.pools[0].alive());
    }
    #[test]
    fn death_puddle_is_limited_to_two_flames() {
        let floor = Collision::test_floor();
        let mut spit = Spit::default();
        spit.detonate(&floor, Vec3::Y * 0.1, DEATH_FLAMES).unwrap();
        for _ in 0..90 {
            spit.advance_pools(&floor, 1. / 30.);
        }
        assert_eq!(spit.pools[0].flames.len(), DEATH_FLAMES);
    }
    #[test]
    fn survivors_in_acid_take_ramped_damage_and_others_do_not() {
        let floor = Collision::test_floor();
        let mut spit = Spit::default();
        spit.detonate(&floor, Vec3::Y * 0.1, MAX_FLAMES).unwrap();
        let mut squad = Squad(vec![member(Vec3::ZERO), member(Vec3::X * 15.)]);
        let mut raised = member(Vec3::Y * u(30.));
        raised.state.health = 100.;
        squad.0.push(raised);
        let start = squad.0[0].state.health;
        let dt = 1. / 30.;
        for _ in 0..(LIFETIME / dt) as usize + 5 {
            for (i, d) in spit.advance_pools(&floor, dt) {
                damage_survivors(&spit.pools[i], d, &mut squad, Some(&floor));
            }
        }
        let taken = start - squad.0[0].state.health - squad.0[0].state.temporary;
        assert!(taken > 90., "{taken}");
        assert_eq!(squad.0[1].state.health, start);
        // Standing 30 units above the flame base exceeds the 20-unit limit.
        assert_eq!(squad.0[2].state.health, 100.);
    }
    #[test]
    fn survivor_bots_walk_out_of_acid() {
        let floor = Collision::test_floor();
        let mut spit = Spit::default();
        spit.detonate(&floor, Vec3::Y * 0.1, MAX_FLAMES).unwrap();
        let mut squad = Squad(vec![member(Vec3::new(0.3, 0., 0.1))]);
        let dt = 1. / 30.;
        let mut clear = None;
        for step in 0..150 {
            spit.advance_pools(&floor, dt);
            escape_acid(&spit.pools, &mut squad, &floor, dt);
            if step > 30 && !spit.pools[0].touching(squad.0[0].position, HEIGHT, None) {
                clear = Some(step);
                break;
            }
        }
        assert!(clear.is_some(), "{}", squad.0[0].position);
        assert!(squad.0[0].position.xz().length() > 1.);
    }
    #[test]
    fn spit_cooldown_latency_and_hud_meter_follow_retail_cvars() {
        let free = Collision::default();
        let mut spit = Spit::default();
        assert_eq!(spit.progress(), 1.);
        assert!(spit.try_spit());
        assert_eq!(spit.latency, Some(SPIT_LATENCY));
        assert_eq!(spit.progress(), 0.);
        assert!(!spit.try_spit());
        spit.latency = None;
        spit.launch(&free, Vec3::Y, Vec3::NEG_Z);
        let p = &spit.projectiles[0];
        assert!((p.velocity.length() / UNIT - SPIT_VELOCITY).abs() < 0.01);
        assert!((p.position.z + u(MOUTH_OFFSET)).abs() < 1e-4);
        assert_eq!(spit.cooldown, SPIT_INTERVAL);
        assert!(!spit.try_spit());
        spit.cooldown = 5.;
        assert!((spit.progress() - 0.75).abs() < 1e-5);
        assert_eq!(vm_activity(SPIT_PHASE), Some("ACT_VM_VOMIT"));
        assert_eq!(vm_activity(4), None);
    }
    #[test]
    fn claw_reaches_retail_range_only_in_front() {
        let free = Collision::default();
        let squad = Squad(vec![member(Vec3::new(0., 0., -u(CLAW_RANGE + 10.)))]);
        let eye = Vec3::Y * u(62.);
        assert_eq!(claw_victim(&squad, &free, eye, Vec3::NEG_Z, 0.), Some(0));
        assert_eq!(claw_victim(&squad, &free, eye, Vec3::Z, 0.), None);
        let far = Squad(vec![member(Vec3::new(0., 0., -u(CLAW_RANGE + 30.)))]);
        assert_eq!(claw_victim(&far, &free, eye, Vec3::NEG_Z, 0.), None);
    }
    #[test]
    fn owned_spitter_assets_have_activities_sounds_and_effects() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            MODEL,
            VIEW,
            "scripts/weapon_spitter_claw.txt",
            "materials/vgui/hud/pz_charge_spitter.vtf",
        ] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "spitterzombie.alert",
            "spitterzombie.death",
            "spitterzombie.pain",
            "spitterzombie.voice",
            "spitterzombie.spit",
            "spitterzombie.acid",
            "spitterzombie.acid.fadeout",
            "spitprojectile.bounce",
            "claw.hitflesh",
            "claw.swing",
            "claw.hit",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
        }
        let pcf =
            crate::source_pcf::parse(&assets.read("particles/spitter_fx.pcf").unwrap()).unwrap();
        for effect in [PROJECTILE_EFFECT, POOL_EFFECT] {
            assert!(
                pcf.iter()
                    .any(|e| e.kind == "DmeParticleSystemDefinition" && e.name == effect),
                "{effect}"
            );
        }
        let assets = std::cell::RefCell::new(assets);
        let sk =
            l4d2_formats::anim::Skeleton::load_with(VIEW, |p| assets.borrow_mut().read(p).ok())
                .unwrap();
        for activity in [
            "ACT_VM_DEPLOY",
            "ACT_VM_IDLE",
            "ACT_VM_VOMIT",
            "ACT_VM_PRIMARYATTACK",
        ] {
            assert!(
                sk.sequences().iter().any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
    }
}
