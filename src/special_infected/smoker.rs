//! Playable Smoker: tongue targeting, flight, grab, drag, choke, release and
//! recharge, plus claw melee, the visible tongue rope and the death cloud.
//!
//! The shared lifecycle (team switch, ghost spawn, death/respawn and the
//! survivor-bot opposition) lives in special_infected.rs; this module runs
//! after `special_infected::tick`. Retail values come from the running retail
//! game (build 10097, netconsole) and the decompiled server.dll CTongue code;
//! see local-research/si-agents/smoker/NOTES.md for addresses and citations.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, HEIGHT, RADIUS, UNIT},
    game::Body,
    game_state::{Match, Squad},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::sync::atomic::{AtomicU8, Ordering};
pub const WEAPON: &str = "weapon_smoker_claw";
pub const MODEL: &str = "models/infected/smoker.mdl";
pub const VIEW: &str = "models/v_models/weapons/v_claw_smoker.mdl";
/// Retail z_gas_health = 250.
pub const HEALTH: f32 = 250.;
/// Retail z_gas_speed = 210.
pub const SPEED: f32 = 210.;
pub const PLAYABLE: bool = true;
/// The view switches to a third-person chase camera the moment the tongue
/// grabs a survivor (special_phase 3) and stays there while reeling and
/// choking; the world Smoker body is drawn instead of the claw viewmodel.
pub const THIRD_PERSON_PIN: bool = true;
pub const VM_ACTIONS: &[(&str, &str)] = &[("tongue", "ACT_VM_TONGUE")];
pub const CONTROLS: &str =
    "Smoker playtest: aim at a survivor and left click to shoot the tongue; right click claws.";
/// Phase 2 (tongue out) and phase 3 (reeling/choking) play the native tongue
/// activity once; claw (4) and idle keep the shared defaults.
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    matches!(phase, 2 | 3).then_some("ACT_VM_TONGUE")
}

// Retail convars (netconsole, build 10097) unless noted.
/// tongue_range.
const RANGE: f32 = 750. * UNIT;
/// tongue_fly_speed.
const FLY_SPEED: f32 = 1000. * UNIT;
/// tongue_vs_cone_start_tolerance (versus); the scan accepts dot >= 1 - tol.
const CONE_TOLERANCE: f32 = 0.015;
/// tongue_hit_delay: recharge after a choke/incap hold.
pub const HIT_DELAY: f32 = 20.;
/// tongue_miss_delay: recharge started at launch, and after a drag-only hold
/// in versus (server FUN_1036a690).
pub const MISS_DELAY: f32 = 15.;
/// tongue_start_pull_delay.
const START_PULL_DELAY: f32 = 0.1;
/// tongue_victim_max_speed / tongue_victim_acceleration.
const VICTIM_MAX_SPEED: f32 = 175. * UNIT;
const VICTIM_ACCELERATION: f32 = 30.;
/// tongue_gravity_force, applied as force * dt^2 per tick (FUN_1036a7b0).
const GRAVITY_FORCE: f32 = 4000. * UNIT;
/// tongue_break_from_damage_amount.
pub const BREAK_DAMAGE: f32 = 50.;
/// tongue_choke_damage_amount / tongue_choke_damage_interval.
pub const CHOKE_DAMAGE: f32 = 10.;
pub const DAMAGE_INTERVAL: f32 = 1.;
/// tongue_drag_damage_amount (versus only in FUN_103490a0).
pub const DRAG_DAMAGE: f32 = 3.;
/// tongue_los_forgiveness_time.
const LOS_FORGIVENESS: f32 = 1.;
/// tongue_player_dropping_to_ground_time.
const DROP_TIME: f32 = 1.;
/// Misfire flight time (FUN_1036f110 compares against 1.0).
const MISFIRE_TIME: f32 = 1.;
/// Grab when the tip is within sqrt(25) units (FUN_1036f820).
const GRAB_DISTANCE: f32 = 5. * UNIT;
/// The bend array holds ten points; the eleventh breaks the tongue.
const MAX_BENDS: usize = 10;
/// tongue_bend_point_deflection.
const BEND_DEFLECTION: f32 = 5. * UNIT;
/// tongue_no_progress_choke_early_delay / _ambush_delay.
const EARLY_DELAY: f32 = 1.5;
const AMBUSH_DELAY: f32 = 0.5;
/// tongue_no_progress_choke_time / _damage_interval / _tolerance /
/// tongue_no_progress_break_interval.
const NO_PROGRESS_TIME: f32 = 0.5;
const NO_PROGRESS_DAMAGE_DELAY: f32 = 0.5;
const NO_PROGRESS_TOLERANCE: f32 = 25. * UNIT;
const NO_PROGRESS_BREAK: f32 = 10.;
/// tongue_vertical_choke_time_off_ground; hanging is only considered within
/// 50 horizontal units of the pull point (2500 squared, FUN_1036e1b0).
const HANG_TIME: f32 = 0.5;
const HANG_RADIUS: f32 = 50. * UNIT;
/// Victims within 48 units (2304 squared) of the pull point are lifted at
/// 300 units/sec when the pull point is above them (FUN_1036a7b0).
const LIFT_RADIUS: f32 = 48. * UNIT;
const LIFT_SPEED: f32 = 300. * UNIT;
/// smoker_pz_claw_dmg, claw_range, claw_swing_interval / _miss_interval.
const CLAW_DAMAGE: f32 = 4.;
const CLAW_RANGE: f32 = 52. * UNIT;
const CLAW_INTERVAL: f32 = 1.;
const CLAW_MISS_INTERVAL: f32 = 0.5;
/// Native smoker_tongue render_rope: Radius Random 6..7 (exponent 5, so
/// mostly near 6) is the ribbon's full width, texel_size 0.05. Only the
/// middle half of smoker_tongue_beam is opaque, so the visible tongue is
/// about 3 units across, as in retail (retail-smoker-tongue-1111.jpg). It
/// was 13 units (twice the radius), an arm-thick tube.
const ROPE_WIDTH: f32 = 6.2 * UNIT;
const ROPE_TEXELS_PER_METER: f32 = 0.05 / UNIT;
const CLOUD_ENTITY: &str = "__rewrite_smoker_smokecloud";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TongueState {
    #[default]
    InMouth,
    Misfire,
    Extending,
    Attached,
    Dropping,
}
/// Survivor victim pose, read by l4d2r_animation for `pinned_by == Smoker`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum VictimPose {
    #[default]
    Pulled,
    Falling,
    ChokeGround,
    ChokeStanding,
    Hanging,
}
/// Survivor models face +X. Dragging faces the pull anchor, whereas a
/// completed choke holds the victim facing away from the Smoker.
pub(crate) fn victim_yaw(toward: Vec3, pose: VictimPose) -> Option<f32> {
    let flat = toward * Vec3::new(1., 0., 1.);
    (flat.length_squared() > 1e-6).then(|| {
        (-flat.z).atan2(flat.x)
            + if matches!(
                pose,
                VictimPose::ChokeGround | VictimPose::ChokeStanding | VictimPose::Hanging
            ) {
                std::f32::consts::PI
            } else {
                0.
            }
    })
}
static VICTIM_POSE: AtomicU8 = AtomicU8::new(0);
impl VictimPose {
    /// Native survivor activities (survivor_*.mdl) for this pose.
    pub fn activity(self) -> &'static str {
        match self {
            Self::Pulled => "ACT_TERROR_DRAGGING_FROM_TONGUE",
            Self::Falling => "ACT_TERROR_IDLE_FALL_FROM_TONGUE",
            Self::ChokeGround => "ACT_TERROR_CHOKING_TONGUE_GROUND",
            Self::ChokeStanding => "ACT_TERROR_STANDING_CHOKE_FROM_TONGUE",
            Self::Hanging => "ACT_TERROR_HANGING_FROM_TONGUE",
        }
    }
    fn chest(self) -> f32 {
        match self {
            Self::Pulled | Self::ChokeGround => 14. * UNIT,
            _ => 48. * UNIT,
        }
    }
}
/// Pose of a survivor held by an AI Smoker (one Smoker at a time, z_smoker_limit 1).
pub(crate) fn set_ai_victim_pose(pose: VictimPose) {
    VICTIM_POSE.store(pose as u8, Ordering::Relaxed);
}
/// Pose of the survivor currently held by the local Smoker.
pub fn victim_pose() -> VictimPose {
    match VICTIM_POSE.load(Ordering::Relaxed) {
        1 => VictimPose::Falling,
        2 => VictimPose::ChokeGround,
        3 => VictimPose::ChokeStanding,
        4 => VictimPose::Hanging,
        _ => VictimPose::Pulled,
    }
}
#[derive(Resource, Default)]
pub struct Smoker {
    pub state: TongueState,
    state_time: f32,
    /// Tongue tip while out of the mouth (render space, metres).
    pub tip: Vec3,
    tip_velocity: Vec3,
    /// Tongue start point (the Smoker's eye) at the last tick.
    pub mouth: Vec3,
    /// Bend points around world obstacles, nearest the Smoker first.
    pub bends: Vec<Vec3>,
    target: Option<usize>,
    los_lost: f32,
    pub victim: Option<usize>,
    grab_health: f32,
    pub choking: bool,
    pub hanging: bool,
    ambush: bool,
    close_grab: bool,
    attached_time: f32,
    pulling: bool,
    victim_speed: f32,
    victim_vertical: f32,
    damage_clock: f32,
    progress_mark: f32,
    progress_time: f32,
    stuck_time: f32,
    off_ground: f32,
    pub pose: VictimPose,
    /// Ability recharge remaining / total (seconds).
    pub timer: f32,
    pub timer_total: f32,
    claw: f32,
    dead: bool,
    cloud: Option<usize>,
    cloud_until: Option<f32>,
    capture_started: bool,
}
/// Why an attached tongue let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Break {
    Damage,
    VictimDied,
    LeftGround,
    BentTooMuch,
    LostSight,
    NoProgress,
}
/// Per-tick Smoker inputs, in render space (metres).
pub struct Frame {
    pub dt: f32,
    pub eye: Vec3,
    pub aim: Vec3,
    pub feet: Vec3,
    pub grounded: bool,
    pub fire: bool,
}
/// Versus recharge after a hold (server FUN_1036a690): drag-only holds use
/// tongue_miss_delay, chokes and incapacitated victims tongue_hit_delay.
pub fn release_delay(choked: bool, victim_incap: bool) -> f32 {
    if choked || victim_incap {
        HIT_DELAY
    } else {
        MISS_DELAY
    }
}
fn chest(member: &crate::game_state::Companion, pose: VictimPose) -> Vec3 {
    member.position
        + Vec3::Y
            * if member.state.incap {
                14. * UNIT
            } else {
                pose.chest()
            }
}
fn clear(c: &Collision, from: Vec3, to: Vec3) -> bool {
    c.trace(from, to, Vec3::ZERO).fraction >= 1.
}
fn segment_point(point: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let delta = to - from;
    let t = ((point - from).dot(delta) / delta.length_squared().max(1e-8)).clamp(0., 1.);
    from + delta * t
}
/// TongueTargetScan: the visible survivor nearest the crosshair inside the
/// versus cone and tongue_range. Retail tests the cone against the target
/// entity; this accepts the closest point of the survivor's body column.
pub fn find_target(c: &Collision, eye: Vec3, aim: Vec3, squad: &Squad) -> Option<usize> {
    let aim = aim.normalize_or_zero();
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead && !m.state.pinned)
        .filter_map(|(i, m)| {
            let top = m.position
                + Vec3::Y
                    * if m.state.incap {
                        30. * UNIT
                    } else {
                        64. * UNIT
                    };
            let bottom = m.position + Vec3::Y * (8. * UNIT);
            // Closest body point to the aim ray.
            let along = |p: Vec3| (p - eye).dot(aim).max(0.);
            let ray_point = |p: Vec3| eye + aim * along(p);
            let mut best = segment_point(ray_point(top), bottom, top);
            for _ in 0..3 {
                best = segment_point(ray_point(best), bottom, top);
            }
            let delta = best - eye;
            let distance = delta.length();
            let dot = aim.dot(delta / distance.max(1e-6));
            (distance <= RANGE && dot >= 1. - CONE_TOLERANCE && clear(c, eye, best))
                .then_some((i, dot))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
impl Smoker {
    fn enter(&mut self, state: TongueState) {
        self.state = state;
        self.state_time = 0.;
    }
    fn reset_hold(&mut self) {
        self.victim = None;
        self.target = None;
        self.bends.clear();
        self.choking = false;
        self.hanging = false;
        self.pulling = false;
        self.victim_speed = 0.;
        self.victim_vertical = 0.;
        self.pose = VictimPose::Pulled;
        self.los_lost = 0.;
    }
    /// Returns to an idle tongue (ghost, death, role change).
    pub fn reset(&mut self) {
        self.reset_hold();
        self.enter(TongueState::InMouth);
        self.timer = 0.;
        self.timer_total = 0.;
        self.claw = 0.;
    }
    pub fn ready(&self) -> bool {
        self.state == TongueState::InMouth && self.timer <= 0.
    }
    pub fn progress(&self) -> f32 {
        if self.state != TongueState::InMouth {
            0.
        } else if self.timer_total <= 0. {
            1.
        } else {
            (1. - self.timer / self.timer_total).clamp(0., 1.)
        }
    }
    /// Ends a hold, computes the retail recharge and drops the tongue.
    fn let_go(&mut self, hunter: &mut Hunter, squad: &mut Squad, state: &mut Match) {
        let incap = self
            .victim
            .and_then(|i| squad.0.get(i))
            .is_some_and(|m| m.state.incap);
        let delay = release_delay(self.choking, incap);
        if let Some(i) = self.victim {
            if let Some(m) = squad.0.get(i) {
                self.tip = chest(m, self.pose);
            }
        }
        if hunter.pin.is_some() {
            super::release(hunter, squad, state);
        }
        self.timer = delay;
        self.timer_total = delay;
        self.reset_hold();
        self.tip_velocity = Vec3::ZERO;
        self.enter(TongueState::Dropping);
        state.special_phase = 2;
    }
    /// One fixed tick of the tongue ability. Returns sound events.
    pub fn step(
        &mut self,
        f: &Frame,
        c: &Collision,
        hunter: &mut Hunter,
        state: &mut Match,
        squad: &mut Squad,
    ) -> (Vec<&'static str>, Option<Break>) {
        let mut sounds = vec![];
        let mut broke = None;
        self.mouth = f.eye;
        self.state_time += f.dt;
        // The launch timer keeps running while the tongue is out.
        self.timer = (self.timer - f.dt).max(0.);
        // A bot shove (shared opposition) released the victim.
        if self.state == TongueState::Attached && hunter.pin != self.victim {
            let victim = self.victim;
            hunter.pin = victim;
            self.let_go(hunter, squad, state);
            sounds.push("SmokerZombie.TongueRetract");
        }
        match self.state {
            TongueState::InMouth => {
                if f.fire && self.ready() && f.grounded {
                    self.tip = f.eye;
                    self.timer = MISS_DELAY;
                    self.timer_total = MISS_DELAY;
                    self.target = find_target(c, f.eye, f.aim, squad);
                    self.los_lost = 0.;
                    if self.target.is_some() {
                        self.enter(TongueState::Extending);
                    } else {
                        self.tip_velocity = f.aim.normalize_or_zero() * FLY_SPEED;
                        self.enter(TongueState::Misfire);
                    }
                    state.special_phase = 2;
                    state.shot_serial = state.shot_serial.wrapping_add(1);
                    sounds.push("SmokerZombie.TongueAttack");
                    sounds.push("SmokerZombie.TongueFly");
                }
            }
            TongueState::Misfire => {
                if self.tip_velocity != Vec3::ZERO {
                    let next = self.tip + self.tip_velocity * f.dt;
                    let hit = c.trace(self.tip, next, Vec3::ZERO);
                    self.tip = self.tip.lerp(next, hit.fraction.clamp(0., 1.));
                    if hit.fraction < 1. {
                        self.tip_velocity = Vec3::ZERO;
                    }
                }
                if self.state_time > MISFIRE_TIME {
                    self.enter(TongueState::InMouth);
                    state.special_phase = 0;
                    sounds.push("SmokerZombie.TongueRetract");
                }
            }
            TongueState::Extending => {
                let target = self.target.filter(|&i| {
                    squad
                        .0
                        .get(i)
                        .is_some_and(|m| !m.state.dead && !m.state.pinned)
                });
                let Some(i) = target else {
                    self.enter(TongueState::Dropping);
                    return (sounds, broke);
                };
                let point = chest(&squad.0[i], VictimPose::ChokeStanding);
                if clear(c, f.eye, point) {
                    self.los_lost = 0.;
                } else {
                    self.los_lost += f.dt;
                    if self.los_lost > LOS_FORGIVENESS {
                        self.enter(TongueState::Dropping);
                        return (sounds, broke);
                    }
                }
                let delta = point - self.tip;
                let next = self.tip + delta.clamp_length_max(FLY_SPEED * f.dt);
                let hit = c.trace(self.tip, next, Vec3::ZERO);
                if hit.fraction < 1. {
                    self.tip = self.tip.lerp(next, hit.fraction.max(0.));
                    self.enter(TongueState::Dropping);
                    return (sounds, broke);
                }
                self.tip = next;
                if self.tip.distance(point) < GRAB_DISTANCE {
                    if self.los_lost > 0. {
                        self.enter(TongueState::Dropping);
                    } else {
                        self.grab(i, f, hunter, state, squad);
                        sounds.push("SmokerZombie.TongueHit");
                    }
                }
            }
            TongueState::Attached => {
                broke = self.hold(f, c, hunter, state, squad, &mut sounds);
                if broke.is_some() {
                    self.let_go(hunter, squad, state);
                }
            }
            TongueState::Dropping => {
                // The tongue sags to the ground before it is reeled back in.
                self.tip_velocity.y -= GRAVITY_FORCE * f.dt;
                let next = self.tip + self.tip_velocity * f.dt;
                let hit = c.trace(self.tip, next, Vec3::ZERO);
                self.tip = self.tip.lerp(next, hit.fraction.clamp(0., 1.));
                if hit.fraction < 1. {
                    self.tip_velocity = Vec3::ZERO;
                }
                if self.state_time > DROP_TIME {
                    self.enter(TongueState::InMouth);
                    state.special_phase = 0;
                    sounds.push("SmokerZombie.TongueRetract");
                }
            }
        }
        (sounds, broke)
    }
    fn grab(
        &mut self,
        i: usize,
        f: &Frame,
        hunter: &mut Hunter,
        state: &mut Match,
        squad: &mut Squad,
    ) {
        let member = &squad.0[i];
        let toward = (f.feet - member.position) * Vec3::new(1., 0., 1.);
        self.ambush = member.aim.dot(toward.normalize_or_zero()) < 0.;
        self.close_grab = toward.length() < 2. * LIFT_RADIUS;
        self.victim = Some(i);
        self.target = None;
        self.grab_health = state.health;
        self.attached_time = 0.;
        self.pulling = false;
        self.victim_speed = 0.;
        self.victim_vertical = 0.;
        self.damage_clock = DAMAGE_INTERVAL;
        self.progress_mark = member.position.distance(f.feet);
        self.progress_time = 0.;
        self.stuck_time = 0.;
        self.off_ground = 0.;
        self.choking = false;
        self.hanging = false;
        self.pose = VictimPose::Pulled;
        self.bends.clear();
        hunter.pin = Some(i);
        hunter.rescue_point = Some(member.position);
        super::pin_member(squad, i, SpecialClass::Smoker);
        state.special_phase = 3;
        self.enter(TongueState::Attached);
    }
    /// Keeps the bend chain valid. Returns false when the tongue can no
    /// longer reach the victim (bent too often or blocked).
    fn update_bends(&mut self, c: &Collision, mouth: Vec3, end: Vec3) -> Result<(), Break> {
        update_bends(&mut self.bends, c, mouth, end)
    }
    fn hold(
        &mut self,
        f: &Frame,
        c: &Collision,
        hunter: &mut Hunter,
        state: &mut Match,
        squad: &mut Squad,
        sounds: &mut Vec<&'static str>,
    ) -> Option<Break> {
        let i = self.victim?;
        let Some(member) = squad.0.get(i) else {
            return Some(Break::VictimDied);
        };
        if member.state.dead {
            return Some(Break::VictimDied);
        }
        if state.health <= self.grab_health - BREAK_DAMAGE {
            return Some(Break::Damage);
        }
        if !f.grounded {
            return Some(Break::LeftGround);
        }
        self.attached_time += f.dt;
        if let Err(reason) = self.update_bends(c, f.eye, chest(member, self.pose)) {
            return Some(reason);
        }
        // Pull toward the last bend point, or the Smoker himself.
        let bent = !self.bends.is_empty();
        let anchor = self.bends.last().copied().unwrap_or(f.feet);
        let mut position = member.position;
        let horizontal = (anchor - position) * Vec3::new(1., 0., 1.);
        let distance = horizontal.length();
        let grounded = c.grounded(position, HEIGHT);
        let lift = distance < LIFT_RADIUS && anchor.y > position.y + HEIGHT;
        if !self.choking && self.attached_time >= START_PULL_DELAY {
            if !self.pulling {
                self.pulling = true;
                sounds.push("SmokerZombie.TongueRetract");
            }
            self.victim_speed = (self.victim_speed + VICTIM_ACCELERATION * VICTIM_MAX_SPEED * f.dt)
                .min(VICTIM_MAX_SPEED);
            // Bodies stop at the Smoker's hull; bend points can be reached.
            let stop = if bent { 0. } else { 2. * RADIUS + UNIT };
            let travel = (distance - stop).clamp(0., self.victim_speed * f.dt);
            let flat = horizontal.normalize_or_zero() * travel;
            let vertical = if lift {
                (anchor.y - HEIGHT - position.y).clamp(0., LIFT_SPEED.min(VICTIM_MAX_SPEED) * f.dt)
            } else if grounded {
                0.
            } else {
                -GRAVITY_FORCE * f.dt * f.dt
            };
            position = if lift {
                // Lifting ignores the floor; walls and ceilings still block.
                let up = c.trace(
                    position + Vec3::Y * HEIGHT * 0.5,
                    position + Vec3::Y * (HEIGHT * 0.5 + vertical),
                    Vec3::new(RADIUS, HEIGHT * 0.5, RADIUS),
                );
                let raised = position + Vec3::Y * vertical * up.fraction.clamp(0., 1.);
                c.walk(raised, flat, HEIGHT, false)
            } else {
                c.walk(position, flat + Vec3::Y * vertical, HEIGHT, grounded)
            };
        } else if !self.hanging && !grounded {
            position = c.walk(
                position,
                Vec3::NEG_Y * GRAVITY_FORCE * f.dt * f.dt,
                HEIGHT,
                false,
            );
        }
        let grounded = c.grounded(position, HEIGHT);
        self.off_ground = if grounded { 0. } else { self.off_ground + f.dt };
        squad.0[i].position = position;
        hunter.rescue_point = Some(position);
        let toward = (anchor - position) * Vec3::new(1., 0., 1.);
        // Hanging choke: off the ground near the pull point (FUN_1036e1b0).
        let near = toward.length() < HANG_RADIUS;
        if near && self.off_ground > HANG_TIME && !self.hanging {
            self.choking = true;
            self.hanging = true;
            self.damage_clock = DAMAGE_INTERVAL;
        }
        // Ground choke after a lack of progress (FUN_1036e4c0).
        let early = if self.ambush {
            AMBUSH_DELAY
        } else {
            EARLY_DELAY
        };
        if self.attached_time >= early && !self.hanging {
            let current = position.distance(anchor);
            if self.progress_mark - current > NO_PROGRESS_TOLERANCE {
                self.progress_mark = current;
                self.progress_time = 0.;
                self.stuck_time = 0.;
                if self.choking {
                    self.choking = false;
                    self.damage_clock = DAMAGE_INTERVAL;
                }
            } else {
                self.progress_time += f.dt;
                if self.progress_time > NO_PROGRESS_TIME {
                    self.stuck_time += f.dt;
                    if self.stuck_time > NO_PROGRESS_DAMAGE_DELAY && !self.choking {
                        self.choking = true;
                        self.damage_clock = DAMAGE_INTERVAL;
                    }
                }
                if !self.choking && self.progress_time > NO_PROGRESS_BREAK {
                    return Some(Break::NoProgress);
                }
            }
        }
        self.pose = if self.hanging {
            VictimPose::Hanging
        } else if self.choking {
            if self.close_grab && grounded {
                VictimPose::ChokeStanding
            } else {
                VictimPose::ChokeGround
            }
        } else if self.off_ground > 0.1 {
            VictimPose::Falling
        } else {
            VictimPose::Pulled
        };
        if let Some(yaw) = victim_yaw(toward, self.pose) {
            hunter.pin_yaw = yaw - std::f32::consts::FRAC_PI_2;
        }
        self.damage_clock -= f.dt;
        if self.damage_clock <= 0. {
            self.damage_clock += DAMAGE_INTERVAL;
            squad.0[i].state.damage(if self.choking {
                CHOKE_DAMAGE
            } else {
                DRAG_DAMAGE
            });
        }
        if squad.0[i].state.dead {
            return Some(Break::VictimDied);
        }
        None
    }
}
pub fn install(app: &mut App) {
    app.init_resource::<Smoker>()
        .init_resource::<BodyMouth>()
        .add_systems(FixedUpdate, tick.after(super::tick))
        .add_systems(
            Update,
            (world_model, draw_tongue)
                .chain()
                .after(crate::game::controls),
        )
        .add_systems(
            Startup,
            prepare_cloud.before(crate::source_particles::setup),
        );
}
fn prepare_cloud(
    mut smoker: ResMut<Smoker>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    use l4d2r_game_core::{
        entities::{Ent, Kind},
        mapents::MapEnt,
    };
    let Some(mut logic) = logic else {
        return;
    };
    if let Some(&id) = logic.world.find(CLOUD_ENTITY).first() {
        smoker.cloud = Some(id);
        return;
    }
    let index = logic.world.ents.len();
    let id = logic.world.add_runtime_entity(Ent {
        me: MapEnt {
            index,
            classname: "info_particle_system".into(),
            targetname: CLOUD_ENTITY.into(),
            origin: l4d2r_math::Vec3::ZERO,
            angles: l4d2r_math::Vec3::ZERO,
            mdl: None,
            brush: None,
            ent: l4d2_formats::bsp::Entity(vec![
                ("classname".into(), "info_particle_system".into()),
                ("targetname".into(), CLOUD_ENTITY.into()),
                ("effect_name".into(), "smoker_smokecloud".into()),
            ]),
        },
        conns: vec![],
        kind: Kind::Particle {
            active: false,
            started: 0.,
        },
        alive: true,
        templated: false,
        parent: None,
    });
    smoker.cloud = Some(id);
}
fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut smoker: ResMut<Smoker>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    let now = logic.as_ref().map_or(0., |l| l.world.time);
    // A map restart rebuilds the entity list: re-resolve the cloud emitter
    // by name rather than trusting a stale index.
    if let Some(l) = logic.as_ref() {
        smoker.cloud = l.world.find(CLOUD_ENTITY).first().copied();
    }
    if let (Some(until), Some(id), Some(logic)) = (smoker.cloud_until, smoker.cloud, logic.as_mut())
    {
        if now >= until || now + 5. < until {
            if let Some(e) = logic.world.ents.get_mut(id) {
                e.kind = l4d2r_game_core::entities::Kind::Particle {
                    active: false,
                    started: now,
                };
            }
            smoker.cloud_until = None;
        }
    }
    if !state.infected || state.special_class != SpecialClass::Smoker {
        if smoker.state != TongueState::InMouth || smoker.victim.is_some() {
            smoker.reset();
        }
        smoker.dead = false;
        return;
    }
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    if state.dead {
        if !smoker.dead {
            smoker.dead = true;
            // Death + Explode voices: see `voice`.
            smoker.reset();
            if let (Some(id), Some(logic)) = (smoker.cloud, logic.as_mut()) {
                if let Some(e) = logic.world.ents.get_mut(id) {
                    let f = body.feet;
                    e.me.origin = l4d2r_math::Vec3::new(f.x, -f.z, f.y) / UNIT;
                    e.kind = l4d2r_game_core::entities::Kind::Particle {
                        active: true,
                        started: now,
                    };
                    smoker.cloud_until = Some(now + 0.2);
                }
            }
        }
        hunter.ability_progress = Some(0.);
        return;
    }
    smoker.dead = false;
    // `--capture-smoker-tongue`: spawn, stage the squad ahead and fire once.
    let capture = std::env::args().any(|a| a == "--capture-smoker-tongue");
    if capture && hunter.ghost && !smoker.capture_started && state.elapsed > 0.3 {
        smoker.capture_started = true;
        hunter.ghost = false;
        state.special_phase = 0;
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let side = forward.cross(Vec3::Y);
        for (i, member) in squad.0.iter_mut().enumerate() {
            member.position = body.feet + forward * 15. + side * (i as f32 * 1.5);
            member.cooldown = 10.;
        }
        body.pitch = -0.1;
    }
    if hunter.ghost {
        if smoker.state != TongueState::InMouth || smoker.timer > 0. {
            smoker.reset();
        }
        hunter.ability_progress = Some(1.);
        return;
    }
    let dt = time.delta_secs();
    let aim = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let eye = body.feet + Vec3::Y * body.stance.eye();
    let fire = std::mem::take(&mut input.fire)
        || input.fire_held
        || capture && state.elapsed > 1. && smoker.timer_total == 0.;
    let claw = std::mem::take(&mut input.shove) || input.shove_held;
    input.reload = false;
    input.use_item = false;
    let collision = collision.for_actor(true, false);
    let frame = Frame {
        dt,
        eye,
        aim,
        feet: body.feet,
        grounded: body.grounded,
        fire: fire && smoker.claw <= 0.,
    };
    let before = smoker.state;
    let (events, broke) = smoker.step(&frame, &collision, &mut hunter, &mut state, &mut squad);
    if capture && (before != smoker.state || broke.is_some()) {
        println!(
            "Smoker capture: t={:.2} {:?} -> {:?} break={broke:?} grounded={} fire={} victim={:?} pose={:?}",
            state.elapsed,
            before,
            smoker.state,
            frame.grounded,
            frame.fire,
            smoker.victim,
            smoker.pose
        );
    }
    for name in events {
        // The tongue-attack yell is a voice (FUN_1036f400 -> FUN_10347590).
        if name == "SmokerZombie.TongueAttack" {
            super::voice::speak(&mut hunter, &mut sounds, name, 0.);
        } else {
            sounds.play(name, None);
        }
    }
    VICTIM_POSE.store(smoker.pose as u8, Ordering::Relaxed);
    if smoker.state != TongueState::InMouth {
        // Max speed is 1 unit/sec while the tongue is out (FUN_102c9940).
        body.velocity.x = 0.;
        body.velocity.z = 0.;
    }
    smoker.claw = (smoker.claw - dt).max(0.);
    if smoker.state == TongueState::InMouth && claw && smoker.claw <= 0. {
        let victim = squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.state.dead)
            .find(|(_, m)| {
                let point = segment_point(
                    eye + aim * CLAW_RANGE,
                    m.position + Vec3::Y * (8. * UNIT),
                    m.position + Vec3::Y * (64. * UNIT),
                );
                let delta = point - eye;
                delta.length() < CLAW_RANGE + RADIUS
                    && aim.dot(delta.normalize_or_zero()) > 0.6
                    && clear(&collision, eye, point)
            })
            .map(|(i, _)| i);
        // Retail: Claw.Swing on every swing, then the impact sound.
        sounds.play("Claw.Swing", None);
        super::voice::clawed(&mut hunter);
        if let Some(i) = victim {
            squad.0[i].state.damage(CLAW_DAMAGE);
            sounds.play("Claw.HitFlesh", None);
            smoker.claw = CLAW_INTERVAL;
        } else {
            if logic
                .as_mut()
                .is_some_and(|l| l.pound_obstacle(body.feet, aim, &collision))
            {
                sounds.play("Claw.Hit", None);
            }
            smoker.claw = CLAW_MISS_INTERVAL;
        }
        state.special_phase = 4;
        state.shot_serial = state.shot_serial.wrapping_add(1);
    } else if state.special_phase == 4 && smoker.claw <= 0. {
        state.special_phase = 0;
    }
    hunter.ability_progress = Some(smoker.progress());
}
/// Keeps a tongue's bend chain valid (player and AI Smokers). Fails when the
/// tongue can no longer reach `end` (bent too often or blocked).
pub(crate) fn update_bends(bends: &mut Vec<Vec3>, c: &Collision, mouth: Vec3, end: Vec3) -> Result<(), Break> {
    // tongue_unbend: drop bends once the previous anchor sees the victim.
    while !bends.is_empty() {
        let before = if bends.len() >= 2 {
            bends[bends.len() - 2]
        } else {
            mouth
        };
        if clear(c, before, end) {
            bends.pop();
        } else {
            break;
        }
    }
    let anchor = bends.last().copied().unwrap_or(mouth);
    let hit = c.trace(anchor, end, Vec3::ZERO);
    if hit.fraction >= 1. {
        return Ok(());
    }
    if bends.len() >= MAX_BENDS {
        return Err(Break::BentTooMuch);
    }
    // Retail sweeps the previous clear line toward the new one in two-unit
    // steps (FUN_1036cc20); this searches outward from the contact
    // instead, for the nearest point that sees both ends.
    let contact = anchor.lerp(end, hit.fraction.max(0.)) + hit.normal * BEND_DEFLECTION;
    let along = ((end - anchor) * Vec3::new(1., 0., 1.)).normalize_or_zero();
    let side = along.cross(Vec3::Y).normalize_or_zero();
    let directions = [side, -side, Vec3::Y, along, -Vec3::Y, -along];
    for step in 0..=16 {
        for direction in directions {
            let bend = contact + direction * (step as f32 * BEND_DEFLECTION);
            if clear(c, anchor, bend) && clear(c, bend, end) {
                bends.push(bend);
                return Ok(());
            }
            if step == 0 {
                break;
            }
        }
    }
    Err(Break::LostSight)
}
#[derive(Component)]
struct TongueRope;
pub(crate) fn rope_mesh(points: &[Vec3], camera: Vec3) -> Mesh {
    let mut positions = vec![];
    let mut normals = vec![];
    let mut uvs = vec![];
    let mut indices: Vec<u32> = vec![];
    let mut length = 0.;
    for (k, &p) in points.iter().enumerate() {
        let tangent = if k + 1 < points.len() {
            points[k + 1] - p
        } else if k > 0 {
            p - points[k - 1]
        } else {
            Vec3::X
        }
        .normalize_or_zero();
        if k > 0 {
            length += p.distance(points[k - 1]);
        }
        let view = (camera - p).normalize_or_zero();
        let mut side = tangent.cross(view).normalize_or_zero();
        if side == Vec3::ZERO {
            side = tangent.any_orthonormal_vector();
        }
        let half = side * ROPE_WIDTH * 0.5;
        let v = length * ROPE_TEXELS_PER_METER;
        positions.push((p - half).to_array());
        positions.push((p + half).to_array());
        normals.push(view.to_array());
        normals.push(view.to_array());
        uvs.push([0., v]);
        uvs.push([1., v]);
        if k > 0 {
            let b = (k as u32 - 1) * 2;
            indices.extend([b, b + 1, b + 2, b + 1, b + 3, b + 2]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}
/// Densifies the tongue path so long spans stay camera-facing.
pub(crate) fn rope_points(path: &[Vec3]) -> Vec<Vec3> {
    let mut out = vec![];
    for pair in path.windows(2) {
        let pieces = (pair[0].distance(pair[1]) / 0.25).ceil().clamp(1., 64.) as usize;
        for s in 0..pieces {
            out.push(pair[0].lerp(pair[1], s as f32 / pieces as f32));
        }
    }
    if let Some(&last) = path.last() {
        out.push(last);
    }
    out
}
/// smoker_tongue particles: 25 mapped from start to end along the path
/// (Position Along Path Sequential / Maintain Position Along Path).
const TONGUE_PARTICLES: usize = 25;
/// Movement Dampen Relative to Control Point falloff range.
const TONGUE_DAMPEN_RANGE: f32 = 32. * UNIT;
/// The smoker_tongue rope: particles spread along the bent path, each with
/// the Oscillate Vector wobble (rate -3..3 units/s at 0.2..0.5 Hz) damped
/// near the mouth, the tip and every bend, drawn with render_rope's four
/// Catmull-Rom subdivisions. The wobble amplitude (rate / 2*pi*f) follows
/// the operator values; per-particle random streams are not retail's.
pub(crate) fn tongue_points(path: &[Vec3], clock: f32) -> Vec<Vec3> {
    if path.len() < 2 {
        return path.to_vec();
    }
    let mut lengths = vec![0.];
    for pair in path.windows(2) {
        lengths.push(lengths.last().unwrap() + pair[0].distance(pair[1]));
    }
    let total = *lengths.last().unwrap();
    if total < 1e-4 {
        return path.to_vec();
    }
    let at = |s: f32| {
        let k = lengths.partition_point(|l| *l <= s).clamp(1, path.len() - 1);
        let span = (lengths[k] - lengths[k - 1]).max(1e-6);
        path[k - 1].lerp(path[k], ((s - lengths[k - 1]) / span).clamp(0., 1.))
    };
    // Particles plus the bend points themselves, so corners stay exact.
    let mut stations: Vec<(f32, bool)> = (0..TONGUE_PARTICLES)
        .map(|i| (total * i as f32 / (TONGUE_PARTICLES - 1) as f32, false))
        .collect();
    stations.extend(lengths[1..lengths.len() - 1].iter().map(|l| (*l, true)));
    stations.sort_by(|a, b| a.0.total_cmp(&b.0));
    stations.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-4 && {
        a.1 |= b.1;
        true
    });
    let hash = |i: usize, salt: u32| {
        let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
        x ^= x >> 15;
        x = x.wrapping_mul(0x2C1B_3C6D);
        x ^= x >> 12;
        (x & 0xFFFF) as f32 / 65535.
    };
    let points: Vec<Vec3> = stations
        .iter()
        .enumerate()
        .map(|(i, &(s, bend))| {
            let p = at(s);
            if bend {
                return p;
            }
            let near = lengths
                .iter()
                .map(|l| (s - l).abs())
                .fold(f32::INFINITY, f32::min);
            let damp = (near / TONGUE_DAMPEN_RANGE).clamp(0., 1.);
            let mut offset = Vec3::ZERO;
            for axis in 0..3 {
                let rate = (hash(i, axis * 3 + 1) * 6. - 3.) * UNIT;
                let frequency = 0.2 + hash(i, axis * 3 + 2) * 0.3;
                let phase = hash(i, axis * 3 + 3) * std::f32::consts::TAU;
                offset[axis as usize] = rate / (std::f32::consts::TAU * frequency)
                    * (std::f32::consts::TAU * frequency * clock + phase).sin();
            }
            p + offset * damp
        })
        .collect();
    crate::ropes::catmull_rom(&points, 4)
}
pub(crate) fn tongue_material(
    source: &crate::game_ui::HudSource,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let texture = crate::source_assets::SourceAssets::new(&source.0, vec![])
        .and_then(|mut a| a.read("materials/particle/smoker_tongue_beam.vtf"))
        .and_then(|b| crate::vtf::decode(&b, 256))
        .ok()
        .map(|t| {
            let mut image = Image::new(
                Extent3d {
                    width: t.width,
                    height: t.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                t.rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::ClampToEdge,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
            images.add(image)
        });
    // smoker_tongue Color Random midpoint (78,56,66)..(51,36,43). The Cable
    // shader has no spritecard modulate doubling; retail's tongue reads as a
    // dark maroon rope.
    materials.add(StandardMaterial {
        base_color: Color::srgb(TONGUE_TINT[0], TONGUE_TINT[1], TONGUE_TINT[2]),
        base_color_texture: texture,
        alpha_mode: AlphaMode::Mask(0.5),
        unlit: true,
        // Bevy's DistanceFog uses a linear ramp with the HDR-scaled fog
        // colour; Source squares the ramp (BlendPixelFog). Unsquared, a
        // dark tongue a few metres away turned pale tan in Hotel's haze.
        fog_enabled: false,
        double_sided: true,
        cull_mode: None,
        ..default()
    })
}
/// smoker_tongue Color Random midpoint (78,56,66)..(51,36,43).
const TONGUE_TINT: [f32; 3] = [65. / 255., 46. / 255., 55. / 255.];
fn draw_tongue(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    smoker: Res<Smoker>,
    squad: Res<Squad>,
    body_mouth: Res<BodyMouth>,
    source: Option<Res<crate::game_ui::HudSource>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ropes: Query<(Entity, &Mesh3d), With<TongueRope>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    time: Res<Time>,
) {
    let visible = state.infected
        && state.special_class == SpecialClass::Smoker
        && !state.dead
        && !hunter.ghost
        && smoker.state != TongueState::InMouth;
    let Some(camera) = camera.iter().next().filter(|_| visible) else {
        for (entity, _) in &ropes {
            commands.entity(entity).despawn();
        }
        return;
    };
    let end = match smoker.state {
        TongueState::Attached => smoker
            .victim
            .and_then(|i| squad.0.get(i))
            .map(|m| chest(m, smoker.pose))
            .unwrap_or(smoker.tip),
        _ => smoker.tip,
    };
    let start = if let Some(mouth) = body_mouth.0 {
        // Third person: from the drawn Smoker's animated mouth.
        mouth
    } else if state.infected_third_person() {
        let toward = (path_first(&smoker, end) - smoker.mouth) * Vec3::new(1., 0., 1.);
        smoker.mouth + toward.normalize_or_zero() * 0.22 - Vec3::Y * 0.1
    } else {
        // Leave the mouth just below the first-person view.
        camera.translation() + camera.forward() * 0.45 + camera.down() * 0.3
    };
    let mut path = vec![start];
    path.extend(smoker.bends.iter().copied());
    path.push(end);
    let mesh = rope_mesh(&tongue_points(&path, time.elapsed_secs()), camera.translation());
    if let Some((_, handle)) = ropes.iter_mut().next() {
        if let Some(mut existing) = meshes.get_mut(&handle.0) {
            *existing = mesh;
        }
        return;
    }
    let Some(source) = source else {
        return;
    };
    let material = material
        .get_or_insert_with(|| tongue_material(&source, &mut images, &mut materials))
        .clone();
    commands.spawn((
        TongueRope,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::default(),
        NoFrustumCulling,
        RenderLayers::layer(1),
    ));
}
/// First point the tongue heads to from the mouth.
fn path_first(smoker: &Smoker, end: Vec3) -> Vec3 {
    smoker.bends.first().copied().unwrap_or(end)
}
/// Native smoker.mdl loop while the tongue holds a survivor: reeling in, or
/// the choke once the victim is held at the Smoker.
fn world_sequence(smoker: &Smoker) -> (&'static str, f32) {
    if smoker.choking {
        ("tongue_attack_incap_survivor_idle", 10.333)
    } else {
        ("tongue_attack_drag_survivor_idle", 3.033)
    }
}
/// smoker.mdl draw 1 is the extended tongue, which rests sideways out of the
/// mouth (about 0.74 m) and retail bends toward the victim procedurally. The
/// rope already draws the tongue from the mouth, so the third-person body
/// leaves that piece out.
const EXTENDED_TONGUE_DRAW: usize = 1;
#[derive(Component)]
struct SmokerDraw(usize);
/// World position of the drawn third-person body's `smoker_mouth`
/// attachment, where the tongue rope starts.
#[derive(Resource, Default)]
struct BodyMouth(Option<Vec3>);
#[allow(clippy::too_many_arguments)]
fn world_model(
    mut commands: Commands,
    state: Res<Match>,
    smoker: Res<Smoker>,
    squad: Res<Squad>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut draws: Query<(Entity, &SmokerDraw, &Mesh3d, &mut Transform)>,
    mut clock: Local<(f32, &'static str)>,
    mut mouth: ResMut<BodyMouth>,
) {
    mouth.0 = None;
    if !(state.infected_third_person() && state.special_class == SpecialClass::Smoker) {
        for (entity, ..) in &draws {
            commands.entity(entity).despawn();
        }
        *clock = (0., "");
        return;
    }
    let (label, length) = world_sequence(&smoker);
    if clock.1 != label {
        *clock = (0., label);
    } else if !state.paused {
        clock.0 = (clock.0 + time.delta_secs()) % length;
    }
    // Face the victim; the model's forward is +X (as the Charger's).
    let toward = smoker
        .victim
        .and_then(|i| squad.0.get(i))
        .map(|m| (m.position - body.feet) * Vec3::new(1., 0., 1.))
        .filter(|d| d.length_squared() > 1e-4)
        .unwrap_or(Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z);
    let yaw = (-toward.x).atan2(-toward.z);
    let transform = Transform::from_translation(body.feet)
        .with_rotation(Quat::from_rotation_y(yaw + std::f32::consts::FRAC_PI_2));
    mouth.0 = library
        .scripted_attachment(MODEL, label, clock.0, "smoker_mouth")
        .map(|local| transform.transform_point(local));
    if draws.is_empty() {
        if let Some(parts) = models.0.get(&(MODEL.into(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                if index == EXTENDED_TONGUE_DRAW {
                    continue;
                }
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.scripted(MODEL, index, label, clock.0, &mut owned);
                commands.spawn((
                    SmokerDraw(index),
                    Mesh3d(meshes.add(owned)),
                    MeshMaterial3d(material.clone()),
                    transform,
                    RenderLayers::layer(1),
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
        return;
    }
    for (_, draw, mesh, mut location) in &mut draws {
        *location = transform;
        if !state.paused {
            if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
                library.scripted(MODEL, draw.0, label, clock.0, &mut mesh);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_state::Companion;
    fn survivor(position: Vec3) -> Companion {
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
            aim: Vec3::Z,
        }
    }
    fn smoker_state() -> Match {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        state.special_class = SpecialClass::Smoker;
        state.health = HEALTH;
        state.special_phase = 0;
        state
    }
    fn frame(fire: bool) -> Frame {
        Frame {
            dt: 1. / 30.,
            eye: Vec3::Y * (62. * UNIT),
            aim: Vec3::NEG_Z,
            feet: Vec3::ZERO,
            grounded: true,
            fire,
        }
    }
    struct Rig {
        smoker: Smoker,
        hunter: Hunter,
        state: Match,
        squad: Squad,
        c: Collision,
    }
    impl Rig {
        fn new(victim: Vec3) -> Self {
            Self {
                smoker: Smoker::default(),
                hunter: Hunter::default(),
                state: smoker_state(),
                squad: Squad(vec![survivor(victim)]),
                c: Collision::test_solid_floor(),
            }
        }
        fn run(&mut self, f: &Frame, ticks: usize) -> Vec<&'static str> {
            let mut all = vec![];
            for _ in 0..ticks {
                let (sounds, _) = self.smoker.step(
                    f,
                    &self.c,
                    &mut self.hunter,
                    &mut self.state,
                    &mut self.squad,
                );
                all.extend(sounds);
            }
            all
        }
    }
    #[test]
    fn third_person_body_skips_the_sideways_extended_tongue_draw() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let draws = crate::studio::load(&mut assets, MODEL, 0, 0).unwrap();
        let tongue = &draws[EXTENDED_TONGUE_DRAW];
        let reach = tongue
            .positions
            .iter()
            .map(|p| p[0].abs().max(p[2].abs()))
            .fold(0f32, f32::max);
        let low = tongue
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::MAX, f32::min);
        assert!(reach > 0.6 && low > 1.5, "reach {reach} low {low}");
        assert_eq!(
            world_sequence(&Smoker::default()).0,
            "tongue_attack_drag_survivor_idle"
        );
    }
    #[test]
    fn retail_values_match_the_measured_convars() {
        assert_eq!(HEALTH, 250.);
        assert_eq!(SPEED, 210.);
        assert!((RANGE / UNIT - 750.).abs() < 1e-3);
        assert!((FLY_SPEED / UNIT - 1000.).abs() < 1e-3);
        assert!((VICTIM_MAX_SPEED / UNIT - 175.).abs() < 1e-3);
        assert_eq!((HIT_DELAY, MISS_DELAY), (20., 15.));
        assert_eq!((CHOKE_DAMAGE, DRAG_DAMAGE, BREAK_DAMAGE), (10., 3., 50.));
        assert!(SpecialClass::Smoker.playable());
        assert!(SpecialClass::Smoker.third_person_pin());
    }
    #[test]
    fn target_scan_uses_the_versus_cone_range_and_sight() {
        let c = Collision::test_solid_floor();
        let eye = Vec3::Y * (62. * UNIT);
        let ahead = Squad(vec![survivor(Vec3::NEG_Z * 10.)]);
        assert_eq!(find_target(&c, eye, Vec3::NEG_Z, &ahead), Some(0));
        // 15 degrees off the crosshair is outside the 0.985 cone.
        let off = Quat::from_rotation_y(0.26) * Vec3::NEG_Z;
        assert_eq!(find_target(&c, eye, off, &ahead), None);
        let far = Squad(vec![survivor(Vec3::NEG_Z * (760. * UNIT))]);
        assert_eq!(find_target(&c, eye, Vec3::NEG_Z, &far), None);
        let mut walled = Collision::test_solid_floor();
        walled.test_add_box(Vec3::new(-3., 0., -5.), Vec3::new(3., 4., -4.5));
        assert_eq!(find_target(&walled, eye, Vec3::NEG_Z, &ahead), None);
    }
    #[test]
    fn choking_victims_face_away_but_dragging_faces_the_anchor() {
        for toward in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
            for pose in [
                VictimPose::Pulled,
                VictimPose::Falling,
                VictimPose::ChokeGround,
                VictimPose::ChokeStanding,
                VictimPose::Hanging,
            ] {
                let yaw = victim_yaw(toward, pose).unwrap();
                let forward = Quat::from_rotation_y(yaw) * Vec3::X;
                let expected = if matches!(pose, VictimPose::Pulled | VictimPose::Falling) {
                    toward
                } else {
                    -toward
                };
                assert!(forward.dot(expected) > 0.9999, "{pose:?} {toward:?}");
                // The playable Smoker passes through the shared pin-yaw
                // convention; it must produce the same body orientation.
                let local = super::super::pin_rotation(yaw - std::f32::consts::FRAC_PI_2, false);
                assert!((local * Vec3::X).dot(expected) > 0.9999);
            }
        }
        assert_eq!(victim_yaw(Vec3::Y, VictimPose::ChokeGround), None);
    }
    #[test]
    fn a_hit_flies_at_tongue_speed_grabs_pulls_and_chokes() {
        let mut rig = Rig::new(Vec3::NEG_Z * 10.);
        let sounds = rig.run(&frame(true), 1);
        assert!(sounds.contains(&"SmokerZombie.TongueAttack"));
        assert_eq!(rig.smoker.state, TongueState::Extending);
        assert_eq!(rig.state.special_phase, 2);
        assert_eq!(rig.smoker.timer, MISS_DELAY);
        // 10 m at 1000 units/sec (25.4 m/s) takes ~0.39 s.
        let sounds = rig.run(&frame(false), 13);
        assert_eq!(rig.smoker.state, TongueState::Attached, "{sounds:?}");
        assert!(sounds.contains(&"SmokerZombie.TongueHit"));
        assert_eq!(rig.hunter.pin, Some(0));
        assert_eq!(rig.squad.0[0].state.pinned_by, Some(SpecialClass::Smoker));
        assert_eq!(rig.state.special_phase, 3);
        // One second of pulling moves the victim 175 units.
        let before = rig.squad.0[0].position;
        rig.run(&frame(false), 30);
        // 175 units/sec after the 0.1 s pull delay (28-29 moving ticks).
        let moved = rig.squad.0[0].position.z - before.z;
        assert!((150.0..=175.).contains(&(moved / UNIT)), "{}", moved / UNIT);
        assert!(rig.squad.0[0].state.health < 100.);
        // Reaching the Smoker stops progress, which starts the ground choke.
        rig.run(&frame(false), 150);
        assert!(rig.smoker.choking);
        assert!(rig.squad.0[0].position.length() < 2. * RADIUS + 2. * UNIT);
        let health = rig.squad.0[0].state.health;
        rig.run(&frame(false), 30);
        assert!((health - rig.squad.0[0].state.health - CHOKE_DAMAGE).abs() < 1e-3);
    }
    #[test]
    fn drag_damage_is_three_per_second_before_any_choke() {
        let mut rig = Rig::new(Vec3::NEG_Z * 15.);
        rig.run(&frame(true), 1);
        rig.run(&frame(false), 25);
        assert_eq!(rig.smoker.state, TongueState::Attached);
        let health = rig.squad.0[0].state.health;
        rig.run(&frame(false), 30);
        assert!(!rig.smoker.choking);
        assert!((health - rig.squad.0[0].state.health - DRAG_DAMAGE).abs() < 1e-3);
    }
    #[test]
    fn a_miss_misfires_then_retracts_and_keeps_the_miss_recharge() {
        let mut rig = Rig::new(Vec3::X * 10.);
        rig.run(&frame(true), 1);
        assert_eq!(rig.smoker.state, TongueState::Misfire);
        rig.run(&frame(false), 15);
        // Fifteen ticks at 1000 units/sec.
        assert!(
            (rig.smoker.tip.z + 500. * UNIT).abs() < 0.05,
            "{}",
            rig.smoker.tip
        );
        let sounds = rig.run(&frame(false), 16);
        assert_eq!(rig.smoker.state, TongueState::InMouth);
        assert!(sounds.contains(&"SmokerZombie.TongueRetract"));
        assert!(!rig.smoker.ready());
        assert!(rig.smoker.progress() > 0. && rig.smoker.progress() < 0.2);
        assert_eq!(rig.state.special_phase, 0);
        rig.run(&frame(true), 30 * 13);
        assert_eq!(rig.smoker.state, TongueState::InMouth);
        rig.run(&frame(false), 30);
        assert!(rig.smoker.ready());
    }
    #[test]
    fn damage_to_the_smoker_breaks_the_tongue_and_sets_the_recharge() {
        let mut rig = Rig::new(Vec3::NEG_Z * 10.);
        rig.run(&frame(true), 14);
        assert_eq!(rig.smoker.state, TongueState::Attached);
        rig.state.damage(49.);
        rig.run(&frame(false), 1);
        assert_eq!(rig.smoker.state, TongueState::Attached);
        rig.state.damage(1.);
        let (_, broke) = rig.smoker.step(
            &frame(false),
            &rig.c,
            &mut rig.hunter,
            &mut rig.state,
            &mut rig.squad,
        );
        assert_eq!(broke, Some(Break::Damage));
        assert_eq!(rig.smoker.state, TongueState::Dropping);
        assert!(!rig.squad.0[0].state.pinned);
        assert_eq!(rig.hunter.pin, None);
        assert_eq!(rig.smoker.timer_total, MISS_DELAY);
        rig.run(&frame(false), 31);
        assert_eq!(rig.smoker.state, TongueState::InMouth);
        assert_eq!(rig.state.special_phase, 0);
    }
    #[test]
    fn rescue_victim_death_and_leaving_the_ground_release() {
        // A bot shove clears the shared pin; choke holds use the hit delay.
        let mut rig = Rig::new(Vec3::NEG_Z * 3.);
        rig.run(&frame(true), 120);
        assert!(rig.smoker.choking);
        super::super::release(&mut rig.hunter, &mut rig.squad, &mut rig.state);
        rig.run(&frame(false), 1);
        assert_eq!(rig.smoker.state, TongueState::Dropping);
        assert_eq!(rig.smoker.timer_total, HIT_DELAY);
        let mut rig = Rig::new(Vec3::NEG_Z * 10.);
        rig.run(&frame(true), 14);
        rig.squad.0[0].state.dead = true;
        rig.run(&frame(false), 1);
        assert_eq!(rig.smoker.state, TongueState::Dropping);
        let mut rig = Rig::new(Vec3::NEG_Z * 10.);
        rig.run(&frame(true), 14);
        let mut airborne = frame(false);
        airborne.grounded = false;
        let (_, broke) = rig.smoker.step(
            &airborne,
            &rig.c,
            &mut rig.hunter,
            &mut rig.state,
            &mut rig.squad,
        );
        assert_eq!(broke, Some(Break::LeftGround));
        assert_eq!(release_delay(false, true), HIT_DELAY);
    }
    #[test]
    fn the_tongue_bends_around_a_corner_and_breaks_after_ten_bends() {
        let mut c = Collision::test_solid_floor();
        // Pillar between the Smoker eye and a victim off to the side.
        c.test_add_box(Vec3::new(-0.5, 0., -3.5), Vec3::new(0.5, 4., -2.5));
        let mut smoker = Smoker::default();
        let eye = Vec3::Y * 1.5;
        assert!(!clear(&c, eye, Vec3::new(0.2, 1., -6.)));
        assert!(
            smoker
                .update_bends(&c, eye, Vec3::new(0.2, 1., -6.))
                .is_ok()
        );
        assert_eq!(smoker.bends.len(), 1);
        assert!(clear(&c, eye, smoker.bends[0]));
        assert!(clear(&c, smoker.bends[0], Vec3::new(0.2, 1., -6.)));
        // Unbends once the line is clear again.
        assert!(smoker.update_bends(&c, eye, Vec3::new(3., 1., -1.)).is_ok());
        assert!(smoker.bends.is_empty());
        smoker.bends = vec![eye; MAX_BENDS];
        assert_eq!(
            smoker.update_bends(&c, eye + Vec3::Z * 0.01, Vec3::new(0.2, 1., -6.)),
            Err(Break::BentTooMuch)
        );
    }
    #[test]
    fn a_smoker_above_lifts_the_victim_into_a_hanging_choke() {
        let mut c = Collision::test_solid_floor();
        // Ledge the Smoker stands on, 3 m above the floor.
        c.test_add_box(Vec3::new(-2., 0., -1.), Vec3::new(2., 3., 1.));
        let mut rig = Rig::new(Vec3::NEG_Z * 4.);
        rig.c = c;
        let mut f = frame(false);
        f.feet = Vec3::Y * 3.;
        f.eye = f.feet + Vec3::Y * (62. * UNIT);
        f.aim = (Vec3::new(0., 1., -4.) - f.eye).normalize();
        f.fire = true;
        rig.run(&f, 1);
        f.fire = false;
        rig.run(&f, 30 * 4);
        assert_eq!(rig.smoker.state, TongueState::Attached);
        assert!(rig.smoker.hanging, "{:?}", rig.squad.0[0].position);
        assert_eq!(rig.smoker.pose, VictimPose::Hanging);
        assert!(rig.squad.0[0].position.y > 0.5);
    }
    #[test]
    fn the_tongue_rope_keeps_its_ends_and_bends_and_wobbles_only_mid_span() {
        let path = [Vec3::ZERO, Vec3::new(4., 0., 0.), Vec3::new(4., 0., -6.)];
        for clock in [0., 0.7, 3.1] {
            let points = tongue_points(&path, clock);
            // 25 particles plus the bend, with four Catmull-Rom points per span.
            assert_eq!(points.len(), 26 + 25 * 4);
            assert_eq!(points[0], path[0]);
            assert_eq!(*points.last().unwrap(), path[2]);
            assert!(points.iter().any(|p| *p == path[1]), "the bend stays exact");
            // Wobble amplitude is at most rate / (2 pi f) = 3 / (2 pi 0.2) units.
            let limit = 3. / (std::f32::consts::TAU * 0.2) * UNIT * 3f32.sqrt() + 0.05;
            for p in &points {
                let to_path = if p.z.abs() < 1e-3 || p.x < 3.9 {
                    Vec3::new(0., p.y, p.z).length()
                } else {
                    Vec3::new(p.x - 4., p.y, 0.).length()
                };
                assert!(to_path < limit, "{p:?}");
            }
        }
        // Damped near the mouth: the first particle after it barely moves.
        let a = tongue_points(&path, 0.);
        let b = tongue_points(&path, 1.3);
        assert!(a[1].distance(b[1]) < 0.03, "{:?} {:?}", a[1], b[1]);
    }
    #[test]
    fn rope_mesh_faces_the_camera_and_tiles_along_its_length() {
        let points = rope_points(&[Vec3::ZERO, Vec3::NEG_Z * 2.]);
        assert!(points.len() >= 9);
        let mesh = rope_mesh(&points, Vec3::Y * 5.);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!()
        };
        assert!(((Vec3::from(p[0]) - Vec3::from(p[1])).length() - ROPE_WIDTH).abs() < 1e-4);
        assert!((p[0][1] - p[1][1]).abs() < 1e-4);
        assert_eq!(vm_activity(2), Some("ACT_VM_TONGUE"));
        assert_eq!(vm_activity(4), None);
    }
    #[test]
    fn owned_smoker_assets_have_activities_sounds_and_effects() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            MODEL,
            VIEW,
            "materials/particle/smoker_tongue_beam.vtf",
            "materials/vgui/hud/pz_charge_smoker.vtf",
            "resource/ui/hud/smokerhealth.res",
            "particles/smoker_fx.pcf",
        ] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let pcf =
            crate::source_pcf::parse(&assets.read("particles/smoker_fx.pcf").unwrap()).unwrap();
        for effect in ["smoker_smokecloud", "smoker_tongue"] {
            assert!(pcf.iter().any(|e| e.name == effect), "{effect}");
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "smokerzombie.alert",
            "smokerzombie.death",
            "smokerzombie.pain",
            "smokerzombie.breathe",
            "smokerzombie.explode",
            "smokerzombie.tongueattack",
            "smokerzombie.tonguefly",
            "smokerzombie.tonguehit",
            "smokerzombie.tongueretract",
            "claw.hitflesh",
            "claw.swing",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
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
            "ACT_VM_PRIMARYATTACK",
            "ACT_VM_TONGUE",
        ] {
            assert!(has(&view, activity), "{activity}");
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
            let survivor = load(&format!("models/survivors/survivor_{character}.mdl"));
            for pose in [
                VictimPose::Pulled,
                VictimPose::Falling,
                VictimPose::ChokeGround,
                VictimPose::ChokeStanding,
                VictimPose::Hanging,
            ] {
                // Rochelle has no standing choke; the victim branch falls
                // back to the ground choke.
                let fallback = pose == VictimPose::ChokeStanding
                    && has(&survivor, VictimPose::ChokeGround.activity());
                assert!(
                    has(&survivor, pose.activity()) || fallback,
                    "{character} {pose:?}"
                );
            }
        }
    }
}
