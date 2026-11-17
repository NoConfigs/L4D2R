//! Playable Jockey: leap (CLeap ability), ride (the jockey steers the
//! survivor while the victim resists), ride damage, dismount, claw melee and
//! the retail voice loop.
//!
//! Numbers come from the user's running retail game (cvars, build 10097) and
//! the owned server.dll decompilation; see local-research/si-agents/jockey/
//! NOTES.md for every source. The shared lifecycle (team switch, ghost spawn,
//! death/respawn, survivor-bot opposition and shove rescue) lives in
//! special_infected.rs.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, HEIGHT, RADIUS, UNIT},
    game::{Body, CompanionActor},
    game_state::{Match, Squad},
};
use bevy::prelude::*;
pub const WEAPON: &str = "weapon_jockey_claw";
pub const MODEL: &str = "models/infected/jockey.mdl";
pub const VIEW: &str = "models/weapons/arms/v_jockey_arms.mdl";
/// Retail z_jockey_health = 325.
pub const HEALTH: f32 = 325.;
/// Retail z_jockey_speed = 250 (CTerrorPlayer max speed 102c9940, class 5;
/// weapon_jockey_claw MaxPlayerSpeed is also 250).
pub const SPEED: f32 = 250.;
pub const PLAYABLE: bool = true;
/// Retail keeps the riding jockey in first person (captured ride view
/// local-research/parity/jockey-retail-ride-view.jpg), so no orbit camera.
pub const THIRD_PERSON_PIN: bool = false;
/// v_jockey_arms.mdl only has idle, deploy, lunge and attack activities.
pub const VM_ACTIONS: &[(&str, &str)] =
    &[("leap", "ACT_VM_LUNGE"), ("claw", "ACT_VM_PRIMARYATTACK")];
pub const CONTROLS: &str =
    "Jockey playtest: left click leaps; on contact WASD steers the survivor; right click claws.";
/// Phase 2 keeps the shared ACT_VM_LUNGE and phase 4 ACT_VM_PRIMARYATTACK.
/// The arms have no pummel activity, so a ride idles.
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    (phase == 3).then_some("ACT_VM_IDLE")
}

// Retail cvars (netconsole `find leap`, `find jockey`, `find claw`).
/// z_leap_power: CLeap launch speed along the aim (server 10364440).
pub const LEAP_POWER: f32 = 400.;
/// Measured retail player leap (jockey-leap-natural.log, level aim): 400
/// forward, 150 initial vertical (123.333 after one 30Hz gravity tick) and
/// an 18-unit take-off lift.
pub const LEAP_UP: f32 = 150.;
pub const LEAP_LIFT: f32 = 18.;
/// z_leap_interval: recharge after a leap lands without a victim (10364920).
pub const LEAP_INTERVAL: f32 = 0.5;
/// z_leap_interval_post_ride: recharge after a dismount (10313220).
pub const POST_RIDE_INTERVAL: f32 = 6.;
/// z_leap_interval_post_incap: recharge when the victim is incapacitated
/// (survivor OnIncapacitated 1032eb40).
pub const POST_INCAP_INTERVAL: f32 = 30.;
/// z_jockey_ride_damage / _delay / _interval (jockey ride think 103485b0).
pub const RIDE_DAMAGE: f32 = 4.;
pub const RIDE_DAMAGE_DELAY: f32 = 1.;
pub const RIDE_DAMAGE_INTERVAL: f32 = 1.;
/// z_jockey_control_min / _max / _variance (victim move blend 102ca4a0).
pub const CONTROL_MIN: f32 = 0.8;
pub const CONTROL_MAX: f32 = 0.8;
pub const CONTROL_VARIANCE: f32 = 0.7;
/// z_jockey_speed_blend: the control oscillator advances by
/// RandomFloat(blend, 2 * blend) radians per second.
pub const SPEED_BLEND: f32 = 2.;
/// z_jockey_stagger_amount / _speed (stagger is disabled at retail 0).
pub const STAGGER_AMOUNT: f32 = 0.;
pub const STAGGER_SPEED: f32 = 2.;
/// z_jockey_min_mounted_speed: ridden survivor top speed floor as a fraction
/// of the 220 run speed (102c9940 scales by health / max health).
pub const MIN_MOUNTED_SPEED: f32 = 0.6;
/// Constants in server.dll: victim input scale 0.004 (0x1068b20c) and
/// blended move scale 250 (0x1060dc0c); mounted top speed base 220
/// (0x105e0490); 0.4s attach blend (0x105fc3e4).
pub const MOVE_SCALE: f32 = 250.;
pub const MOUNTED_RUN_SPEED: f32 = 220.;
pub const ATTACH_BLEND: f32 = 0.4;
/// Rewrite approximation: the first-person ride camera sits this far above
/// the survivor origin + 62-unit eye so it clears the ridden survivor's head
/// (retail parents the jockey at the victim origin; its exact ride eye
/// position is not measured).
pub const RIDE_VIEW_RAISE: f32 = 20.;
/// versus_shove_jockey_fov_leaping: a survivor shoving within this angle of
/// the leaping jockey knocks it away instead of being ridden (103347c0).
pub const SHOVE_FOV: f32 = 45.;
/// jockey_pz_claw_dmg and the shared claw cvars.
pub const CLAW_DAMAGE: f32 = 4.;
pub const CLAW_RANGE: f32 = 52.;
pub const CLAW_RANGE_DOWN: f32 = 70.;
pub const CLAW_SWING_DURATION: f32 = 0.2;
pub const CLAW_HIT_INTERVAL: f32 = 1.;
pub const CLAW_MISS_INTERVAL: f32 = 0.5;
/// Map-sound id for the looping JockeyZombie.Ride (the attackloop waves
/// carry smpl loop chunks). Far above any map entity index.
const RIDE_SOUND: usize = usize::MAX - 0x10c4e7;

fn u(v: f32) -> f32 {
    v * UNIT
}
/// CLeap launch velocity (rewrite metres/sec): the normalised aim times
/// z_leap_power plus the measured 150-unit upward impulse.
pub fn leap_velocity(aim: Vec3) -> Vec3 {
    aim.normalize_or_zero() * u(LEAP_POWER) + Vec3::Y * u(LEAP_UP)
}
/// Share of the victim's movement that follows the jockey (102ca4a0):
/// lerp(min, max, 1 - health fraction) + variance * sin(phase) * 0.5,
/// clamped to 0..1.
pub fn control(health_fraction: f32, phase: f32) -> f32 {
    ((CONTROL_MAX - CONTROL_MIN) * (1. - health_fraction.clamp(0., 1.))
        + CONTROL_MIN
        + CONTROL_VARIANCE * phase.sin() * 0.5)
        .clamp(0., 1.)
}
/// Blended victim move in Source units/sec (horizontal). Each input is
/// normalised when longer than 0.001, so a passive victim contributes zero.
pub fn ride_move(jockey: Vec3, victim: Vec3, control: f32) -> Vec3 {
    let unit = |v: Vec3| {
        let v = Vec3::new(v.x, 0., v.z);
        if v.length() > 0.001 {
            v.normalize()
        } else {
            Vec3::ZERO
        }
    };
    (unit(victim) * (1. - control) + unit(jockey) * control) * MOVE_SCALE
}
/// Ridden survivor top speed in Source units/sec (102c9940).
pub fn mounted_top_speed(health_fraction: f32) -> f32 {
    health_fraction.max(MIN_MOUNTED_SPEED) * MOUNTED_RUN_SPEED
}
/// Wish direction and speed (Source units/sec) of the ridden survivor.
pub fn ride_wish(jockey: Vec3, victim: Vec3, control: f32, health_fraction: f32) -> (Vec3, f32) {
    let wish = ride_move(jockey, victim, control);
    (
        wish.normalize_or_zero(),
        wish.length().min(mounted_top_speed(health_fraction)),
    )
}
/// True when a shoving survivor faces the incoming jockey within the
/// versus_shove_jockey_fov_leaping cone.
pub fn shove_deflects(shoving: bool, victim_aim: Vec3, victim: Vec3, jockey: Vec3) -> bool {
    let to = (jockey - victim) * Vec3::new(1., 0., 1.);
    let aim = victim_aim * Vec3::new(1., 0., 1.);
    shoving
        && aim.length_squared() > 1e-6
        && aim.normalize().dot(to.normalize_or_zero()) >= SHOVE_FOV.to_radians().cos()
}
/// Why a ride ended; selects the next leap interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RideEnd {
    /// Rescue shove, jockey death, victim death or dismount.
    Dismount,
    /// The victim became incapacitated.
    Incap,
}
#[derive(Resource)]
pub struct Jockey {
    pub leaping: bool,
    airtime: f32,
    previous: Vec3,
    /// Remaining and total time of the CLeap activation timer.
    pub timer: f32,
    pub duration: f32,
    pub victim: Option<usize>,
    attach_from: Vec3,
    attach: f32,
    damage_clock: f32,
    control_phase: f32,
    stagger: [f32; 2],
    velocity: Vec3,
    pub ride_time: f32,
    claw: Option<f32>,
    phase_clock: f32,
    sound: bool,
    rng: u32,
    capture_fired: bool,
}
impl Default for Jockey {
    fn default() -> Self {
        Self {
            leaping: false,
            airtime: 0.,
            previous: Vec3::ZERO,
            timer: 0.,
            duration: LEAP_INTERVAL,
            victim: None,
            attach_from: Vec3::ZERO,
            attach: 0.,
            damage_clock: 0.,
            control_phase: 0.,
            stagger: [0.; 2],
            velocity: Vec3::ZERO,
            ride_time: 0.,
            claw: None,
            phase_clock: 0.,
            sound: false,
            rng: 0x6a0c_4e75,
            capture_fired: false,
        }
    }
}
impl Jockey {
    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        lo + (hi - lo) * (self.rng as f32 / u32::MAX as f32)
    }
    /// HUD meter: 1 when a leap is available, recharging after a landing or
    /// dismount, empty while leaping or riding.
    pub fn progress(&self) -> f32 {
        if self.leaping || self.victim.is_some() {
            0.
        } else if self.duration > 0. {
            (1. - self.timer / self.duration).clamp(0., 1.)
        } else {
            1.
        }
    }
    pub fn can_leap(&self, grounded: bool) -> bool {
        grounded
            && !self.leaping
            && self.victim.is_none()
            && self.timer <= 0.
            && self.claw.is_none()
    }
    /// Launches a leap; returns the new body velocity.
    pub fn start_leap(&mut self, feet: Vec3, aim: Vec3) -> Vec3 {
        self.leaping = true;
        self.airtime = 0.;
        self.previous = feet;
        leap_velocity(aim)
    }
    /// A leap that touched the ground without a victim (10364920).
    pub fn land(&mut self) {
        self.leaping = false;
        self.timer = LEAP_INTERVAL;
        self.duration = LEAP_INTERVAL;
    }
    pub fn mount(&mut self, victim: usize, from: Vec3) {
        self.leaping = false;
        self.victim = Some(victim);
        self.attach_from = from;
        self.attach = 0.;
        self.damage_clock = RIDE_DAMAGE_DELAY;
        self.control_phase = 0.;
        self.stagger = [0.; 2];
        self.velocity = Vec3::ZERO;
        self.ride_time = 0.;
    }
    pub fn dismount(&mut self, end: RideEnd) {
        self.victim = None;
        self.leaping = false;
        self.duration = match end {
            RideEnd::Dismount => POST_RIDE_INTERVAL,
            RideEnd::Incap => POST_INCAP_INTERVAL,
        };
        self.timer = self.duration;
    }
    /// Advances the ride oscillators and damage timer; returns the damage
    /// to apply this tick.
    pub fn ride_step(&mut self, dt: f32) -> f32 {
        self.ride_time += dt;
        self.attach += dt;
        for i in 0..2 {
            self.stagger[i] += self.random(STAGGER_SPEED, STAGGER_SPEED * 2.) * dt;
        }
        self.control_phase += self.random(SPEED_BLEND, SPEED_BLEND * 2.) * dt;
        self.damage_clock -= dt;
        if self.damage_clock <= 0. {
            self.damage_clock += RIDE_DAMAGE_INTERVAL;
            RIDE_DAMAGE
        } else {
            0.
        }
    }
    /// Jockey feet blend onto the victim origin over 0.4s, then follow it.
    pub fn attach_point(&self, victim: Vec3) -> Vec3 {
        self.attach_from
            .lerp(victim, (self.attach / ATTACH_BLEND).clamp(0., 1.))
    }
    pub fn control(&self, health_fraction: f32) -> f32 {
        control(health_fraction, self.control_phase)
    }
    /// Retail stagger term added to the blended move (zero at retail
    /// z_jockey_stagger_amount 0), in Source units/sec.
    fn stagger_move(&self) -> Vec3 {
        Vec3::new(self.stagger[0].cos(), 0., self.stagger[1].cos()) * STAGGER_AMOUNT
    }
}
fn member_height(m: &crate::game_state::Companion) -> f32 {
    if m.state.incap {
        crate::collision::DUCK_HEIGHT
    } else {
        HEIGHT
    }
}
/// Earliest survivor whose hull the leap swept through this tick, using the
/// Hunter's swept-hull contact plus a clear line between the bodies.
fn leap_victim(squad: &Squad, collision: &Collision, from: Vec3, to: Vec3) -> Option<usize> {
    let eye = to + Vec3::Y * (HEIGHT * 0.5);
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead && !m.state.incap && !m.state.pinned)
        .filter_map(|(i, m)| {
            super::swept_victim(from, to, m.position, member_height(m)).map(|f| (i, f))
        })
        .filter(|(i, _)| {
            collision
                .trace(
                    eye,
                    squad.0[*i].position + Vec3::Y * (HEIGHT * 0.5),
                    Vec3::ZERO,
                )
                .fraction
                >= 0.99
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
fn claw_victim(
    squad: &Squad,
    collision: &Collision,
    eye: Vec3,
    forward: Vec3,
    pitch: f32,
) -> Option<usize> {
    let range = u(CLAW_RANGE + (CLAW_RANGE_DOWN - CLAW_RANGE) * (-pitch.sin()).max(0.));
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .filter_map(|(i, m)| {
            let lo = m.position - Vec3::new(RADIUS, 0., RADIUS);
            let hi = m.position + Vec3::new(RADIUS, member_height(m), RADIUS);
            let center = m.position + Vec3::Y * member_height(m) * 0.6;
            let distance = eye.clamp(lo, hi).distance(eye);
            (distance < range
                && forward.dot((center - eye).normalize_or_zero()) > 0.6
                && collision.trace(eye, center, Vec3::ZERO).fraction >= 0.99)
                .then_some((i, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
/// The ridden bot's own input: it pulls toward the nearest standing
/// teammate (a rewrite approximation of a resisting survivor).
fn victim_input(squad: &Squad, victim: usize) -> Vec3 {
    // sb_stop: a stopped bot gives no input, so the jockey steers alone.
    if crate::game_state::bots_stopped() {
        return Vec3::ZERO;
    }
    let from = squad.0[victim].position;
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != victim && !m.state.dead && !m.state.incap && !m.state.pinned)
        .map(|(_, m)| m.position - from)
        .filter(|d| d.xz().length() > 1.5)
        .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
        .map_or(Vec3::ZERO, |d| {
            (d * Vec3::new(1., 0., 1.)).normalize_or_zero()
        })
}
fn actor_rotation(aim: Vec3) -> Quat {
    Quat::from_rotation_y((-aim.z).atan2(aim.x))
}
fn capture() -> bool {
    static CAPTURE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CAPTURE.get_or_init(|| std::env::args().any(|a| a == "--capture-jockey"))
}
pub fn install(app: &mut App) {
    app.init_resource::<Jockey>()
        .add_systems(FixedUpdate, tick.after(super::tick))
        .add_systems(Update, ride_model.after(crate::game::controls));
}
fn stop_ride_sound(
    jockey: &mut Jockey,
    commands: &mut Commands,
    sounds: &mut crate::game_audio::Queue,
    playing: &Query<(Entity, &crate::game_audio::MapSound)>,
) {
    if std::mem::take(&mut jockey.sound) {
        sounds.1.push(RIDE_SOUND);
        for (entity, sound) in playing {
            if sound.0 == RIDE_SOUND {
                commands.entity(entity).despawn();
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut jockey: ResMut<Jockey>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut actors: Query<(&CompanionActor, &mut Transform)>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    let j = &mut *jockey;
    if !state.infected || state.special_class != SpecialClass::Jockey {
        stop_ride_sound(j, &mut commands, &mut sounds, &playing);
        if j.victim.is_some() || j.leaping || j.timer > 0. {
            *j = Jockey::default();
        }
        return;
    }
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    let collision = collision.for_actor(true, hunter.ghost);
    // The shared tick releases the pin on a rescue shove, jockey death or
    // role change; treat that as a dismount.
    if let Some(i) = j.victim {
        if hunter.pin != Some(i) {
            j.dismount(RideEnd::Dismount);
            stop_ride_sound(j, &mut commands, &mut sounds, &playing);
            if let Some(m) = squad.0.get(i) {
                let back = m.position - actor_forward(m.aim) * (2.2 * RADIUS);
                if !state.dead && collision.clear_hull(back, HEIGHT) {
                    body.feet = back;
                }
            }
            body.velocity = Vec3::ZERO;
        }
    }
    if capture() && hunter.ghost && !state.dead {
        hunter.ghost = false;
        state.special_phase = 0;
    }
    if state.dead || hunter.ghost {
        j.leaping = false;
        j.claw = None;
        j.timer = 0.;
        j.capture_fired = false;
        return;
    }
    if capture() && !j.capture_fired && state.elapsed > 1. && j.victim.is_none() && !j.leaping {
        // Stage the bots in front and leap at the first one.
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let side = Quat::from_rotation_y(body.yaw) * Vec3::X;
        for (i, member) in squad.0.iter_mut().enumerate() {
            member.position = collision
                .supported_spawn_near(
                    body.feet + forward * (2.2 + i as f32 * 1.5) + side * (i as f32 * 1.3),
                    HEIGHT,
                )
                .unwrap_or(member.position);
            member.cooldown = 30.;
        }
        body.pitch = 0.;
    }
    j.timer = (j.timer - dt).max(0.);
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let eye = body.feet + Vec3::Y * body.stance.eye();
    let auto = capture() && !j.capture_fired && state.elapsed > 1.2;
    let fire = std::mem::take(&mut input.fire) || input.fire_held || auto;
    let claw = std::mem::take(&mut input.shove) || input.shove_held;
    input.reload = false;
    input.use_item = false;
    if fire && j.can_leap(body.grounded) {
        j.capture_fired |= auto;
        let lifted = body.feet + Vec3::Y * u(LEAP_LIFT);
        if collision.clear_hull(lifted, crate::collision::DUCK_HEIGHT) {
            body.feet = lifted;
        }
        body.velocity = j.start_leap(body.feet, forward);
        body.grounded = false;
        state.special_phase = 2;
        // Retail plays no leap sound: JockeyZombie.Pounce is never emitted by
        // server.dll and the jockey animations carry no sound events.
        state.shot_serial = state.shot_serial.wrapping_add(1);
    } else if claw && j.victim.is_none() && !j.leaping && j.claw.is_none() && hunter.cooldown <= 0.
    {
        j.claw = Some(CLAW_SWING_DURATION);
        hunter.cooldown = CLAW_MISS_INTERVAL;
        j.phase_clock = CLAW_MISS_INTERVAL;
        state.special_phase = 4;
        state.shot_serial = state.shot_serial.wrapping_add(1);
        sounds.play("Claw.Swing", None);
    }
    if let Some(left) = j.claw.as_mut() {
        *left -= dt;
        if *left <= 0. {
            j.claw = None;
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
    j.phase_clock -= dt;
    if state.special_phase == 4 && j.phase_clock <= 0. && j.claw.is_none() {
        state.special_phase = 0;
    }
    if j.leaping {
        j.airtime += dt;
        let victim = leap_victim(&squad, &collision, j.previous, body.feet);
        if let Some(i) = victim {
            let m = &squad.0[i];
            if shove_deflects(m.state.shove > 0., m.aim, m.position, body.feet) {
                // Knocked away by a facing shove: the leap ends here.
                body.velocity = (body.feet - m.position) * Vec3::new(1., 0., 1.);
                body.velocity = body.velocity.normalize_or_zero() * u(150.);
                j.land();
                state.special_phase = 0;
            } else {
                j.mount(i, body.feet);
                hunter.pin = Some(i);
                hunter.pin_yaw = body.yaw;
                hunter.rescue_point = None;
                super::pin_member(&mut squad, i, SpecialClass::Jockey);
                squad.0[i].vertical = 0.;
                state.special_phase = 3;
                body.velocity = Vec3::ZERO;
                sounds.play("Zombie.Punch", Some(squad.0[i].position));
                sounds.0.push_back(crate::game_audio::Request {
                    name: "JockeyZombie.Ride".into(),
                    position: None,
                    looping: true,
                    gain: 1.,
                    map_entity: Some(RIDE_SOUND), soundlevel: None,
                });
                j.sound = true;
            }
        } else if body.grounded && j.airtime > dt * 1.5 {
            j.land();
            state.special_phase = 0;
        }
    }
    if let Some(i) = j.victim {
        let incap = squad.0.get(i).is_none_or(|m| m.state.incap || m.state.dead);
        if incap {
            let end = if squad
                .0
                .get(i)
                .is_some_and(|m| m.state.incap && !m.state.dead)
            {
                RideEnd::Incap
            } else {
                RideEnd::Dismount
            };
            super::release(&mut hunter, &mut squad, &mut state);
            j.dismount(end);
            stop_ride_sound(j, &mut commands, &mut sounds, &playing);
        } else {
            let damage = j.ride_step(dt);
            let health = (squad.0[i].state.health / 100.).clamp(0., 1.);
            let mut steer = Vec3::ZERO;
            for (key, v) in [
                (KeyCode::KeyW, Vec3::NEG_Z),
                (KeyCode::KeyS, Vec3::Z),
                (KeyCode::KeyA, Vec3::NEG_X),
                (KeyCode::KeyD, Vec3::X),
            ] {
                if keys.pressed(key) {
                    steer += v;
                }
            }
            if capture() && j.ride_time < 3. {
                steer = Vec3::NEG_Z;
            }
            let steer = Quat::from_rotation_y(body.yaw) * steer;
            let resist = victim_input(&squad, i);
            let wish = ride_move(steer, resist, j.control(health)) + j.stagger_move();
            let speed = wish.length().min(mounted_top_speed(health));
            let member = &mut squad.0[i];
            let grounded = collision.grounded(member.position, HEIGHT);
            j.velocity = crate::player_movement::horizontal(
                j.velocity,
                wish.normalize_or_zero(),
                u(speed),
                grounded,
                dt,
            );
            member.vertical = if grounded {
                0.
            } else {
                member.vertical - u(800.) * dt
            };
            member.position = collision.walk(
                member.position,
                (j.velocity + Vec3::Y * member.vertical) * dt,
                HEIGHT,
                grounded,
            );
            if j.velocity.xz().length() > u(10.) {
                member.aim = (j.velocity * Vec3::new(1., 0., 1.)).normalize();
            }
            if damage > 0. {
                member.state.damage(damage);
                state.shot_serial = state.shot_serial.wrapping_add(1);
            }
            body.velocity = Vec3::ZERO;
            body.feet = j.attach_point(member.position + Vec3::Y * u(RIDE_VIEW_RAISE));
            hunter.pin_yaw = body.yaw;
            let (position, aim) = (member.position, member.aim);
            for (actor, mut transform) in &mut actors {
                if actor.0 == i {
                    transform.translation = position;
                    transform.rotation = actor_rotation(aim);
                }
            }
        }
    }
    // Ambient laugh while not riding (103485b0): see `voice`.
    j.previous = body.feet;
    hunter.ability_progress = Some(j.progress());
}
fn actor_forward(aim: Vec3) -> Vec3 {
    let flat = aim * Vec3::new(1., 0., 1.);
    if flat.length_squared() > 1e-6 {
        flat.normalize()
    } else {
        Vec3::X
    }
}
#[derive(Component)]
struct RideDraw(usize);
/// Temporary world Jockey drawn on the victim during a third-person ride,
/// posed with the native jockey_ride (ACT_TERROR_JOCKEY_RIDE) sequence. The
/// retail jockey origin is parented to the victim origin (103485b0), so the
/// authored ride pose places it on the survivor's shoulders.
#[allow(clippy::too_many_arguments)]
fn ride_model(
    mut commands: Commands,
    state: Res<Match>,
    jockey: Res<Jockey>,
    squad: Res<Squad>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    mut meshes: ResMut<Assets<Mesh>>,
    time: Res<Time>,
    mut draws: Query<(Entity, &RideDraw, &Mesh3d, &mut Transform)>,
    mut clock: Local<f32>,
) {
    let victim = jockey
        .victim
        .filter(|_| state.infected_third_person() && state.special_class == SpecialClass::Jockey)
        .and_then(|i| squad.0.get(i));
    let Some(victim) = victim else {
        for (entity, _, _, _) in &draws {
            commands.entity(entity).despawn();
        }
        *clock = 0.;
        return;
    };
    if !state.paused {
        *clock += time.delta_secs();
    }
    let blend = (jockey.attach / ATTACH_BLEND).clamp(0., 1.);
    let transform = Transform::from_translation(jockey.attach_from.lerp(victim.position, blend))
        .with_rotation(actor_rotation(victim.aim));
    if draws.is_empty() {
        if let Some(parts) = models.0.get(&(MODEL.into(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.scripted(MODEL, index, "jockey_ride", *clock, &mut owned);
                commands.spawn((
                    RideDraw(index),
                    Mesh3d(meshes.add(owned)),
                    MeshMaterial3d(material.clone()),
                    transform,
                    bevy::camera::visibility::RenderLayers::layer(1),
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
    }
    for (_, draw, mesh, mut location) in &mut draws {
        *location = transform;
        if !state.paused {
            if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
                library.scripted(MODEL, draw.0, "jockey_ride", *clock, &mut mesh);
            }
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
            aim: Vec3::NEG_X,
        }
    }
    #[test]
    fn leap_launches_along_the_aim_at_retail_power() {
        // Level aim: the measured retail 400 forward / 150 up launch.
        let v = leap_velocity(Vec3::new(3., 0., -4.)) / UNIT;
        assert!((v.xz().length() - 400.).abs() < 0.01);
        assert!((v.y - 150.).abs() < 0.01);
        assert!((v.xz().normalize() - Vec2::new(0.6, -0.8)).length() < 1e-5);
        // One 30Hz gravity tick leaves the recorded 123.333.
        assert!((v.y - 800. / 30. - 123.333).abs() < 0.01);
        let up = leap_velocity(Vec3::new(0., 1., -1.));
        assert!(up.y > 0.);
    }
    #[test]
    fn leap_requires_ground_and_recharge_and_lands_into_retail_interval() {
        let mut j = Jockey::default();
        assert!(j.can_leap(true));
        assert!(!j.can_leap(false));
        assert_eq!(j.progress(), 1.);
        j.start_leap(Vec3::ZERO, Vec3::NEG_Z);
        assert!(!j.can_leap(true));
        assert_eq!(j.progress(), 0.);
        j.land();
        assert_eq!(j.timer, LEAP_INTERVAL);
        assert!(!j.can_leap(true));
        j.timer = 0.25;
        assert!((j.progress() - 0.5).abs() < 1e-6);
        j.timer = 0.;
        assert!(j.can_leap(true));
    }
    #[test]
    fn dismount_and_incap_use_post_ride_and_post_incap_intervals() {
        let mut j = Jockey::default();
        j.mount(0, Vec3::ZERO);
        assert_eq!(j.progress(), 0.);
        j.dismount(RideEnd::Dismount);
        assert_eq!((j.timer, j.duration), (6., 6.));
        assert!(j.victim.is_none());
        j.mount(1, Vec3::ZERO);
        j.dismount(RideEnd::Incap);
        assert_eq!((j.timer, j.duration), (30., 30.));
    }
    #[test]
    fn ride_damage_waits_for_the_delay_then_ticks_every_interval() {
        let mut j = Jockey::default();
        j.mount(0, Vec3::ZERO);
        let dt = 1. / 30.;
        let mut ticks = vec![];
        for step in 1..=110 {
            if j.ride_step(dt) > 0. {
                ticks.push(step as f32 * dt);
            }
        }
        assert_eq!(ticks.len(), 3);
        assert!((ticks[0] - 1.).abs() < 0.04);
        assert!((ticks[1] - 2.).abs() < 0.04);
        assert!((ticks[2] - 3.).abs() < 0.04);
    }
    #[test]
    fn control_oscillates_around_retail_point_eight_and_is_clamped() {
        assert!((control(1., 0.) - 0.8).abs() < 1e-6);
        assert!((control(0.2, 0.) - 0.8).abs() < 1e-6);
        assert!((control(1., std::f32::consts::FRAC_PI_2) - 1.).abs() < 1e-6);
        assert!((control(1., -std::f32::consts::FRAC_PI_2) - 0.45).abs() < 1e-6);
        let mut j = Jockey::default();
        j.mount(0, Vec3::ZERO);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for _ in 0..300 {
            j.ride_step(1. / 30.);
            let c = j.control(1.);
            lo = lo.min(c);
            hi = hi.max(c);
        }
        assert!(lo < 0.5 && hi > 0.99);
    }
    #[test]
    fn jockey_steers_a_passive_victim_and_resistance_slows_it() {
        let forward = Vec3::NEG_Z;
        let (dir, speed) = ride_wish(forward, Vec3::ZERO, 0.8, 1.);
        assert!((dir - forward).length() < 1e-6);
        assert!((speed - 200.).abs() < 1e-3);
        // Full control is capped by the 220 survivor run speed.
        assert!((ride_wish(forward, Vec3::ZERO, 1., 1.).1 - 220.).abs() < 1e-3);
        // An opposing victim keeps (c - (1 - c)) of the move.
        let (dir, speed) = ride_wish(forward, Vec3::Z, 0.8, 1.);
        assert!((dir - forward).length() < 1e-6);
        assert!((speed - 150.).abs() < 1e-3);
        // A sideways pull bends the path toward the victim's teammates.
        let (dir, _) = ride_wish(forward, Vec3::X, 0.8, 1.);
        assert!(dir.x > 0.2 && dir.z < -0.9);
        // Input magnitudes do not matter, only directions.
        assert_eq!(
            ride_move(forward * 5., Vec3::ZERO, 0.8),
            ride_move(forward, Vec3::ZERO, 0.8)
        );
    }
    #[test]
    fn mounted_speed_follows_health_with_retail_floor() {
        assert!((mounted_top_speed(1.) - 220.).abs() < 1e-3);
        assert!((mounted_top_speed(0.8) - 176.).abs() < 1e-3);
        assert!((mounted_top_speed(0.3) - 132.).abs() < 1e-3);
        assert!((ride_wish(Vec3::NEG_Z, Vec3::ZERO, 1., 0.3).1 - 132.).abs() < 1e-3);
    }
    #[test]
    fn attach_blends_onto_the_victim_over_retail_time() {
        let mut j = Jockey::default();
        j.mount(0, Vec3::new(0., 1., 0.));
        let target = Vec3::new(2., 0., 0.);
        assert_eq!(j.attach_point(target), Vec3::new(0., 1., 0.));
        j.ride_step(0.2);
        assert!((j.attach_point(target) - Vec3::new(1., 0.5, 0.)).length() < 1e-5);
        j.ride_step(0.3);
        assert_eq!(j.attach_point(target), target);
    }
    #[test]
    fn facing_shove_deflects_but_side_or_idle_survivors_are_ridden() {
        let victim = Vec3::ZERO;
        let jockey = Vec3::X * 1.5;
        assert!(shove_deflects(true, Vec3::X, victim, jockey));
        assert!(shove_deflects(true, Vec3::new(1., 0., 0.9), victim, jockey));
        assert!(!shove_deflects(true, Vec3::Z, victim, jockey));
        assert!(!shove_deflects(false, Vec3::X, victim, jockey));
    }
    #[test]
    fn leap_contact_sweeps_survivor_hulls_and_skips_pinned_or_incapped() {
        let c = Collision::test_solid_floor();
        let mut squad = Squad(vec![
            member(Vec3::new(0., 0., -3.)),
            member(Vec3::new(0., 0., -1.5)),
            member(Vec3::new(3., 0., -2.)),
        ]);
        let from = Vec3::new(0., 0.3, 0.);
        let to = Vec3::new(0., 0.3, -4.);
        assert_eq!(leap_victim(&squad, &c, from, to), Some(1));
        squad.0[1].state.incap = true;
        assert_eq!(leap_victim(&squad, &c, from, to), Some(0));
        squad.0[0].state.pinned = true;
        assert_eq!(leap_victim(&squad, &c, from, to), None);
    }
    #[test]
    fn stopped_victim_bot_gives_no_ride_input() {
        let member = |position: Vec3| {
            let mut m = crate::game_state::Companion {
                state: Match::new(&crate::game_state::tests_weapon()),
                position,
                spawn: position,
                model: String::new(),
                vertical: 0.,
                cooldown: 0.,
                revive_time: 0.,
                aim: Vec3::Z,
            };
            m.state.dead = false;
            m
        };
        let squad = Squad(vec![member(Vec3::ZERO), member(Vec3::Z * 5.)]);
        assert!(victim_input(&squad, 0).length() > 0.5);
        crate::game_state::set_bots_stopped(true);
        assert_eq!(victim_input(&squad, 0), Vec3::ZERO);
        crate::game_state::set_bots_stopped(false);
    }
    #[test]
    fn victim_bot_pulls_toward_nearest_standing_teammate() {
        let mut squad = Squad(vec![
            member(Vec3::ZERO),
            member(Vec3::new(5., 0., 0.)),
            member(Vec3::new(0., 0., 3.)),
        ]);
        assert!((victim_input(&squad, 0) - Vec3::Z).length() < 1e-6);
        squad.0[2].state.incap = true;
        assert!((victim_input(&squad, 0) - Vec3::X).length() < 1e-6);
        squad.0[1].state.dead = true;
        assert_eq!(victim_input(&squad, 0), Vec3::ZERO);
    }
    #[test]
    fn claw_reaches_retail_range_only_in_front() {
        let c = Collision::test_solid_floor();
        let squad = Squad(vec![member(Vec3::new(0., 0., -1.6))]);
        let eye = Vec3::Y * 1.5;
        assert_eq!(claw_victim(&squad, &c, eye, Vec3::NEG_Z, 0.), Some(0));
        assert_eq!(claw_victim(&squad, &c, eye, Vec3::Z, 0.), None);
        let far = Squad(vec![member(Vec3::new(0., 0., -2.5))]);
        assert_eq!(claw_victim(&far, &c, eye, Vec3::NEG_Z, 0.), None);
    }
    #[test]
    fn viewmodel_and_cues_match_the_native_arms_and_soundscripts() {
        assert_eq!(vm_activity(3), Some("ACT_VM_IDLE"));
        assert_eq!(vm_activity(2), None);
    }
    #[test]
    fn owned_jockey_assets_have_activities_sounds_and_hud_artwork() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            MODEL,
            VIEW,
            "scripts/weapon_jockey_claw.txt",
            "resource/ui/hud/jockeyhealth.res",
            "materials/vgui/hud/pz_charge_jockey.vtf",
        ] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "jockeyzombie.alert",
            "jockeyzombie.death",
            "jockeyzombie.pain",
            "jockeyzombie.recognize",
            "jockeyzombie.ride",
            "zombie.punch",
            "claw.hit",
            "claw.hitflesh",
            "claw.swing",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
        }
        let ride = registry.definitions.get("jockeyzombie.ride").unwrap();
        for wave in &ride.waves {
            let bytes = assets.read(wave).unwrap();
            assert!(crate::game_audio::wave_has_loop(&bytes), "{wave}");
        }
        let assets = std::cell::RefCell::new(assets);
        let load = |path: &str| {
            l4d2_formats::anim::Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok())
                .unwrap()
        };
        let has = |sk: &l4d2_formats::anim::Skeleton, activity: &str| {
            sk.sequences().iter().any(|s| s.activity_name == activity)
        };
        let view = load(VIEW);
        for activity in [
            "ACT_VM_DEPLOY",
            "ACT_VM_IDLE",
            "ACT_VM_LUNGE",
            "ACT_VM_PRIMARYATTACK",
        ] {
            assert!(has(&view, activity), "{activity}");
        }
        assert!(!has(&view, "ACT_VM_PUMMEL_ATTACK"));
        let world = load(MODEL);
        assert!(has(&world, "ACT_TERROR_JOCKEY_RIDE"));
        assert!(world.find_sequence("jockey_ride").is_some());
        for survivor in ["gambler", "producer", "coach", "mechanic"] {
            let sk = load(&format!("models/survivors/survivor_{survivor}.mdl"));
            assert!(has(&sk, "ACT_TERROR_JOCKEY_RIDDEN"), "{survivor}");
        }
    }
}
