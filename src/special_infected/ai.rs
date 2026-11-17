//! AI special infected: the Hunter, Smoker, Boomer, Spitter, Jockey and
//! Charger the Director sends against the survivors (and `z_spawn <class>`).
//!
//! Each AI special is an ordinary `game::Enemy` (so gunfire, hitboxes, bots'
//! targeting, death animations and ragdolls are shared with the commons)
//! plus an [`AiSpecial`] brain driven by [`tick`]. `game::enemy_ai` keeps
//! them in its crowd (navigation routes, separation) but leaves their
//! movement and attacks to this module, like the Witch.
//!
//! Retail values are the live cvars of the installed game (netconsole,
//! `local-research/si-agents/ai-cvars.txt`) and the class modules' retail
//! constants. Behaviour is a first reconstruction: retail's per-class bot
//! behaviour trees (retreat/ambush choices, ledge and hazard steering,
//! shove reactions) are not yet reproduced; see KNOWN_GAPS.md.
use super::SpecialClass;
use crate::{
    collision::{Collision, EYE, HEIGHT, RADIUS, UNIT},
    game::{Body, Enemy},
    game_state::{Match, Squad},
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use crate::l4d2r_animation::{GpuBone, GpuSkin};
use std::sync::Mutex;

fn u(v: f32) -> f32 {
    v * UNIT
}
// Retail cvars (netconsole, build 10097).
/// z_spawn_safety_range / z_spawn_range.
pub const SPAWN_SAFETY_RANGE: f32 = 550.;
pub const SPAWN_RANGE: f32 = 1500.;
/// z_spawn_flow_limit (flow distance from the lead survivor, Source units).
pub const SPAWN_FLOW_LIMIT: f32 = 1500.;
/// hunter_pounce_ready_range, hunter_leap_away_give_up_range.
const POUNCE_READY_RANGE: f32 = 500.;
const POUNCE_GIVE_UP_RANGE: f32 = 1000.;
/// z_pounce_crouch_delay.
const POUNCE_CROUCH_DELAY: f32 = 1.;
/// hunter_pounce_loft_rate (degrees per unit) / hunter_pounce_max_loft_angle.
const POUNCE_LOFT_RATE: f32 = 0.035;
const POUNCE_MAX_LOFT: f32 = 45.;
/// z_pounce_damage every z_pounce_damage_interval.
const POUNCE_DAMAGE: f32 = 5.;
const POUNCE_DAMAGE_INTERVAL: f32 = 0.5;
/// smoker_tongue_delay (seconds of sight before the tongue fires).
const TONGUE_DELAY: f32 = 1.5;
/// z_vomit_range / z_vomit_interval / z_vomit_duration.
const VOMIT_RANGE: f32 = 300.;
const VOMIT_INTERVAL: f32 = 30.;
const VOMIT_DURATION: f32 = 1.5;
/// z_spit_range.
const SPIT_RANGE: f32 = 900.;
/// z_jockey_leap_range / z_jockey_leap_again_timer.
const LEAP_RANGE: f32 = 200.;
const LEAP_AGAIN: f32 = 5.;
/// z_charge_warmup.
const CHARGE_WARMUP: f32 = 0.5;
/// Range at which the bot Charger commits to a charge (approximation).
const CHARGE_RANGE: f32 = 600.;
/// Pain voice rate limit (retail player-infected pain, voice.rs).
const PAIN_INTERVAL: f32 = 0.7;
/// tank_throw_allow_range: the victim must be at least this far for a throw.
const TANK_THROW_ALLOW_RANGE: f32 = 250.;
/// tank_throw_min_interval.
const TANK_THROW_MIN_INTERVAL: f32 = 8.;
/// tank_stuck_time_suicide.
const TANK_STUCK_SUICIDE: f32 = 10.;
/// Seconds a minion may chase an unseen target without moving before it
/// backs off for MINION_REPOSITION_TIME (approximation of retail repositioning).
const MINION_STUCK_REPOSITION: f32 = 3.;
const MINION_REPOSITION_TIME: f32 = 2.5;
/// Deepest drop an AI special walks off while steering without a nav route
/// (about where infected fall damage starts).
const UNROUTED_MAX_DROP: f32 = 200.;
/// Recovery after the punch window before the next decision (seconds).
const TANK_SWING_RECOVER: f32 = 0.6;
/// Retail co-op Tank health: z_tank_health 4000 scaled by difficulty
/// (Easy 0.75, Normal 1, Advanced/Expert 2).
pub fn tank_health(difficulty: crate::campaign_settings::Difficulty) -> f32 {
    use crate::campaign_settings::Difficulty::*;
    4000. * match difficulty {
        Easy => 0.75,
        Normal => 1.,
        Advanced | Expert => 2.,
    }
}
pub(crate) fn difficulty_index(d: crate::campaign_settings::Difficulty) -> u8 {
    use crate::campaign_settings::Difficulty::*;
    match d {
        Easy => 0,
        Normal => 1,
        Advanced => 2,
        Expert => 3,
    }
}
/// A survivor flung by an AI Tank punch or knocked down by its rock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Knock {
    who: Victim,
    velocity: Vec3,
    airborne: bool,
    getup: f32,
    flight: f32,
    /// Body rotation while knocked: facing the Tank (punch) or the rock.
    facing: Quat,
    /// Who knocked the survivor: the Tank, or a charging Charger.
    by: SpecialClass,
}
/// Body rotation facing `point` from `from` (model +X forward).
fn facing(from: Vec3, point: Vec3) -> Quat {
    let face = (point - from) * Vec3::new(1., 0., 1.);
    Quat::from_rotation_y((-face.z).atan2(face.x))
}
/// The knocked survivor's native activity: the punch hit then flight while
/// airborne, the punch landing get-up, or the rock knockdown get-up.
fn knock_activity(k: &Knock) -> &'static str {
    if k.by == SpecialClass::Charger {
        // Survivors knocked aside by a charge: Hit_by_Charger, the fall loop,
        // then GetUpFrom_Charger.
        return if k.airborne && k.flight < CHARGER_HIT_CLIP {
            "ACT_TERROR_HIT_BY_CHARGER"
        } else if k.airborne {
            "ACT_TERROR_IDLE_FALL_FROM_CHARGERHIT"
        } else {
            "ACT_TERROR_CHARGERHIT_LAND_SLOW"
        };
    }
    if k.airborne {
        "ACT_TERROR_HIT_BY_TANKPUNCH"
    } else if k.flight > 0. {
        "ACT_TERROR_TANKPUNCH_LAND"
    } else {
        "ACT_TERROR_TANKROCK_TO_STAND"
    }
}
const GRAVITY: f32 = 800.;
/// Lead-in shown before the Charger fall loop (Hit_by_Charger, approx.).
const CHARGER_HIT_CLIP: f32 = 0.4;

/// A survivor an AI special can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Victim {
    Player,
    Bot(usize),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Chase,
    /// Hunter crouched and readying a pounce.
    Crouch(f32),
    /// Hunter lunge or Jockey leap in flight.
    Airborne(Vec3),
    /// Hunter pounce, Jockey ride or Charger pummel holding a survivor.
    Pin(Victim),
    /// Smoker tongue flying toward a survivor.
    Tongue {
        victim: Victim,
        tip: Vec3,
    },
    /// Smoker reeling in (choking once the survivor arrives).
    Reel {
        victim: Victim,
        choking: bool,
    },
    /// Boomer vomiting.
    Vomit(f32),
    /// Spitter spit latency.
    Spit(f32),
    /// Spitter keeping away after spitting.
    Flee(f32),
    /// Charger turning to charge.
    Warmup(f32),
    Charge {
        direction: Vec3,
        carried: Option<Victim>,
    },
    /// Charger staggering after a wall impact without a victim.
    Stagger(f32),
    /// Tank punch: wind-up, fist sweep, recovery. `low`: the ground pound
    /// on an incapacitated survivor (Attack_Incap, ACT_HULK_ATTACK_LOW).
    Swing { clock: f32, hit: bool, low: bool },
    /// Tank overhead rock throw (rooted).
    /// `variant`: which ACT_TANK_OVERHEAD_THROW sequence (tank::THROWS).
    Throw { clock: f32, aim: Vec3, released: bool, variant: usize },
}
#[derive(Component)]
pub struct AiSpecial {
    pub class: SpecialClass,
    pub mode: Mode,
    /// Ability recharge remaining (seconds).
    pub ability: f32,
    claw: f32,
    clock: f32,
    damage_clock: f32,
    sight: f32,
    speed: f32,
    previous_health: f32,
    grab_health: f32,
    pain: f32,
    alerted: bool,
    velocity: Vec3,
    dead: bool,
    logged: &'static str,
    /// View-style yaw of the pounce/leap that pinned the current victim.
    pin_yaw: f32,
    /// Last `Enemy::reaction_serial` seen (shoves from the player and bots).
    shove_serial: u64,
    /// Whether the last chase step had a NAV route (playtest trace).
    routed: bool,
    /// Survivors already hit by the current Tank swing (bit 0 player, then bots).
    swing_hits: u32,
    /// Difficulty health applied (AI Tank).
    scaled: bool,
    /// AI Tank set on fire: burns until death.
    on_fire: bool,
    /// Seconds an AI Tank has made no progress while chasing.
    stuck: f32,
    stuck_anchor: Vec3,
    /// Seconds below running speed (run/idle hysteresis).
    still: f32,
    /// Smoothed ground speed driving the run cycle's playback rate.
    shown_speed: f32,
    /// Smoker tongue bends around corners while reeling (smoker::update_bends).
    bends: Vec<Vec3>,
    /// Shove reaction for the current stagger (direction of the push), or a
    /// Hunter's pounce knock-off.
    stagger_act: &'static str,
    /// Falling (not a lunge/leap): ACT_JUMP.
    falling: bool,
    /// Hunter hard landing after a missed pounce (Lunge_Land_low_nav_01).
    land: f32,
    /// Smoker tongue_attack_to_idle after a hold or miss.
    end_tongue: f32,
    /// Hit flinch (activity, serial) for gesture slot 4.
    flinch: (&'static str, u64),
    /// Charger pummel: the carry ended against a wall (else the ground),
    /// the pound clip playing ("" during the slam and pummel intro), its
    /// clock, and how many pounds have been chosen (random stream).
    slam_wall: bool,
    pound: &'static str,
    pound_clock: f32,
    pounds: u32,
    /// Boomer: seconds until BoomerZombie.Fall may play again.
    fall_yell: f32,
}
/// Hit flinch layered over an AI special's animation (gesture slot 4).
#[derive(Component, Default)]
pub(crate) struct AiFlinch(pub &'static str, pub u64);
/// Lunge_nav_01 / Pounce_01 / Lunge_Land_low_nav_01 / tongue_attack_to_idle.
const HUNTER_LUNGE_START: f32 = 0.37;
const HUNTER_POUNCE_IMPACT: f32 = 0.33;
const HUNTER_LAND_HARD: f32 = 0.67;
const SMOKER_END_TONGUE: f32 = 0.33;
/// Side of the special a point lies on, in its body frame (model +X forward):
/// 0 front, 1 back, 2 left, 3 right.
fn side_of(rotation: Quat, from: Vec3, to: Vec3) -> usize {
    let local = rotation.inverse() * ((to - from) * Vec3::new(1., 0., 1.));
    if local.x.abs() >= local.z.abs() {
        if local.x >= 0. { 0 } else { 1 }
    } else if local.z < 0. {
        2
    } else {
        3
    }
}
/// Shove reaction pushed away from a shover on `side` (front shover pushes
/// it backward); a Hunter shoved off its victim plays the pounce knock-off.
fn shove_activity(class: SpecialClass, knocked_off: bool, side: usize) -> &'static str {
    if class == SpecialClass::Hunter && knocked_off {
        return ["ACT_TERROR_HUNTER_POUNCE_KNOCKOFF_BACKWARD", "ACT_TERROR_HUNTER_POUNCE_KNOCKOFF_FORWARD",
            "ACT_TERROR_HUNTER_POUNCE_KNOCKOFF_R", "ACT_TERROR_HUNTER_POUNCE_KNOCKOFF_L"][side];
    }
    ["ACT_TERROR_SHOVED_BACKWARD", "ACT_TERROR_SHOVED_FORWARD", "ACT_TERROR_SHOVED_RIGHTWARD", "ACT_TERROR_SHOVED_LEFTWARD"][side]
}
/// Hit flinch toward the shooter's side; None for classes without flinches.
fn flinch_activity(class: SpecialClass, crouched: bool, side: usize) -> Option<&'static str> {
    match class {
        SpecialClass::Boomer => Some("ACT_TERROR_FLINCH"),
        SpecialClass::Hunter if crouched => Some(["ACT_FLINCH_CROUCH_FRONT", "ACT_FLINCH_CROUCH_Back", "ACT_FLINCH_CROUCH_LEFT", "ACT_FLINCH_CROUCH_RIGHT"][side]),
        SpecialClass::Hunter | SpecialClass::Smoker | SpecialClass::Charger => {
            Some(["ACT_FLINCH_CHEST", "ACT_FLINCH_CHEST_BACK", "ACT_FLINCH_LEFTARM", "ACT_FLINCH_RIGHTARM"][side])
        }
        _ => None,
    }
}
fn mode_label(mode: &Mode) -> &'static str {
    match mode {
        Mode::Chase => "chase",
        Mode::Crouch(_) => "crouch",
        Mode::Airborne(_) => "airborne",
        Mode::Pin(Victim::Player) => "pin player",
        Mode::Pin(Victim::Bot(_)) => "pin bot",
        Mode::Tongue { .. } => "tongue",
        Mode::Reel { choking: false, .. } => "reel",
        Mode::Reel { choking: true, .. } => "choke",
        Mode::Vomit(_) => "vomit",
        Mode::Spit(_) => "spit",
        Mode::Flee(_) => "flee",
        Mode::Warmup(_) => "warmup",
        Mode::Charge { carried: None, .. } => "charge",
        Mode::Charge {
            carried: Some(_), ..
        } => "carry",
        Mode::Stagger(_) => "stagger",
        Mode::Swing { .. } => "swing",
        Mode::Throw { .. } => "throw",
    }
}
impl AiSpecial {
    pub fn new(class: SpecialClass) -> Self {
        Self {
            class,
            mode: Mode::Chase,
            ability: 0.,
            claw: 0.,
            clock: 0.,
            damage_clock: 0.,
            sight: 0.,
            speed: 0.,
            previous_health: health(class),
            grab_health: 0.,
            pain: 0.,
            alerted: false,
            velocity: Vec3::ZERO,
            dead: false,
            logged: "chase",
            pin_yaw: 0.,
            shove_serial: 0,
            routed: false,
            swing_hits: 0,
            scaled: false,
            on_fire: false,
            stuck: 0.,
            stuck_anchor: Vec3::ZERO,
            still: RUN_HOLD,
            shown_speed: 0.,
            bends: vec![],
            stagger_act: "ACT_TERROR_SHOVED_BACKWARD",
            slam_wall: false,
            pound: "",
            pound_clock: 0.,
            pounds: 0,
            fall_yell: 0.,
            falling: false,
            land: 0.,
            end_tongue: 0.,
            flinch: ("", 0),
        }
    }
    /// The survivor this special currently holds, if any.
    pub fn holding(&self) -> Option<Victim> {
        match self.mode {
            Mode::Pin(v) | Mode::Reel { victim: v, .. } => Some(v),
            Mode::Charge { carried, .. } => carried,
            _ => None,
        }
    }
    /// Gesture-slot activity layered over `activity` (server
    /// CTerrorPlayerAnimState::DoAnimationEvent 1033cb40): a Tank punch is
    /// ACT_TERROR_ATTACK_MOVING in slot 0 whether it runs or stands, and the
    /// ground pound on an incapacitated survivor is ACT_HULK_ATTACK_LOW.
    pub fn gesture(&self) -> Option<&'static str> {
        match (self.class, self.mode) {
            (SpecialClass::Tank, Mode::Swing { low: true, .. }) => Some("ACT_HULK_ATTACK_LOW"),
            (SpecialClass::Tank, Mode::Swing { .. }) => Some("ACT_TERROR_ATTACK_MOVING"),
            _ => None,
        }
    }
    /// Native activity for the animation system (models/infected/*.mdl).
    pub fn activity(&self) -> &'static str {
        // A one-tick stall (wall slide, step, route corner) keeps the run
        // cycle instead of restarting idle -> run.
        let moving = self.speed > 0.3 || self.still < RUN_HOLD;
        if self.falling && matches!(self.mode, Mode::Chase | Mode::Flee(_)) {
            return "ACT_JUMP";
        }
        match (self.class, self.mode) {
            (SpecialClass::Hunter, Mode::Chase) if self.land > 0. => "ACT_TERROR_HUNTER_LANDING_HARD",
            (SpecialClass::Smoker, Mode::Chase) if self.end_tongue > 0. => "ACT_TERROR_SMOKER_END_TONGUE_ATTACK",
            // smoker_tongue_delay: the tongue_attack_antic wind-up while aiming.
            (SpecialClass::Smoker, Mode::Chase) if self.sight > 0. && self.ability <= 0. => {
                "ACT_TERROR_SMOKER_PREPARE_TONGUE_LAUNCH"
            }
            // The punch is a gesture (`gesture`) over the Tank's locomotion.
            (SpecialClass::Tank, Mode::Swing { .. }) if moving => "ACT_RUN",
            (SpecialClass::Tank, Mode::Swing { .. }) => "ACT_IDLE",
            // A sequence label: the throw timing follows this exact clip.
            (SpecialClass::Tank, Mode::Throw { variant, .. }) => super::tank::THROWS[variant].0,
            (SpecialClass::Tank, Mode::Chase) if moving => "ACT_RUN",
            (SpecialClass::Tank, Mode::Chase) => "ACT_IDLE",
            (_, Mode::Chase | Mode::Flee(_)) if self.claw > 0.6 => "ACT_TERROR_ATTACK",
            (_, Mode::Chase | Mode::Flee(_)) if moving => "ACT_RUN",
            (_, Mode::Chase | Mode::Flee(_)) => "ACT_IDLE",
            (SpecialClass::Hunter, Mode::Crouch(_)) => "ACT_CROUCHIDLE",
            (SpecialClass::Hunter, Mode::Airborne(_)) if self.clock < HUNTER_LUNGE_START => "ACT_TERROR_HUNTER_LUNGE",
            (SpecialClass::Hunter, Mode::Airborne(_)) => "ACT_TERROR_HUNTER_LUNGE_IDLE",
            (SpecialClass::Hunter, Mode::Pin(_)) if self.clock < HUNTER_POUNCE_IMPACT => "ACT_TERROR_HUNTER_POUNCE",
            (SpecialClass::Hunter, Mode::Pin(_)) => "ACT_TERROR_HUNTER_POUNCE_MELEE",
            (SpecialClass::Jockey, Mode::Airborne(_)) => "ACT_JOCKEY_POUNCE",
            (SpecialClass::Jockey, Mode::Pin(_)) => "ACT_TERROR_JOCKEY_RIDE",
            // tongue_attack_grab_survivor while the tongue flies.
            (_, Mode::Tongue { .. }) => "ACT_TERROR_SMOKER_SENDING_OUT_TONGUE",
            (_, Mode::Reel { choking: false, .. }) => "ACT_TERROR_SMOKER_REELING_IN_TONGUE_IDLE",
            (_, Mode::Reel { choking: true, .. }) => "ACT_TERROR_SMOKER_CRITICAL_ATTACK_IDLE",
            (_, Mode::Vomit(_)) => "ACT_SECONDARYATTACK",
            (_, Mode::Spit(_)) => "ACT_TERROR_SPITTER_SPIT",
            (_, Mode::Warmup(_) | Mode::Charge { .. }) => "ACT_TERROR_CHARGER_CHARGE",
            (_, Mode::Stagger(_)) => self.stagger_act,
            // Slam (wall: knocked back; ground: Charger_slam_ground), the
            // pummel intro, then the chosen pound clip.
            (SpecialClass::Charger, Mode::Pin(_)) if self.clock < super::charger::slam_delay(self.slam_wall) => {
                if self.slam_wall { "ACT_TERROR_SHOVED_BACKWARD" } else { "ACT_TERROR_SLAM_GROUND" }
            }
            (SpecialClass::Charger, Mode::Pin(_)) if self.pound.is_empty() => "ACT_TERROR_CHARGER_PUMMEL",
            (SpecialClass::Charger, Mode::Pin(_)) => super::charger::pound_activity(self.pound),
            _ => "ACT_IDLE",
        }
    }
}
/// Activity read by `l4d2r_animation` for AI specials.
#[derive(Component)]
/// Main activity, and the slot-0 gesture layered over it (the Tank punch).
pub(crate) struct AiActivity(pub &'static str, pub Option<&'static str>);

pub fn health(class: SpecialClass) -> f32 {
    match class {
        SpecialClass::Hunter => 250.,
        SpecialClass::Smoker => super::smoker::HEALTH,
        SpecialClass::Boomer => super::boomer::HEALTH,
        SpecialClass::Spitter => super::spitter::HEALTH,
        SpecialClass::Jockey => super::jockey::HEALTH,
        SpecialClass::Charger => super::charger::HEALTH,
        SpecialClass::Tank => super::tank::HEALTH,
    }
}
/// z_<class>_speed (Source units/sec).
/// Seconds below running speed before a chasing special drops to idle.
const RUN_HOLD: f32 = 0.25;
/// Time constant (seconds) of the speed that scales the run cycle.
const SPEED_SMOOTHING: f32 = 0.15;
/// Chase body turn rate (radians/s). Not a measured retail value: retail
/// turns the body toward the path over several ticks (NextBot locomotion)
/// instead of snapping; the heavy Tank turns slowest.
fn turn_rate(class: SpecialClass) -> f32 {
    match class {
        SpecialClass::Tank => 360f32.to_radians(),
        _ => 720f32.to_radians(),
    }
}
pub fn run_speed(class: SpecialClass) -> f32 {
    match class {
        SpecialClass::Hunter => 300.,
        SpecialClass::Smoker => super::smoker::SPEED,
        SpecialClass::Boomer => super::boomer::SPEED,
        SpecialClass::Spitter => super::spitter::SPEED,
        SpecialClass::Jockey => super::jockey::SPEED,
        SpecialClass::Charger => super::charger::SPEED,
        SpecialClass::Tank => 210.,
    }
}
/// <class>_pz_claw_dmg.
pub fn claw_damage(class: SpecialClass) -> f32 {
    match class {
        SpecialClass::Hunter => 6.,
        SpecialClass::Charger => super::charger::CLAW_DAMAGE,
        _ => 4.,
    }
}
fn voice(class: SpecialClass) -> &'static str {
    match class {
        SpecialClass::Hunter => "HunterZombie",
        SpecialClass::Smoker => "SmokerZombie",
        SpecialClass::Boomer => "BoomerZombie",
        SpecialClass::Spitter => "SpitterZombie",
        SpecialClass::Jockey => "JockeyZombie",
        SpecialClass::Charger => "ChargerZombie",
        SpecialClass::Tank => "HulkZombie",
    }
}
fn alert_sound(class: SpecialClass) -> String {
    match class {
        // hulk soundscripts have no Alert/Recognize/Death: Yell and Die.
        SpecialClass::Tank => "HulkZombie.Yell".into(),
        SpecialClass::Hunter => "HunterZombie.Growl".into(),
        SpecialClass::Boomer => "BoomerZombie.Rage".into(),
        c => format!("{}.Recognize", voice(c)),
    }
}
/// Hunter pounce aim: toward the victim, lofted by hunter_pounce_loft_rate
/// degrees per unit of distance, up to hunter_pounce_max_loft_angle.
pub fn pounce_direction(from: Vec3, to: Vec3) -> Vec3 {
    let delta = to - from;
    let flat = (delta * Vec3::new(1., 0., 1.)).normalize_or(Vec3::NEG_Z);
    let distance = delta.length() / UNIT;
    let base = delta.y.atan2((delta * Vec3::new(1., 0., 1.)).length());
    let loft = (distance * POUNCE_LOFT_RATE)
        .min(POUNCE_MAX_LOFT)
        .to_radians();
    let pitch = (base + loft).min(POUNCE_MAX_LOFT.to_radians().max(base));
    flat * pitch.cos() + Vec3::Y * pitch.sin()
}

// --- Console / Director requests -------------------------------------------
static REQUESTS: Mutex<Vec<SpecialClass>> = Mutex::new(Vec::new());
/// `z_spawn <hunter|smoker|boomer|spitter|jockey|charger>`; `None` leaves
/// the command to others (the Witch handles `z_spawn witch`).
pub(crate) fn command(text: &str) -> Option<String> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    if !words.first()?.eq_ignore_ascii_case("z_spawn") {
        return None;
    }
    let class = SpecialClass::parse(words.get(1)?)?;
    if !SpecialClass::MINIONS.contains(&class) && class != SpecialClass::Tank {
        return None;
    }
    REQUESTS.lock().ok()?.push(class);
    Some(format!("Spawning {}", class.name()))
}
fn take_requests() -> Vec<SpecialClass> {
    REQUESTS
        .lock()
        .map(|mut r| std::mem::take(&mut *r))
        .unwrap_or_default()
}

// --- Victim poses shared with the survivor animation -----------------------
/// Survivor-player victim pose for the third-person pin view: (native
/// survivor activity, body rotation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayerPin {
    pub activity: &'static str,
    pub rotation: Quat,
}
static PLAYER_PIN: Mutex<Option<PlayerPin>> = Mutex::new(None);
pub(crate) fn player_pin() -> Option<PlayerPin> {
    PLAYER_PIN.lock().ok().and_then(|p| *p)
}
static CHARGER_POSES: Mutex<Vec<(usize, super::charger::VictimPose)>> = Mutex::new(Vec::new());
/// Survivors shoved by a teammate this tick. Shoving a survivor held by a
/// Smoker's tongue breaks the hold (retail rescue), so bots free a smoked
/// teammate by shoving them.
static SHOVED_VICTIMS: Mutex<Vec<Victim>> = Mutex::new(Vec::new());
#[cfg(test)]
pub(crate) fn take_shoved_victims() -> Vec<Victim> {
    SHOVED_VICTIMS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}
pub(crate) fn shove_victim(victim: Victim) {
    if let Ok(mut v) = SHOVED_VICTIMS.lock() {
        v.push(victim);
    }
}
/// AI specials currently holding a survivor (or tonguing one), refreshed
/// every tick: survivor bots treat them as known threats and shoot them first.
static HOLDERS: Mutex<Vec<Entity>> = Mutex::new(Vec::new());
pub(crate) fn holders() -> Vec<Entity> {
    HOLDERS.lock().map(|h| h.clone()).unwrap_or_default()
}
/// Pound counter per bot held by an AI Charger: a new value restarts the
/// victim's pounded clip with the Charger's pound (retail restarts the
/// victim with each pound; the survivor clips run 1.667 s against some
/// 1.6 s pounds, so free-running loops drift apart).
static CHARGER_POUNDS: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());
pub(crate) fn charger_pound_serial(index: usize) -> Option<u32> {
    CHARGER_POUNDS
        .lock()
        .ok()?
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, s)| *s)
}
/// Pose of a bot held by an AI Charger (consulted by `charger::victim_pose`).
pub(crate) fn charger_pose(index: usize) -> Option<super::charger::VictimPose> {
    CHARGER_POSES
        .lock()
        .ok()?
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, p)| *p)
}

pub fn install(app: &mut App) {
    // `--sb-stop`: start with retail `sb_stop 1` (survivor bots frozen), for
    // watching a special's full attack in playtests and captures.
    if std::env::args().any(|a| a == "--sb-stop") {
        crate::game_state::set_bots_stopped(true);
    }
    if let Some(class) = std::env::args()
        .skip_while(|a| a != "--capture-ai-special")
        .nth(1)
        .and_then(|n| SpecialClass::parse(&n))
    {
        if let Ok(mut r) = REQUESTS.lock() {
            r.push(class);
        }
    }
    app.add_systems(
        FixedUpdate,
        (spawn_requests, tick)
            .chain()
            .after(super::tick)
            .run_if(crate::game_loading::visible),
    )
    .add_systems(
        Update,
        (draw_tongues, player_pin_body, pin_crosshair).after(crate::game::controls),
    )
    .add_systems(
        RunFixedMainLoop,
        restore_poses.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
    )
    .add_systems(FixedFirst, record_previous_poses)
    .add_systems(FixedLast, record_poses)
    .add_systems(
        RunFixedMainLoop,
        show_poses.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
    );
}

/// Render interpolation of an AI special's root between 30 Hz ticks (the
/// camera is interpolated the same way). Without it a fast actor such as
/// the Tank visibly steps every tick at high frame rates.
#[derive(Component, Default)]
pub(crate) struct SmoothPose {
    previous: Option<Transform>,
    current: Option<Transform>,
    shown: Option<Transform>,
}
fn restore_poses(mut actors: Query<(&mut Transform, &mut SmoothPose)>) {
    for (mut transform, mut pose) in &mut actors {
        match (pose.shown, pose.current) {
            // Untouched since shown: put the simulated pose back.
            (Some(shown), Some(current)) if shown == *transform => *transform = current,
            // Moved by a non-fixed system: that is the new simulated pose.
            _ => {
                pose.previous = Some(*transform);
                pose.current = Some(*transform);
            }
        }
        pose.shown = None;
    }
}
fn record_previous_poses(mut actors: Query<(&Transform, &mut SmoothPose)>) {
    for (transform, mut pose) in &mut actors {
        pose.previous = Some(*transform);
    }
}
fn record_poses(mut actors: Query<(&Transform, &mut SmoothPose)>) {
    for (transform, mut pose) in &mut actors {
        pose.current = Some(*transform);
    }
}
fn show_poses(fixed: Res<Time<Fixed>>, mut actors: Query<(&mut Transform, &mut SmoothPose)>) {
    let alpha = fixed.overstep_fraction().clamp(0., 1.);
    for (mut transform, mut pose) in &mut actors {
        let (Some(previous), Some(current)) = (pose.previous, pose.current) else {
            continue;
        };
        if *transform != current || previous.translation.distance_squared(current.translation) > 1. {
            // Changed outside the fixed loop, or teleported.
            continue;
        }
        let shown = Transform {
            translation: previous.translation.lerp(current.translation, alpha),
            rotation: previous.rotation.slerp(current.rotation, alpha),
            scale: current.scale,
        };
        *transform = shown;
        pose.shown = Some(shown);
    }
}

/// Spawns one AI special at `position`, facing `toward`.
pub(crate) fn spawn(
    commands: &mut Commands,
    models: &crate::viewer::RenderModels,
    class: SpecialClass,
    position: Vec3,
    toward: Vec3,
) -> Option<Entity> {
    let model = class.model();
    let Some(draws) = models.0.get(&(model.to_owned(), 0, 0)) else {
        eprintln!("AI special: {model} is not loaded");
        return None;
    };
    let face = (toward - position) * Vec3::new(1., 0., 1.);
    let rotation = Quat::from_rotation_y((-face.z).atan2(face.x));
    let brain = AiSpecial::new(class);
    let activity = AiActivity(brain.activity(), brain.gesture());
    let actor = commands
        .spawn((
            Transform::from_translation(position).with_rotation(rotation),
            Visibility::default(),
            Enemy {
                model: model.into(),
                death_loot: None,
                loot_dropped: false,
                reaction_serial: 0,
                attack_serial: 0,
                health: health(class),
                attack: 0.,
                vertical: 0.,
                reaction: 0,
                corpse_time: 0.,
                death: Default::default(),
                movement_speed: 0.,
                burning: 0.,
            },
            brain,
            activity,
            AiFlinch::default(),
            SmoothPose::default(),
        ))
        .id();
    for (index, (mesh, material)) in draws.iter().enumerate() {
        // smoker.mdl draw 1 is the sideways extended tongue (see smoker.rs).
        if class == SpecialClass::Smoker && index == 1 {
            continue;
        }
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            crate::viewer::DynamicModelLighting::default(),
            Transform::default(),
            RenderLayers::layer(1),
            ChildOf(actor),
        ));
    }
    println!("AI special spawned: {} at {position:?}", class.name());
    Some(actor)
}
fn spawn_requests(
    mut commands: Commands,
    collision: Res<Collision>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    mut squad: ResMut<Squad>,
    mut state: ResMut<Match>,
    mut incap_capture: Local<bool>,
    mut actors: Query<(&crate::game::CompanionActor, &mut Transform)>,
) {
    // Opt-in visual regression: kill the three staged survivor bots from
    // standing without altering normal gameplay or Director spawning.
    if !*incap_capture
        && state.elapsed > 0.5
        && std::env::args().any(|a| a == "--capture-dead-bots")
    {
        *incap_capture = true;
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let right = Quat::from_rotation_y(body.yaw) * Vec3::X;
        for (i, m) in squad.0.iter_mut().enumerate() {
            m.position = collision.snap_ground(
                body.feet + forward * 4. + right * (i as f32 - 1.) * 1.8,
                HEIGHT,
            );
            m.state.dead = true;
            m.state.incap = false;
        }
        for (actor, mut transform) in &mut actors {
            if let Some(m) = squad.0.get(actor.0) {
                transform.translation = m.position;
            }
        }
        println!("Capture: survivor bots killed from standing");
    }
    // `--capture-bots-ahead`: stage the bots just past the z_spawn point so an
    // AI special targets one of them (pinned-teammate glow captures).
    if !*incap_capture
        && std::env::args().any(|a| a == "--capture-bots-ahead")
    {
        *incap_capture = true;
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let right = Quat::from_rotation_y(body.yaw) * Vec3::X;
        // Just past where `z_spawn` places a special, so a bot is nearest.
        let spot = super::witch::placement(&collision.for_common(), &body).unwrap_or(body.feet + forward * 4.);
        for (i, m) in squad.0.iter_mut().enumerate() {
            m.position = collision.snap_ground(
                spot + forward * 1.2 + right * (i as f32 - 1.) * 1.4,
                HEIGHT,
            );
            m.cooldown = 10.;
        }
        for (actor, mut transform) in &mut actors {
            if let Some(m) = squad.0.get(actor.0) {
                transform.translation = m.position;
            }
        }
    }
    // `--capture-incap-bot`: down survivor bot 0 once play starts (last-stand
    // animation captures; nearby bots then revive it).
    if !*incap_capture
        && state.elapsed > 0.5
        && std::env::args().any(|a| a == "--capture-incap-bot")
    {
        *incap_capture = true;
        if let Some(m) = squad.0.first_mut() {
            m.state.damage(m.state.health + 1.);
            println!("Capture: survivor bot 0 incapacitated ({})", m.state.incap);
        }
    }
    // `--capture-incap-player`: down the survivor player (HUD captures).
    if !state.incap
        && !state.dead
        && state.elapsed > 0.5
        && state.elapsed < 0.6
        && std::env::args().any(|a| a == "--capture-incap-player")
    {
        let health = state.health;
        state.damage(health + 1.);
        println!("Capture: survivor player incapacitated ({})", state.incap);
    }
    for class in take_requests() {
        let c = collision.for_common();
        // Opt-in capture staging: 15 m straight ahead (Tank rock throws).
        let far = std::env::args().any(|a| a == "--capture-ai-far").then(|| {
            let ahead = body.feet + Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z * 15.;
            c.snap_ground(ahead + Vec3::Y * 0.5, HEIGHT)
        }).filter(|p| c.clear_hull(*p, HEIGHT));
        let Some(position) = far.or_else(|| super::witch::placement(&c, &body)) else {
            eprintln!(
                "z_spawn {}: no supported ground in front of the player",
                class.name()
            );
            continue;
        };
        spawn(&mut commands, &models, class, position, body.feet);
    }
}

/// Live AI specials per `SpecialClass::MINIONS` slot, for the Director.
pub(crate) fn live_counts<'a>(
    specials: impl Iterator<Item = (&'a AiSpecial, &'a Enemy)>,
) -> [usize; 6] {
    let mut counts = [0; 6];
    for (ai, enemy) in specials {
        if enemy.health <= 0. {
            continue;
        }
        if let Some(i) = SpecialClass::MINIONS.iter().position(|c| *c == ai.class) {
            counts[i] += 1;
        }
    }
    counts
}

// --- Survivors ---------------------------------------------------------------
#[derive(Clone, Copy)]
struct Survivor {
    who: Victim,
    feet: Vec3,
    height: f32,
    incap: bool,
    pinned: bool,
}
impl Survivor {
    fn chest(&self) -> Vec3 {
        self.feet + Vec3::Y * self.height * 0.6
    }
}
fn survivors(state: &Match, body: &Body, squad: &Squad) -> Vec<Survivor> {
    let mut out = vec![];
    if !state.infected && !state.dead {
        out.push(Survivor {
            who: Victim::Player,
            feet: body.feet,
            height: if state.incap {
                crate::collision::DUCK_HEIGHT
            } else {
                HEIGHT
            },
            incap: state.incap,
            pinned: state.pinned,
        });
    }
    for (i, m) in squad.0.iter().enumerate() {
        if m.state.dead {
            continue;
        }
        out.push(Survivor {
            who: Victim::Bot(i),
            feet: m.position,
            height: if m.state.incap {
                crate::collision::DUCK_HEIGHT
            } else {
                HEIGHT
            },
            incap: m.state.incap,
            pinned: m.state.pinned,
        });
    }
    out
}
fn find(list: &[Survivor], who: Victim) -> Option<Survivor> {
    list.iter().copied().find(|s| s.who == who)
}
fn damage(state: &mut Match, squad: &mut Squad, who: Victim, amount: f32) {
    match who {
        Victim::Player => state.damage(amount),
        Victim::Bot(i) => {
            if let Some(m) = squad.0.get_mut(i) {
                m.state.damage(amount);
            }
        }
    }
}
/// The local player is being pummelled (not carried) by an AI Charger, for
/// the survivor music (crate::survivor_music).
static PLAYER_PUMMELED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn player_pummeled() -> bool {
    PLAYER_PUMMELED.load(std::sync::atomic::Ordering::Relaxed)
}
fn pin(state: &mut Match, squad: &mut Squad, who: Victim, class: SpecialClass, can_fire: bool) {
    match who {
        Victim::Player => {
            state.pinned = true;
            state.pinned_by = Some(class);
            state.pin_allows_fire = can_fire;
        }
        Victim::Bot(i) => super::pin_member(squad, i, class),
    }
}
fn unpin(state: &mut Match, squad: &mut Squad, who: Victim) {
    match who {
        Victim::Player => {
            state.pinned = false;
            state.pinned_by = None;
            state.pin_allows_fire = false;
            PLAYER_PUMMELED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        Victim::Bot(i) => {
            if let Some(m) = squad.0.get_mut(i) {
                m.state.pinned = false;
                m.state.pinned_by = None;
            }
        }
    }
}
fn move_victim(body: &mut Body, squad: &mut Squad, who: Victim, to: Vec3) {
    match who {
        Victim::Player => {
            body.feet = to;
            body.velocity = Vec3::ZERO;
        }
        Victim::Bot(i) => {
            if let Some(m) = squad.0.get_mut(i) {
                m.position = to;
            }
        }
    }
}
fn bile(state: &mut Match, squad: &mut Squad, who: Victim) -> bool {
    match who {
        Victim::Player => {
            let fresh = state.vomited <= 0.;
            state.vomited = super::boomer::IT_DURATION;
            fresh
        }
        Victim::Bot(i) => squad.0.get_mut(i).is_some_and(super::boomer::bile),
    }
}
/// AI Tank punch landing on a survivor: retail co-op damage table, then a
/// fling at z_tank_throw_force (no fling for an incapacitated survivor).
#[allow(clippy::too_many_arguments)]
fn tank_punch(
    collision: &Collision,
    state: &mut Match,
    body: &mut Body,
    squad: &mut Squad,
    queue: &mut crate::game_audio::Queue,
    knocks: &mut Vec<Knock>,
    s: &Survivor,
    tank_feet: Vec3,
    forward: Vec3,
    difficulty: crate::campaign_settings::Difficulty,
) {
    damage(state, squad, s.who, super::tank::punch_damage(false, difficulty_index(difficulty), s.incap));
    let now_down = match s.who {
        Victim::Player => state.incap || state.dead,
        Victim::Bot(i) => squad.0.get(i).is_none_or(|m| m.state.incap || m.state.dead),
    };
    if s.incap || now_down {
        queue.play("HulkZombie.PunchIncap", Some(s.feet));
        return;
    }
    queue.play("Claw.HitFlesh", Some(s.feet));
    queue.play("HulkZombie.Punch", Some(s.feet));
    let velocity = super::tank::fling_velocity(tank_feet, s.feet, forward);
    let lifted = s.feet + Vec3::Y * super::tank::FLING_LIFT * UNIT;
    if collision.clear_hull(lifted, HEIGHT) {
        move_victim(body, squad, s.who, lifted);
    }
    if let Some(Victim::Bot(i)) = Some(s.who) {
        if let Some(m) = squad.0.get_mut(i) {
            m.vertical = velocity.y;
            m.aim = Vec3::new(tank_feet.x - m.position.x, 0., tank_feet.z - m.position.z).normalize_or(Vec3::X);
        }
    }
    pin(state, squad, s.who, SpecialClass::Tank, false);
    knocks.retain(|k| k.who != s.who);
    knocks.push(Knock {
        who: s.who,
        velocity,
        airborne: true,
        getup: super::tank::GETUP,
        flight: 0.,
        facing: facing(s.feet, tank_feet),
        by: SpecialClass::Tank,
    });
}
/// Flung survivors fly under gravity, land, then get up after
/// GetUpFrom_TankPunch/TankRock (1.5 s) and are released.
fn update_knocks(
    collision: &Collision,
    knocks: &mut Vec<Knock>,
    state: &mut Match,
    body: &mut Body,
    squad: &mut Squad,
    dt: f32,
) {
    let c = collision.for_actor(false, false);
    knocks.retain_mut(|k| {
        let (position, dead) = match k.who {
            Victim::Player => (body.feet, state.dead || state.infected),
            Victim::Bot(i) => match squad.0.get(i) {
                Some(m) => (m.position, m.state.dead),
                None => return false,
            },
        };
        if dead {
            return false;
        }
        if k.airborne {
            k.flight += dt;
            k.velocity.y -= crate::player_movement::GRAVITY * dt;
            let (next, contact) = c.walk_velocity(position, k.velocity, dt, HEIGHT, false);
            let ceiling = k.velocity.y > 0. && contact.y < k.velocity.y * 0.5;
            k.velocity = Vec3::new(contact.x, if ceiling { 0. } else { k.velocity.y }, contact.z);
            let landed = k.velocity.y <= 0. && c.grounded(next, HEIGHT) || k.flight > 6.;
            let next = if landed { c.snap_ground(next, HEIGHT) } else { next };
            move_victim(body, squad, k.who, next);
            if let Victim::Bot(i) = k.who {
                if let Some(m) = squad.0.get_mut(i) {
                    // Non-zero `vertical` marks the airborne pose for animation.
                    m.vertical = if landed { 0. } else if k.velocity.y == 0. { -1e-4 } else { k.velocity.y };
                }
            }
            if landed {
                k.airborne = false;
                k.velocity = Vec3::ZERO;
            }
            return true;
        }
        k.getup -= dt;
        if k.getup > 0. {
            return true;
        }
        let held_by_knocker = match k.who {
            Victim::Player => state.pinned_by == Some(k.by),
            Victim::Bot(i) => squad.0.get(i).is_some_and(|m| m.state.pinned_by == Some(k.by)),
        };
        if held_by_knocker {
            unpin(state, squad, k.who);
        }
        false
    });
}
/// AI Tank rocks: CTankRock ProximityThink against every survivor (player
/// and bots) within z_tank_rock_radius of the swept path with line of
/// sight, otherwise a world hit with the growing hull.
#[allow(clippy::too_many_arguments)]
fn update_tank_rocks(
    collision: &Collision,
    rocks: &mut super::tank::AiRocks,
    state: &mut Match,
    body: &mut Body,
    squad: &mut Squad,
    queue: &mut crate::game_audio::Queue,
    knocks: &mut Vec<Knock>,
    difficulty: crate::campaign_settings::Difficulty,
    dt: f32,
) {
    let c = collision.for_actor(true, false);
    let list = survivors(state, body, squad);
    let mut hits: Vec<(Survivor, Vec3)> = vec![];
    // Bullets stop at the nearest rock in their path.
    for (origin, direction, reach, damage) in super::tank::take_rock_shots() {
        let struck = rocks
            .rocks
            .iter_mut()
            .filter_map(|rock| {
                let radius = super::tank::rock_half_extent(rock.age);
                let to = rock.position - origin;
                let along = to.dot(direction);
                (along > 0. && along <= reach && (to - direction * along).length() <= radius)
                    .then_some((along, rock))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, rock)) = struck {
            rock.health -= damage;
        }
    }
    rocks.rocks.retain(|rock| {
        if rock.health > 0. {
            return true;
        }
        queue.play("HulkZombie.ThrownProjectileHit", Some(rock.position));
        println!("AI Tank rock shot down at {:?}", rock.position);
        super::death_effects::rock_impact(rock.position);
        false
    });
    rocks.rocks.retain_mut(|rock| {
        let next = rock.advance(dt);
        let victim = list
            .iter()
            .filter(|s| !s.incap && !hits.iter().any(|(h, _)| h.who == s.who))
            .filter_map(|s| {
                let center = s.feet + Vec3::Y * s.height * 0.5;
                let near = super::tank::closest_point(center, rock.position, next);
                (near.distance(center) < super::tank::ROCK_RADIUS * UNIT
                    && c.trace(near, center, Vec3::ZERO).fraction >= 1.)
                    .then_some((*s, near.distance(center), near))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((s, _, near)) = victim {
            queue.play("HulkZombie.ThrownProjectileHit", Some(near));
            super::death_effects::rock_impact(near);
            hits.push((s, rock.position));
            return false;
        }
        let half = Vec3::splat(super::tank::rock_half_extent(rock.age));
        let hit = c.trace(rock.position, next, half);
        if hit.fraction < 1. && !hit.start_solid || rock.age > 10. {
            rock.position = rock.position.lerp(next, hit.fraction);
            queue.play("HulkZombie.ThrownProjectileHit", Some(rock.position));
            super::death_effects::rock_impact(rock.position);
            return false;
        }
        rock.position = next;
        true
    });
    for (s, rock) in hits {
        println!("AI Tank rock hit {:?}", s.who);
        damage(state, squad, s.who, super::tank::punch_damage(false, difficulty_index(difficulty), false));
        let down = match s.who {
            Victim::Player => state.incap || state.dead,
            Victim::Bot(i) => squad.0.get(i).is_none_or(|m| m.state.incap || m.state.dead),
        };
        if down {
            continue;
        }
        // Knockdown in place with a getup (FUN_10316eb0(2)).
        pin(state, squad, s.who, SpecialClass::Tank, false);
        if let Victim::Bot(i) = s.who {
            if let Some(m) = squad.0.get_mut(i) {
                m.vertical = 0.;
            }
        }
        knocks.retain(|k| k.who != s.who);
        knocks.push(Knock {
            who: s.who,
            velocity: Vec3::ZERO,
            airborne: false,
            getup: super::tank::GETUP,
            flight: 0.,
            facing: facing(s.feet, rock),
            by: SpecialClass::Tank,
        });
    }
}
fn clear(c: &Collision, from: Vec3, to: Vec3) -> bool {
    c.trace(from, to, Vec3::ZERO).fraction >= 0.99
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut queue: ResMut<crate::game_audio::Queue>,
    mut specials: Query<(
        Entity,
        &mut Transform,
        &mut Enemy,
        &mut AiSpecial,
        &mut AiActivity,
        Option<&mut AiFlinch>,
    )>,
    mut crowd: Local<crate::infected_movement::Crowd>,
    world: (
        Commands,
        Option<ResMut<crate::l4d2r_navigation::Navigation>>,
        Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
        Option<ResMut<crate::source_director::Director>>,
        Option<ResMut<super::spitter::Spit>>,
        Query<(&crate::game::CompanionActor, &mut Transform), Without<AiSpecial>>,
        (
            Option<Res<crate::campaign_settings::SessionSettings>>,
            Option<ResMut<super::tank::AiRocks>>,
            Local<Vec<Knock>>,
        ),
    ),
) {
    let (mut commands, mut navigation, mut logic, mut director, mut spit, mut actors, tank_state) = world;
    let (session, rocks, mut knocks) = tank_state;
    let mut no_rocks = super::tank::AiRocks::default();
    let mut rocks = rocks;
    let rocks: &mut super::tank::AiRocks = match rocks.as_mut() {
        Some(r) => r,
        None => &mut no_rocks,
    };
    let difficulty = session
        .as_ref()
        .map_or(crate::campaign_settings::Difficulty::Normal, |s| s.difficulty);
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    state.vomited = (state.vomited - dt).max(0.);
    let c = collision.for_common();
    let mut charger_poses = vec![];
    let mut pound_serials = vec![];
    // Climb/obstruction bookkeeping shared with the commons' movement code.
    crowd.rebuild(
        specials
            .iter()
            .filter(|(_, _, e, _, _, _)| e.health > 0.)
            .map(|(id, t, _, _, _, _)| (id.to_bits(), t.translation)),
        dt,
    );
    // AI Tank rocks in flight and survivors it flung or knocked down.
    rocks.held.clear();
    update_tank_rocks(&collision, rocks, &mut state, &mut body, &mut squad, &mut queue, &mut knocks, difficulty, dt);
    // Crushed by a Tank-hit prop (prop_impact): flung or staggered.
    for (index, r) in crate::prop_impact::take_reactions() {
        let (who, feet, down) = if index == 0 {
            (Victim::Player, body.feet, state.incap || state.dead || state.infected)
        } else {
            let Some(m) = squad.0.get(index - 1) else { continue };
            (Victim::Bot(index - 1), m.position, m.state.incap || m.state.dead)
        };
        if down {
            continue;
        }
        match r {
            crate::prop_impact::Reaction::Fling { from, velocity } => {
                let velocity = super::tank::fling_velocity(from, feet, velocity.normalize_or_zero());
                if let Victim::Bot(i) = who {
                    if let Some(m) = squad.0.get_mut(i) {
                        m.vertical = velocity.y;
                    }
                }
                pin(&mut state, &mut squad, who, SpecialClass::Tank, false);
                knocks.retain(|k| k.who != who);
                knocks.push(Knock {
                    who,
                    velocity,
                    airborne: true,
                    getup: super::tank::GETUP,
                    flight: 0.,
                    facing: facing(feet, from),
                    by: SpecialClass::Tank,
                });
            }
            crate::prop_impact::Reaction::Stagger { away } => {
                let st = if index == 0 { Some(&mut *state) } else { squad.0.get_mut(index - 1).map(|m| &mut m.state) };
                if let Some(st) = st {
                    st.stagger = super::boomer::STAGGER_DURATION;
                    st.stagger_dir = away.normalize_or_zero();
                }
            }
        }
    }
    update_knocks(&collision, &mut knocks, &mut state, &mut body, &mut squad, dt);
    // Victim facing for this tick: bots by squad index, and the player.
    let mut victim_rotations: Vec<(Victim, Quat, &'static str)> = vec![];
    let shoved: Vec<Victim> = SHOVED_VICTIMS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
    let mut holding_now = vec![];
    for (entity, mut transform, mut enemy, mut ai, mut activity, flinch_out) in &mut specials {
        let feet = transform.translation;
        let list = survivors(&state, &body, &squad);
        // --- Death -------------------------------------------------------
        if enemy.health <= 0. {
            if !ai.dead {
                ai.dead = true;
                println!(
                    "AI {} {:.2}s -> dead at {:?}",
                    ai.class.name(),
                    state.elapsed,
                    feet
                );
                // Retail: the killer speaks KilledZombie
                // ("SubjectIsA:%s,Distance:%f", server 10336010) and the
                // HUD's damage record names the kill (Tanks left out,
                // hud_deathnotice_threats 0).
                if let Some(who) = crate::game_ui::kill_feed::killer(entity) {
                    let victim = match ai.class {
                        SpecialClass::Hunter => "Hunter",
                        SpecialClass::Smoker => "Smoker",
                        SpecialClass::Boomer => "Boomer",
                        SpecialClass::Spitter => "Spitter",
                        SpecialClass::Jockey => "Jockey",
                        SpecialClass::Charger => "Charger",
                        SpecialClass::Tank => "Tank",
                    };
                    if ai.class != SpecialClass::Tank {
                        crate::game_ui::kill_feed::killed(who, victim);
                    }
                    let speaker = match who {
                        crate::game_ui::kill_feed::Killer::Player => session
                            .as_ref()
                            .map(|s| (s.survivor.character().to_string(), body.feet)),
                        crate::game_ui::kill_feed::Killer::Bot(i) => squad.0.get(i).map(|m| {
                            (
                                m.model
                                    .trim_start_matches("models/survivors/survivor_")
                                    .trim_end_matches(".mdl")
                                    .to_string(),
                                m.position,
                            )
                        }),
                    };
                    if let Some((talker, at)) = speaker {
                        crate::source_dialogue::request(
                            &talker,
                            &format!("KilledZombie|SubjectIsA:{victim},Distance:{:.1}", at.distance(feet) / UNIT),
                        );
                    }
                }
                if let Some(v) = ai.holding() {
                    unpin(&mut state, &mut squad, v);
                }
                ai.mode = Mode::Chase;
                match ai.class {
                    SpecialClass::Boomer => explode(
                        &collision,
                        feet,
                        &mut state,
                        &body,
                        &mut squad,
                        &mut queue,
                        director.as_deref_mut(),
                    ),
                    SpecialClass::Spitter => {
                        queue.play("SpitterZombie.Death", Some(feet));
                        if let Some(spit) = spit.as_mut() {
                            spit.detonate(&collision, feet + Vec3::Y * u(1.), 2);
                        }
                    }
                    SpecialClass::Tank => queue.play("HulkZombie.Die", Some(feet)),
                    SpecialClass::Smoker => {
                        for sound in super::voice::death(SpecialClass::Smoker) {
                            queue.play(*sound, Some(feet));
                        }
                    }
                    class => queue.play(&format!("{}.Death", voice(class)), Some(feet)),
                }
                if enemy.corpse_time <= 0. {
                    enemy.corpse_time = 12.;
                }
            }
            let start = transform.translation + Vec3::Y * 0.1;
            enemy.vertical -= GRAVITY * UNIT * dt;
            let travel = Vec3::Y * enemy.vertical * dt;
            let hit = c.physics_trace(start, start + travel, Vec3::splat(0.09));
            if !hit.start_solid {
                transform.translation += travel * hit.fraction;
            }
            if hit.fraction < 1. && hit.normal.y > 0.6 {
                enemy.vertical = 0.;
            }
            enemy.corpse_time -= dt;
            if enemy.corpse_time <= 0. {
                commands.entity(entity).despawn();
            }
            continue;
        }
        if ai.class == SpecialClass::Tank && !ai.scaled {
            ai.scaled = true;
            enemy.health = tank_health(difficulty);
            ai.previous_health = enemy.health;
        }
        if ai.class == SpecialClass::Tank {
            // Once lit, a Tank burns until it dies: full health drains over
            // tank_burn_duration (it keeps burning after leaving the fire).
            if enemy.burning > 0. {
                ai.on_fire = true;
            }
            if ai.on_fire {
                enemy.burning = enemy.burning.max(0.5);
                enemy.health -= tank_health(difficulty) / super::tank::burn_duration(difficulty) * dt;
                ai.previous_health = ai.previous_health.min(enemy.health);
            }
            // tank_stuck_time_suicide: a Tank stuck chasing for 10 s while no
            // survivor sees it gives up (retail kills it; the Director may
            // then spawn another).
            if matches!(ai.mode, Mode::Chase) && feet.distance(ai.stuck_anchor) < u(32.) {
                ai.stuck += dt;
            } else {
                ai.stuck = 0.;
                ai.stuck_anchor = feet;
            }
            let seen = list.iter().any(|s| clear(&c, s.feet + Vec3::Y * EYE, feet + Vec3::Y * u(60.)));
            if seen {
                ai.stuck = ai.stuck.min(TANK_STUCK_SUICIDE - 1.);
            }
            if ai.stuck >= TANK_STUCK_SUICIDE {
                println!("AI Tank stuck for {TANK_STUCK_SUICIDE} s out of sight: suicide");
                enemy.health = 0.;
            }
            if enemy.health <= 0. {
                continue;
            }
        }
        // Minions chasing without progress (blocked route, closed safe-room
        // door): measured here, used below to reposition.
        if ai.class != SpecialClass::Tank {
            if matches!(ai.mode, Mode::Chase) && feet.distance(ai.stuck_anchor) < u(32.) {
                ai.stuck += dt;
            } else {
                ai.stuck = 0.;
                ai.stuck_anchor = feet;
            }
        }
        // --- Pain ----------------------------------------------------------
        ai.pain = (ai.pain - dt).max(0.);
        if enemy.health < ai.previous_health && ai.pain <= 0. {
            queue.play(&format!("{}.Pain", voice(ai.class)), Some(feet));
            ai.pain = PAIN_INTERVAL;
        }
        // Hit flinch toward the side the shot came from (the nearest
        // survivor with sight stands in for the shooter).
        if enemy.health < ai.previous_health
            && matches!(ai.mode, Mode::Chase | Mode::Flee(_) | Mode::Crouch(_) | Mode::Spit(_) | Mode::Vomit(_) | Mode::Warmup(_) | Mode::Tongue { .. } | Mode::Reel { .. })
        {
            let eye = feet + Vec3::Y * EYE;
            let shooter = list
                .iter()
                .filter(|s| clear(&c, s.feet + Vec3::Y * EYE, eye))
                .min_by(|a, b| a.feet.distance(feet).total_cmp(&b.feet.distance(feet)));
            if let Some(shooter) = shooter {
                let side = side_of(transform.rotation, feet, shooter.feet);
                if let Some(act) = flinch_activity(ai.class, matches!(ai.mode, Mode::Crouch(_)), side) {
                    ai.flinch = (act, ai.flinch.1.wrapping_add(1));
                }
            }
        }
        ai.previous_health = enemy.health;
        // Shoved by a survivor (player melee or a rescuing bot): the attacker
        // is knocked off its victim and staggers. z_charger_allow_shove 0
        // makes a Charger immune.
        if enemy.reaction_serial != ai.shove_serial {
            ai.shove_serial = enemy.reaction_serial;
            if enemy.reaction == 1 && !matches!(ai.class, SpecialClass::Charger | SpecialClass::Tank) {
                // The shover is the nearest survivor (shove reach 1.3 m).
                let knocked_off = ai.holding().is_some();
                if let Some(shover) = list.iter().min_by(|a, b| a.feet.distance(feet).total_cmp(&b.feet.distance(feet))) {
                    ai.stagger_act = shove_activity(ai.class, knocked_off, side_of(transform.rotation, feet, shover.feet));
                }
                if let Some(v) = ai.holding() {
                    unpin(&mut state, &mut squad, v);
                    println!("AI {} shoved off {:?}", ai.class.name(), v);
                    if ai.class == SpecialClass::Smoker {
                        queue.play("SmokerZombie.TongueRetract", Some(feet));
                        ai.ability = super::smoker::HIT_DELAY;
                    } else if ai.class == SpecialClass::Jockey {
                        // z_leap_interval_post_ride: a dismounted Jockey
                        // cannot leap again for 6 s (10313220).
                        ai.ability = ai.ability.max(super::jockey::POST_RIDE_INTERVAL);
                    } else {
                        ai.ability = ai.ability.max(1.5);
                    }
                }
                if !matches!(ai.mode, Mode::Stagger(_)) {
                    ai.mode = Mode::Stagger(1.);
                }
            }
        }
        ai.ability = (ai.ability - dt).max(0.);
        ai.claw = (ai.claw - dt).max(0.);
        ai.land = (ai.land - dt).max(0.);
        ai.end_tongue = (ai.end_tongue - dt).max(0.);
        ai.clock += dt;
        ai.speed = 0.;
        enemy.movement_speed = 0.;
        let eye = feet + Vec3::Y * EYE;
        // Nearest free survivor; pinned/incapacitated ones are left to the
        // special already on them (retail specials prefer free survivors).
        let target = list
            .iter()
            .filter(|s| !s.pinned)
            .min_by(|a, b| {
                let score = |s: &Survivor| s.feet.distance(feet) + if s.incap { 20. } else { 0. };
                score(a).total_cmp(&score(b))
            })
            .copied();
        let sees = target.is_some_and(|t| clear(&c, eye, t.chest()));
        if sees && !ai.alerted {
            ai.alerted = true;
            queue.play(&alert_sound(ai.class), Some(feet));
        }
        let distance = target.map_or(f32::INFINITY, |t| t.feet.distance(feet) / UNIT);
        let mut chase_to = target.map(|t| t.feet);
        // Pounces, charges and pins own their movement. Keep track of the
        // starting mode so landing from one does not apply gravity twice.
        let ground_mode = matches!(ai.mode, Mode::Chase | Mode::Flee(_) | Mode::Stagger(_) | Mode::Swing { .. });
        // --- Per-mode behaviour ------------------------------------------
        match ai.mode {
            Mode::Chase => {
                if let Some(t) = target.filter(|_| sees) {
                    let grounded = c.grounded(feet, HEIGHT);
                    match ai.class {
                        SpecialClass::Hunter
                            if ai.ability <= 0. && grounded && distance <= POUNCE_READY_RANGE =>
                        {
                            ai.mode = Mode::Crouch(POUNCE_CROUCH_DELAY);
                            queue.play("HunterZombie.Warn", Some(feet));
                        }
                        SpecialClass::Jockey
                            if ai.ability <= 0. && grounded && distance <= LEAP_RANGE && !t.incap =>
                        {
                            // Aimed at head height: a chest aim from the Jockey eye flattens
                            // the 150-unit hop short of z_jockey_leap_range.
                            let direction = (t.feet + Vec3::Y * EYE - eye).normalize_or_zero();
                            ai.velocity = super::jockey::leap_velocity(direction);
                            ai.mode = Mode::Airborne(feet);
                            ai.clock = 0.;
                        }
                        SpecialClass::Smoker if distance <= 750. => {
                            ai.sight += dt;
                            chase_to = None;
                            if ai.ability <= 0. && ai.sight >= TONGUE_DELAY {
                                ai.mode = Mode::Tongue {
                                    victim: t.who,
                                    tip: eye,
                                };
                                ai.clock = 0.;
                                queue.play("SmokerZombie.TongueAttack", Some(feet));
                            }
                        }
                        SpecialClass::Boomer if ai.ability <= 0. && distance <= VOMIT_RANGE => {
                            ai.mode = Mode::Vomit(VOMIT_DURATION);
                            ai.ability = VOMIT_INTERVAL;
                            queue.play("BoomerZombie.Vomit", Some(feet));
                        }
                        SpecialClass::Spitter if ai.ability <= 0. && distance <= SPIT_RANGE => {
                            ai.mode = Mode::Spit(super::spitter::SPIT_LATENCY);
                            ai.clock = 0.;
                            ai.ability = super::spitter::SPIT_INTERVAL;
                            queue.play("SpitterZombie.Spit", Some(feet));
                        }
                        SpecialClass::Charger
                            if ai.ability <= 0.
                                && grounded
                                && distance <= CHARGE_RANGE
                                && distance > 80.
                                && !t.incap =>
                        {
                            ai.mode = Mode::Warmup(CHARGE_WARMUP);
                        }
                        SpecialClass::Tank => {
                            let to = t.feet - feet;
                            let yaw = (-to.x).atan2(-to.z);
                            if ai.claw <= 0.
                                && super::tank::in_punch_reach(feet, yaw, t.feet, t.height)
                            {
                                face_toward(&mut transform, t.feet);
                                ai.mode = Mode::Swing { clock: 0., hit: false, low: t.incap };
                                ai.swing_hits = 0;
                                enemy.attack_serial = enemy.attack_serial.wrapping_add(1);
                                queue.play("HulkZombie.Attack", Some(feet));
                            } else if ai.ability <= 0.
                                && grounded
                                && distance >= TANK_THROW_ALLOW_RANGE
                            {
                                // tank_rock_overhead_percent 100: always an
                                // overhand throw, one of its three sequences
                                // (the player Tank picks the same way).
                                ai.pounds = ai.pounds.wrapping_add(1);
                                let variant = ((entity.to_bits() as u32)
                                    .wrapping_mul(2654435761)
                                    .wrapping_add(ai.pounds.wrapping_mul(40503))
                                    .rotate_left(13)
                                    % super::tank::THROWS.len() as u32) as usize;
                                ai.mode = Mode::Throw { clock: 0., aim: t.chest(), released: false, variant };
                                ai.ability = TANK_THROW_MIN_INTERVAL + super::tank::THROWS[variant].1;
                                queue.play("HulkZombie.Throw.Pickup", Some(feet));
                            }
                        }
                        _ => {}
                    }
                    if ai.class != SpecialClass::Smoker {
                        ai.sight = 0.;
                    }
                } else {
                    ai.sight = 0.;
                    // Out of sight and going nowhere (e.g. outside a closed
                    // safe room): back off to find another way or angle
                    // instead of waiting at the wall forever.
                    if ai.class != SpecialClass::Tank && ai.stuck >= MINION_STUCK_REPOSITION && target.is_some() {
                        ai.stuck = 0.;
                        ai.mode = Mode::Flee(MINION_REPOSITION_TIME);
                    }
                }
            }
            Mode::Swing { clock, hit, low } => {
                let now = clock + dt;
                let window = super::tank::WINDUP..super::tank::WINDUP + super::tank::SWING_DURATION;
                let mut hit = hit;
                if now >= window.start && clock < window.end {
                    let forward = transform.rotation * Vec3::X;
                    let yaw = (-forward.x).atan2(-forward.z);
                    // The fist sweep also hits physics props in front
                    // (tank_swing_physics_prop_force), once per swing.
                    if clock < window.start {
                        if let Some(logic) = logic.as_mut() {
                            let reach = (super::tank::SWING_RANGE + super::tank::FIST_RADIUS) * UNIT + RADIUS;
                            super::tank::punch_prop(logic, eye, forward, reach, &c);
                        }
                    }
                    for (bit, s) in list.iter().enumerate() {
                        if ai.swing_hits & (1 << bit.min(31)) != 0
                            || !super::tank::in_punch_reach(feet, yaw, s.feet, s.height)
                            || !clear(&c, eye, s.chest())
                        {
                            continue;
                        }
                        ai.swing_hits |= 1 << bit.min(31);
                        hit = true;
                        tank_punch(&collision, &mut state, &mut body, &mut squad, &mut queue, &mut knocks, s, feet, forward, difficulty);
                    }
                }
                ai.mode = if now >= window.end + TANK_SWING_RECOVER {
                    ai.claw = if hit { super::tank::SWING_INTERVAL } else { super::tank::MISS_INTERVAL };
                    Mode::Chase
                } else {
                    Mode::Swing { clock: now, hit, low }
                };
            }
            Mode::Throw { clock, aim, released, variant } => {
                chase_to = None;
                let (_, duration, create, release) = super::tank::THROWS[variant];
                let now = clock + dt;
                // Keep turning toward the victim until the rock leaves the hand.
                let aim = if released {
                    aim
                } else {
                    target.filter(|_| sees).map_or(aim, |t| t.chest())
                };
                if !released {
                    face_toward(&mut transform, aim);
                }
                let forward = transform.rotation * Vec3::X;
                let yaw = (-forward.x).atan2(-forward.z);
                let mut released = released;
                if !released && now >= create && now < release {
                    let held = super::tank::model_point(feet, yaw, super::tank::debris_at(variant, now));
                    let id = rocks.serial.wrapping_add(1);
                    rocks.held.push((id, held, Quat::from_rotation_y(yaw)));
                }
                if !released && now >= release {
                    released = true;
                    let hand = super::tank::model_point(feet, yaw, super::tank::debris_at(variant, release));
                    // Retail traces eye -> debris attachment and spawns at the end.
                    let trace = c.trace(eye, hand, Vec3::ZERO);
                    let start = eye.lerp(hand, (trace.fraction - 0.02).max(0.));
                    rocks.serial = rocks.serial.wrapping_add(1);
                    let spin = Vec3::new(60_f32.to_radians(), ((rocks.serial % 241) as f32 - 120.).to_radians(), 0.);
                    let velocity = super::tank::rock_launch(start, aim);
                    let id = rocks.serial;
                    rocks.rocks.push(super::tank::Rock::launched(id, start, velocity, spin));
                    queue.play("HulkZombie.Throw", Some(feet));
                    queue.play("HulkZombie.Throw.FlyLoop", Some(start));
                }
                ai.mode = if now >= duration {
                    Mode::Chase
                } else {
                    Mode::Throw { clock: now, aim, released, variant }
                };
            }
            Mode::Crouch(remaining) => {
                chase_to = None;
                // The crouched Hunter keeps facing its prey.
                if let Some(t) = target.filter(|_| sees) {
                    face_toward(&mut transform, t.feet);
                }
                let remaining = remaining - dt;
                ai.mode = Mode::Crouch(remaining);
                if remaining <= 0. {
                    match target.filter(|_| sees && distance <= POUNCE_GIVE_UP_RANGE) {
                        Some(t) => {
                            ai.velocity = super::launch_velocity(pounce_direction(eye, t.chest()));
                            ai.mode = Mode::Airborne(feet);
                            ai.clock = 0.;
                            queue.play("HunterZombie.Pounce", Some(feet));
                        }
                        None => ai.mode = Mode::Chase,
                    }
                }
            }
            Mode::Airborne(_) => {
                chase_to = None;
                // The lunge/leap body faces along its flight.
                let flight = ai.velocity * Vec3::new(1., 0., 1.);
                if flight.length_squared() > 1e-4 {
                    transform.rotation = Quat::from_rotation_y((-flight.z).atan2(flight.x));
                }
                ai.velocity.y -= GRAVITY * UNIT * dt;
                let to = feet + ai.velocity * dt;
                let hit_survivor = list
                    .iter()
                    // Hunters pounce incapacitated survivors; Jockeys cannot ride them.
                    .filter(|s| !s.pinned && !(s.incap && ai.class == SpecialClass::Jockey))
                    .filter_map(|s| {
                        super::swept_victim(feet, to, s.feet, s.height).map(|f| (s.who, f))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                // Hull above the feet so a low hop does not start inside the floor.
                let wall = c.trace(
                    feet + Vec3::Y * HEIGHT * 0.55,
                    to + Vec3::Y * HEIGHT * 0.55,
                    Vec3::new(RADIUS, HEIGHT * 0.4, RADIUS),
                );
                if let Some((who, _)) = hit_survivor {
                    let at = find(&list, who).map_or(feet, |s| s.feet);
                    let flat = ai.velocity * Vec3::new(1., 0., 1.);
                    ai.pin_yaw = (-flat.x).atan2(-flat.z);
                    transform.translation = at;
                    ai.mode = Mode::Pin(who);
                    ai.clock = 0.;
                    ai.damage_clock = 0.;
                    ai.velocity = Vec3::ZERO;
                    pin(&mut state, &mut squad, who, ai.class, false);
                    if ai.class == SpecialClass::Jockey {
                        queue.play("JockeyZombie.Ride", Some(at));
                    } else {
                        queue.play("HunterZombie.Pounce.shred", Some(at));
                    }
                } else {
                    if wall.fraction < 1. {
                        // A wall ends the horizontal flight; the special drops.
                        transform.translation = feet + (to - feet) * (wall.fraction - 0.01).max(0.);
                        ai.velocity.x = 0.;
                        ai.velocity.z = 0.;
                    } else {
                        let ground = c.trace(feet, to, Vec3::new(RADIUS, 0.01, RADIUS));
                        transform.translation = if ground.fraction < 1. && ground.normal.y > 0.7 {
                            feet + (to - feet) * (ground.fraction - 0.01).max(0.)
                        } else {
                            to
                        };
                    }
                    let landed = ai.clock > 0.1
                        && ai.velocity.y <= 0.
                        && c.grounded(transform.translation, HEIGHT);
                    if landed || ai.clock > 3. {
                        ai.mode = Mode::Chase;
                        ai.velocity = Vec3::ZERO;
                        if ai.class == SpecialClass::Hunter {
                            ai.land = HUNTER_LAND_HARD;
                        }
                        ai.ability = if ai.class == SpecialClass::Jockey {
                            LEAP_AGAIN
                        } else {
                            1.
                        };
                    }
                }
            }
            Mode::Pin(who) => {
                chase_to = None;
                let Some(victim) = find(&list, who) else {
                    unpin(&mut state, &mut squad, who);
                    ai.mode = Mode::Chase;
                    ai.ability = 1.;
                    continue;
                };
                ai.damage_clock += dt;
                match ai.class {
                    SpecialClass::Hunter => {
                        transform.translation = victim.feet;
                        transform.rotation = super::pin_rotation(ai.pin_yaw, true);
                        let fall = if ai.clock < 1.2 {
                            "ACT_TERROR_INCAP_FROM_POUNCE"
                        } else {
                            "ACT_IDLE_POUNCED"
                        };
                        victim_rotations.push((who, super::pin_rotation(ai.pin_yaw, false), fall));
                        while ai.damage_clock >= POUNCE_DAMAGE_INTERVAL {
                            ai.damage_clock -= POUNCE_DAMAGE_INTERVAL;
                            damage(&mut state, &mut squad, who, POUNCE_DAMAGE);
                        }
                    }
                    SpecialClass::Jockey => {
                        // Retail jockeys leave incapacitated survivors.
                        if victim.incap {
                            unpin(&mut state, &mut squad, who);
                            ai.mode = Mode::Chase;
                            ai.ability = super::jockey::POST_INCAP_INTERVAL;
                            continue;
                        }
                        if ai.clock >= super::jockey::RIDE_DAMAGE_DELAY
                            && ai.damage_clock >= super::jockey::RIDE_DAMAGE_INTERVAL
                        {
                            ai.damage_clock = 0.;
                            damage(&mut state, &mut squad, who, super::jockey::RIDE_DAMAGE);
                        }
                        // Steer the survivor away from the rest of the team.
                        let others: Vec<_> = list
                            .iter()
                            .filter(|s| s.who != who)
                            .map(|s| s.feet)
                            .collect();
                        let away = if others.is_empty() {
                            Quat::from_rotation_y(ai.clock * 0.7) * Vec3::NEG_Z
                        } else {
                            victim.feet - others.iter().sum::<Vec3>() / others.len() as f32
                        };
                        let health_fraction = match who {
                            Victim::Player => state.health / 100.,
                            Victim::Bot(i) => squad.0[i].state.health / 100.,
                        };
                        let control = super::jockey::control(health_fraction, ai.clock * 3.);
                        let (wish, speed) =
                            super::jockey::ride_wish(away, Vec3::ZERO, control, health_fraction);
                        let next = c.walk(
                            victim.feet,
                            wish * u(speed) * dt,
                            HEIGHT,
                            c.grounded(victim.feet, HEIGHT),
                        );
                        move_victim(&mut body, &mut squad, who, next);
                        transform.translation = next;
                        if wish.length_squared() > 0. {
                            transform.rotation = Quat::from_rotation_y((-wish.z).atan2(wish.x));
                        }
                        victim_rotations.push((
                            who,
                            transform.rotation,
                            "ACT_TERROR_JOCKEY_RIDDEN",
                        ));
                    }
                    _ => {
                        // Charger pummel.
                        if who == Victim::Player {
                            PLAYER_PUMMELED.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        // Retail roots the pounded survivor on the Charger's
                        // origin; facing the Charger (below) puts the
                        // authored body under its right fist.
                        move_victim(
                            &mut body,
                            &mut squad,
                            who,
                            victim.feet.lerp(feet, (dt * 8.).min(1.)),
                        );
                        use super::charger::VictimPose;
                        let slam = super::charger::slam_delay(ai.slam_wall);
                        let victim_model = match who {
                            Victim::Player => session
                                .as_ref()
                                .map_or(String::new(), |s| s.survivor.character().to_string()),
                            Victim::Bot(i) => squad.0.get(i).map_or(String::new(), |m| m.model.clone()),
                        };
                        // Traces from the Charger (10342280): a ceiling 148-158
                        // units up allows POUND_UP, a wall 50-62 ahead POUND_NORTH.
                        let span = |from: Vec3, dir: Vec3, (lo, hi): (f32, f32)| {
                            let t = c.trace(from, from + dir * u(hi), Vec3::ZERO);
                            t.fraction < 1. && t.fraction * hi >= lo
                        };
                        let up = span(feet + Vec3::Y * 0.05, Vec3::Y, super::charger::POUND_UP_SPAN);
                        let north = span(
                            feet + Vec3::Y * u(32.),
                            transform.rotation * Vec3::X,
                            super::charger::POUND_NORTH_SPAN,
                        );
                        let choose_pound = |ai: &mut AiSpecial, current: &str| {
                            ai.pounds = ai.pounds.wrapping_add(1);
                            let seed = (entity.to_bits() as u32)
                                .wrapping_mul(2654435761)
                                .wrapping_add(ai.pounds.wrapping_mul(40503))
                                .rotate_left(13);
                            let rolls = (1 + seed % 100, seed / 100);
                            super::charger::next_pound(current, &victim_model, up, north, rolls)
                        };
                        let (pose, act) = if ai.clock < slam {
                            if ai.slam_wall {
                                (VictimPose::SlammedWall, "ACT_TERROR_SLAMMED_WALL")
                            } else {
                                (VictimPose::SlammedGround, "ACT_TERROR_SLAMMED_GROUND")
                            }
                        } else if ai.pound.is_empty() {
                            // Pummel intro (Charger_pummel) until cycle 0.99.
                            let t = ai.clock - slam;
                            let sound = super::charger::PUMMEL_INTRO_SOUND * super::charger::PUMMEL_INTRO;
                            if t - dt < sound && t >= sound {
                                queue.play("ChargerZombie.Pummel", Some(feet));
                            }
                            if t >= super::charger::PUMMEL_INTRO * super::charger::PUMMEL_INTRO_END {
                                ai.pound = choose_pound(&mut ai, "");
                                ai.pound_clock = 0.;
                            }
                            // Retail 1033f7f0: the victim plays PUMMELED for
                            // the whole intro, incapacitated or not.
                            (VictimPose::Pummeled, "ACT_TERROR_CHARGER_PUMMELED")
                        } else {
                            // AE_CHARGER_POUND_VOCALIZE / _IMPACT (10302db0):
                            // the impact deals z_charger_pound_dmg.
                            let (length, _, _) = super::charger::pound_clip(ai.pound);
                            let before = ai.pound_clock;
                            let after = before + dt;
                            let (vocal, impact) = super::charger::pound_events(ai.pound, before, after);
                            if vocal {
                                queue.play("ChargerZombie.VocalizePummel", Some(feet));
                            }
                            if impact {
                                damage(&mut state, &mut squad, who, super::charger::POUND_DAMAGE);
                                queue.play("ChargerZombie.Pummel", Some(feet));
                            }
                            if after >= length {
                                let current = ai.pound;
                                ai.pound = choose_pound(&mut ai, current);
                                ai.pound_clock = after - length;
                            } else {
                                ai.pound_clock = after;
                            }
                            let pose = super::charger::pounded_pose(ai.pound, victim.incap);
                            let act = match pose {
                                VictimPose::PoundedIncap => "ACT_TERROR_CHARGER_POUNDED_INCAP",
                                VictimPose::PoundedNorth => "ACT_TERROR_CHARGER_POUNDED_NORTH",
                                VictimPose::PoundedUp => "ACT_TERROR_CHARGER_POUNDED_UP",
                                _ => "ACT_TERROR_CHARGER_POUNDED_DOWN",
                            };
                            (pose, act)
                        };
                        if let Victim::Bot(i) = who {
                            charger_poses.push((i, pose));
                            if !ai.pound.is_empty() {
                                pound_serials.push((i, ai.pounds));
                            }
                        }
                        victim_rotations.push((
                            who,
                            transform.rotation * Quat::from_rotation_y(std::f32::consts::PI),
                            act,
                        ));
                    }
                }
            }
            Mode::Tongue { victim: who, tip } => {
                chase_to = None;
                let Some(victim) = find(&list, who) else {
                    ai.mode = Mode::Chase;
                    ai.ability = super::smoker::MISS_DELAY;
                    continue;
                };
                let goal = victim.chest();
                let step = u(1000.) * dt;
                let tip = if tip.distance(goal) <= step {
                    goal
                } else {
                    tip + (goal - tip).normalize_or_zero() * step
                };
                let lost = !clear(&c, eye, tip) || tip.distance(eye) > u(750.) * 1.05;
                if lost {
                    ai.mode = Mode::Chase;
                    ai.ability = super::smoker::MISS_DELAY;
                    ai.end_tongue = SMOKER_END_TONGUE;
                    queue.play("SmokerZombie.TongueRetract", Some(feet));
                } else if tip.distance(goal) < u(5.) && victim.pinned {
                    // Another special got there first: the tongue misses.
                    ai.mode = Mode::Chase;
                    ai.ability = super::smoker::MISS_DELAY;
                    ai.end_tongue = SMOKER_END_TONGUE;
                    queue.play("SmokerZombie.TongueRetract", Some(feet));
                } else if tip.distance(goal) < u(5.) {
                    ai.mode = Mode::Reel {
                        victim: who,
                        choking: false,
                    };
                    ai.grab_health = enemy.health;
                    ai.damage_clock = 0.;
                    ai.clock = 0.;
                    ai.bends.clear();
                    pin(&mut state, &mut squad, who, SpecialClass::Smoker, true);
                    super::smoker::set_ai_victim_pose(super::smoker::VictimPose::Pulled);
                    queue.play("SmokerZombie.TongueHit", Some(goal));
                } else {
                    ai.mode = Mode::Tongue { victim: who, tip };
                }
            }
            Mode::Reel {
                victim: who,
                choking,
            } => {
                chase_to = None;
                let Some(victim) = find(&list, who) else {
                    unpin(&mut state, &mut squad, who);
                    ai.mode = Mode::Chase;
                    ai.ability = super::smoker::HIT_DELAY;
                    continue;
                };
                // tongue_break_from_damage_amount; the tongue wraps corners
                // like the player's and breaks when bent too often or blocked.
                let broke = if ai.grab_health - enemy.health >= super::smoker::BREAK_DAMAGE {
                    Some("damage".to_string())
                } else if shoved.contains(&who) {
                    Some("victim shoved".to_string())
                } else {
                    super::smoker::update_bends(&mut ai.bends, &c, eye, victim.chest())
                        .err()
                        .map(|b| format!("{b:?}"))
                };
                if let Some(reason) = broke {
                    println!("AI Smoker tongue broke ({reason}) on {who:?}");
                    unpin(&mut state, &mut squad, who);
                    ai.mode = Mode::Chase;
                    ai.ability = super::smoker::release_delay(choking, victim.incap);
                    ai.end_tongue = SMOKER_END_TONGUE;
                    queue.play("SmokerZombie.TongueRetract", Some(feet));
                    continue;
                }
                let toward = (feet - victim.feet) * Vec3::new(1., 0., 1.);
                let close = toward.length() < u(60.) && ai.bends.is_empty();
                // Dragged toward the nearest bend first.
                let pull = ai.bends.last().map_or(toward, |b| (*b - victim.feet) * Vec3::new(1., 0., 1.));
                if !close {
                    let toward = pull;
                    let speed = (ai.clock * 30. * 4.).min(175.);
                    let next = c.walk(
                        victim.feet,
                        toward.normalize_or_zero() * u(speed) * dt,
                        HEIGHT,
                        c.grounded(victim.feet, HEIGHT),
                    );
                    move_victim(&mut body, &mut squad, who, next);
                }
                ai.damage_clock += dt;
                let choking = choking || close;
                if choking {
                    super::smoker::set_ai_victim_pose(super::smoker::VictimPose::ChokeStanding);
                    if let Victim::Player = who {
                        state.pin_allows_fire = false;
                    }
                }
                let (amount, interval) = if choking {
                    (super::smoker::CHOKE_DAMAGE, super::smoker::DAMAGE_INTERVAL)
                } else {
                    (super::smoker::DRAG_DAMAGE, super::smoker::DAMAGE_INTERVAL)
                };
                if ai.damage_clock >= interval {
                    ai.damage_clock = 0.;
                    damage(&mut state, &mut squad, who, amount);
                }
                ai.mode = Mode::Reel {
                    victim: who,
                    choking,
                };
                let face = (victim.feet - feet) * Vec3::new(1., 0., 1.);
                if face.length_squared() > 1e-4 {
                    transform.rotation = Quat::from_rotation_y((-face.z).atan2(face.x));
                }
                let pose = if choking {
                    super::smoker::VictimPose::ChokeStanding
                } else {
                    super::smoker::VictimPose::Pulled
                };
                victim_rotations.push((
                    who,
                    super::smoker::victim_yaw(pull, pose)
                        .map(Quat::from_rotation_y)
                        .unwrap_or(transform.rotation),
                    pose.activity(),
                ));
            }
            Mode::Vomit(remaining) => {
                chase_to = None;
                let forward = transform.rotation * Vec3::X;
                for s in &list {
                    let to = s.chest() - eye;
                    if to.length() / UNIT <= VOMIT_RANGE
                        && forward.dot(to.normalize_or_zero()) > 0.85
                        && clear(&c, eye, s.chest())
                        && bile(&mut state, &mut squad, s.who)
                    {
                        if let Some(director) = director.as_mut() {
                            director.it_mob(0);
                        }
                        println!("AI Boomer vomited on {:?}", s.who);
                    }
                }
                if let Some(t) = target {
                    face_toward(&mut transform, t.feet);
                }
                let remaining = remaining - dt;
                ai.mode = if remaining <= 0. {
                    Mode::Flee(3.)
                } else {
                    Mode::Vomit(remaining)
                };
            }
            Mode::Spit(remaining) => {
                chase_to = None;
                if let Some(t) = target {
                    face_toward(&mut transform, t.feet);
                }
                let remaining = remaining - dt;
                // The spit leaves after z_spit_latency; the full-body
                // spitter_spitting clip plays on to cycle 0.99 (server
                // 1033f450) before she moves off.
                if remaining <= 0. && remaining + dt > 0. {
                    if let (Some(t), Some(spit)) = (target, spit.as_mut()) {
                        let aim = t.feet + Vec3::Y * u(10.);
                        let flat = (aim - eye) * Vec3::new(1., 0., 1.);
                        // Arc the spit so it lands at range under projectile gravity.
                        let lift = (flat.length() / u(SPIT_RANGE)).clamp(0., 1.) * 0.35;
                        let forward =
                            ((aim - eye).normalize_or_zero() + Vec3::Y * lift).normalize_or_zero();
                        spit.launch(&collision, eye, forward);
                    }
                }
                ai.mode = if ai.clock >= super::spitter::SPIT_ANIMATION * super::spitter::SPIT_ANIMATION_END {
                    Mode::Flee(4.)
                } else {
                    Mode::Spit(remaining)
                };
            }
            Mode::Flee(remaining) => {
                // Keep away from the nearest survivor for a few seconds.
                chase_to = target.map(|t| feet + (feet - t.feet).normalize_or_zero() * 6.);
                let remaining = remaining - dt;
                ai.mode = if remaining <= 0. {
                    Mode::Chase
                } else {
                    Mode::Flee(remaining)
                };
            }
            Mode::Warmup(remaining) => {
                chase_to = None;
                if let Some(t) = target {
                    face_toward(&mut transform, t.feet);
                }
                let remaining = remaining - dt;
                if remaining <= 0. {
                    let direction = target.map_or(transform.rotation * Vec3::X, |t| {
                        ((t.feet - feet) * Vec3::new(1., 0., 1.)).normalize_or_zero()
                    });
                    ai.velocity = direction * u(250.);
                    ai.swing_hits = 0;
                    ai.mode = Mode::Charge {
                        direction,
                        carried: None,
                    };
                    ai.clock = 0.;
                    queue.play("ChargerZombie.Charge", Some(feet));
                } else {
                    ai.mode = Mode::Warmup(remaining);
                }
            }
            Mode::Charge { direction, carried } => {
                chase_to = None;
                let grounded = c.grounded(feet, HEIGHT);
                ai.velocity = super::charger::charge_velocity(ai.velocity, direction, grounded, dt);
                // Charging off a ledge falls (the charge keeps going in the air).
                enemy.vertical = if grounded && enemy.vertical <= 0. {
                    0.
                } else {
                    enemy.vertical - GRAVITY * UNIT * dt
                };
                let delta = ai.velocity * dt;
                let next = c.walk(feet, delta + Vec3::Y * enemy.vertical * dt, HEIGHT, grounded);
                let mut carried = carried;
                for (bit, s) in list.iter().enumerate() {
                    // Like the player Charger: incapacitated and pinned
                    // survivors are passed over, and each survivor is hit at
                    // most once per charge.
                    let mask = 1 << bit.min(31);
                    if Some(s.who) == carried || s.pinned || s.incap || ai.swing_hits & mask != 0 {
                        continue;
                    }
                    if super::charger::touches(feet, next, s.feet) {
                        ai.swing_hits |= mask;
                        if carried.is_none() {
                            carried = Some(s.who);
                            pin(&mut state, &mut squad, s.who, SpecialClass::Charger, false);
                            // Retail 10362ed0: the grab plays Zombie.Punch.
                            queue.play("Zombie.Punch", Some(s.feet));
                        } else {
                            // Knocked aside: z_charge_max_damage, then flung
                            // through the air and a GetUpFrom_Charger.
                            damage(&mut state, &mut squad, s.who, super::charger::IMPACT_DAMAGE);
                            queue.play("ChargerZombie.HitPerson", Some(s.feet));
                            let down = match s.who {
                                Victim::Player => state.incap || state.dead,
                                Victim::Bot(i) => squad.0.get(i).is_none_or(|m| m.state.incap || m.state.dead),
                            };
                            if down {
                                continue;
                            }
                            let lifted = s.feet + Vec3::Y * UNIT;
                            if collision.for_actor(false, false).clear_hull(lifted, HEIGHT) {
                                move_victim(&mut body, &mut squad, s.who, lifted);
                            }
                            pin(&mut state, &mut squad, s.who, SpecialClass::Charger, false);
                            knocks.retain(|k| k.who != s.who);
                            knocks.push(Knock {
                                who: s.who,
                                velocity: super::charger::fling_velocity(ai.velocity, ai.clock),
                                airborne: true,
                                getup: super::charger::GETUP_TIME,
                                flight: 0.,
                                facing: facing(s.feet, feet),
                                by: SpecialClass::Charger,
                            });
                        }
                    }
                }
                transform.translation = next;
                if let Some(v) = carried {
                    move_victim(&mut body, &mut squad, v, next);
                    if let Victim::Bot(i) = v {
                        charger_poses.push((i, super::charger::VictimPose::Carried));
                    }
                    // Carried survivors face the Charger (charger.rs place_actors).
                    victim_rotations.push((
                        v,
                        transform.rotation * Quat::from_rotation_y(std::f32::consts::PI),
                        "ACT_TERROR_CARRIED",
                    ));
                }
                let progress = (next - feet).xz().length();
                // Retail obstacle probe (charger.rs `probe`, FUN_10362630):
                // 24 units ahead while carrying, so the charge stops with
                // the held survivor at the wall rather than the Charger's
                // own hull. Stopping on the hull alone pressed the Charger
                // into the wall and the pummelled survivor (laid out ahead
                // of it) ended up inside the wall.
                let probed = super::charger::probe(&c, next, direction, carried.is_some()).is_some();
                let blocked = probed || (ai.clock > 0.3 && progress < delta.xz().length() * 0.3);
                if blocked || ai.clock >= super::charger::CHARGE_DURATION {
                    queue.play("ChargerZombie.ImpactHard", Some(next));
                    match carried {
                        Some(v) => {
                            damage(&mut state, &mut squad, v, super::charger::SLAM_DAMAGE);
                            ai.mode = Mode::Pin(v);
                            ai.clock = 0.;
                            ai.slam_wall = blocked;
                            ai.pound = "";
                            ai.pound_clock = 0.;
                        }
                        None if blocked => {
                            ai.stagger_act = "ACT_TERROR_SHOVED_BACKWARD";
                            ai.mode = Mode::Stagger(super::charger::STAGGER_TIME);
                        }
                        None => ai.mode = Mode::Chase,
                    }
                    ai.velocity = Vec3::ZERO;
                    ai.ability = super::charger::CHARGE_INTERVAL;
                } else {
                    ai.mode = Mode::Charge { direction, carried };
                }
            }
            Mode::Stagger(remaining) => {
                chase_to = None;
                let remaining = remaining - dt;
                ai.mode = if remaining <= 0. {
                    Mode::Chase
                } else {
                    Mode::Stagger(remaining)
                };
            }
        }
        // --- Claws -----------------------------------------------------------
        if matches!(ai.mode, Mode::Chase | Mode::Flee(_)) && ai.claw <= 0. && ai.class != SpecialClass::Tank {
            if let Some(t) = target.filter(|t| {
                (t.feet - feet).xz().length() <= u(52.) + RADIUS * 2.
                    && (t.feet.y - feet.y).abs() < 1.2
                    && clear(&c, eye, t.chest())
            }) {
                damage(&mut state, &mut squad, t.who, claw_damage(ai.class));
                queue.play("Claw.HitFlesh", Some(t.feet));
                ai.claw = 1.;
                enemy.attack_serial = enemy.attack_serial.wrapping_add(1);
                face_toward(&mut transform, t.feet);
                chase_to = None;
            }
        }
        // --- Locomotion ------------------------------------------------------
        let mobile = matches!(ai.mode, Mode::Chase | Mode::Flee(_) | Mode::Swing { .. });
        let mut moved = false;
        let mut ladder_pose = None;
        if !mobile {
            crowd.cancel_climb(entity.to_bits());
        } else if let Some((position, _, _)) =
            crowd.advance_climb(entity.to_bits(), &c, transform.translation, dt)
        {
            // Climbing over an obstacle like a common infected.
            transform.translation = position;
            enemy.vertical = 0.;
            chase_to = None;
            moved = true;
        } else if let Some((position, _)) = crate::infected_movement::advance_ladder(
            &mut crowd, entity.to_bits(), &c, transform.translation, dt,
        ) {
            // On a NAV ladder (crate::ai_ladder).
            transform.translation = position;
            ladder_pose = crowd.ladders.pose(entity.to_bits());
            if let Some(pose) = ladder_pose {
                transform.rotation = pose.rotation();
            }
            enemy.vertical = 0.;
            chase_to = None;
            moved = true;
        }
        if let Some(goal) = chase_to.filter(|_| mobile) {
            // Only stop next to a target it can actually reach: through a
            // fence or wall it keeps routing round instead of idling forever.
            let close = (goal - feet).xz().length() < u(40.) && sees;
            if !close {
                moved = true;
                let routed = navigation.as_mut().map_or(Vec3::ZERO, |nav| {
                    nav.toward_special(entity.to_bits(), feet, goal, dt)
                });
                if let Some(step) = navigation
                    .as_mut()
                    .and_then(|nav| nav.ladder_infected(entity.to_bits(), feet, goal))
                {
                    crowd.ladders.begin(entity.to_bits(), &c, feet, step);
                }
                // No route (yet): head straight for the goal and let the
                // obstruction steering and climbs below get around things.
                ai.routed = routed.xz().length_squared() > 1e-6;
                let toward = if ai.routed { routed } else { goal - feet };
                let toward = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
                let grounded = c.grounded(feet, HEIGHT);
                enemy.vertical = if grounded {
                    0.
                } else {
                    enemy.vertical - GRAVITY * UNIT * dt
                };
                let next = c.walk(
                    feet,
                    toward * u(run_speed(ai.class)) * dt + Vec3::Y * enemy.vertical * dt,
                    HEIGHT,
                    grounded,
                );
                let mut next = next;
                // Blocked = little progress along the route (a wall slide is
                // not progress), as for commons.
                let progress = ((next - feet) * Vec3::new(1., 0., 1.)).dot(toward);
                let wanted = u(run_speed(ai.class)) * dt;
                if toward != Vec3::ZERO && progress < wanted * 0.35 && grounded {
                    if !crowd.begin_climb(entity.to_bits(), &c, feet, toward) {
                        let pinned = crowd.pinned(entity.to_bits());
                        let direction =
                            crowd.obstruction_direction(entity.to_bits(), &c, feet, toward, HEIGHT, pinned);
                        if direction != Vec3::ZERO {
                            let candidate = c.walk(feet, direction * wanted, HEIGHT, grounded);
                            let step = (candidate - feet) * Vec3::new(1., 0., 1.);
                            let heading = step.normalize_or_zero().dot(toward);
                            if (step.dot(toward) > progress && heading >= 0.5)
                                || (pinned && heading > -0.05 && step.length() > wanted * 0.3)
                            {
                                next = candidate;
                            }
                        }
                    }
                }
                // A breakable door (or other breakable) across the route is
                // clawed down like a player infected does (retail SI bots
                // attack doors; saferoom and rescue doors are not breakable).
                if ai.class != SpecialClass::Tank
                    && toward != Vec3::ZERO
                    && grounded
                    && ai.claw <= 0.
                    && ((next - feet) * Vec3::new(1., 0., 1.)).dot(toward) < wanted * 0.35
                    && logic
                        .as_mut()
                        .is_some_and(|l| l.pound_obstacle(feet, toward, &c))
                {
                    queue.play("Claw.Swing", Some(feet));
                    queue.play("Claw.Hit", Some(feet));
                    ai.claw = 1.;
                    // Clawing a door is progress: no stuck reposition.
                    ai.stuck = 0.;
                    enemy.attack_serial = enemy.attack_serial.wrapping_add(1);
                    let wish = Quat::from_rotation_y((-toward.z).atan2(toward.x));
                    transform.rotation = wish;
                    next = Vec3::new(feet.x, next.y, feet.z);
                }
                // Steering without a nav route (a flee point off the mesh,
                // a route not found yet) never steps off a drop it would not
                // survive; nav routes only take the drops the mesh links.
                if !ai.routed && grounded && !c.grounded(next, HEIGHT) {
                    let below = c.trace(
                        next + Vec3::Y * u(1.),
                        next - Vec3::Y * u(UNROUTED_MAX_DROP),
                        Vec3::new(u(8.), 0., u(8.)),
                    );
                    if below.fraction >= 1. {
                        next = Vec3::new(feet.x, next.y.max(feet.y), feet.z);
                    }
                }
                let along = ((next - feet) * Vec3::new(1., 0., 1.)).dot(toward);
                if ai.routed && crowd.stalled(entity.to_bits(), next, grounded && along < wanted * 0.35, dt) {
                    if let Some(nav) = navigation.as_mut() {
                        nav.reroute_infected(entity.to_bits(), feet, goal);
                    }
                }
                let moved = (next - feet) * Vec3::new(1., 0., 1.);
                transform.translation = next;
                ai.speed = moved.length() / dt.max(1e-3);
                if moved.length_squared() > 1e-6 {
                    // Body yaw turns toward the step at a bounded rate rather
                    // than snapping to every wall-slide and corner step.
                    let wish = Quat::from_rotation_y((-moved.z).atan2(moved.x));
                    transform.rotation = transform.rotation.rotate_towards(wish, turn_rate(ai.class) * dt);
                }
            }
        }
        if !moved && ground_mode
            && (matches!(
                ai.mode,
                Mode::Chase
                    | Mode::Flee(_)
                    | Mode::Stagger(_)
                    | Mode::Swing { .. }
                    | Mode::Warmup(_)
                    | Mode::Spit(_)
                    | Mode::Vomit(_)
                    | Mode::Throw { .. }
            ) || (ai.class == SpecialClass::Charger && matches!(ai.mode, Mode::Pin(_))))
        {
            // No chase target, a nearby target or a stagger stops horizontal
            // intent, but must not suspend the body if its support disappears.
            let (position, vertical) = crate::infected_movement::walk(
                &c, transform.translation, Vec3::ZERO, enemy.vertical, dt,
            );
            transform.translation = position;
            enemy.vertical = vertical;
        }
        if ai.speed > 0.3 {
            ai.still = 0.;
        } else {
            ai.still += dt;
        }
        // Falling (off a ledge, not a lunge or leap): the jump/fall pose.
        ai.falling = matches!(ai.mode, Mode::Chase | Mode::Flee(_)) && enemy.vertical < -u(150.) && ladder_pose.is_none();
        // Boomer::Think (server 10349940): falling with no floor within 75
        // units below (box -10..10 x -10..10 x 0..10 from the feet) plays
        // BoomerZombie.Fall. Its emit holds the next one for the clip
        // (boomer_dive_01, 2.45 s), so a long fall yells once.
        ai.fall_yell = (ai.fall_yell - dt).max(0.);
        if ai.class == SpecialClass::Boomer && enemy.vertical < 0. && ladder_pose.is_none() && ai.fall_yell <= 0. {
            let start = transform.translation + Vec3::Y * u(5.);
            let t = c.trace(start, start - Vec3::Y * u(75.), Vec3::new(u(10.), u(5.), u(10.)));
            if !t.start_solid && t.fraction >= 1. {
                queue.play("BoomerZombie.Fall", Some(transform.translation));
                ai.fall_yell = 2.45;
            }
        }
        if let Some(mut out) = flinch_out {
            if out.1 != ai.flinch.1 {
                *out = AiFlinch(ai.flinch.0, ai.flinch.1);
            }
        }
        ai.shown_speed += (ai.speed - ai.shown_speed) * (1. - (-dt / SPEED_SMOOTHING).exp());
        enemy.movement_speed = ai.shown_speed;
        let next_activity = ladder_pose.map_or_else(|| ai.activity(), |p| p.activity());
        let next_gesture = if ladder_pose.is_some() { None } else { ai.gesture() };
        if activity.1 != next_gesture {
            activity.1 = next_gesture;
        }
        if activity.0 != next_activity {
            activity.0 = next_activity;
        }
        // Playtest trace: where each special is every 5 s.
        if (ai.clock / 5.).floor() != ((ai.clock - dt) / 5.).floor()
            && std::env::args().any(|a| a == "--ai-specials-now")
        {
            println!(
                "AI {} trace at {:?} speed {:.2} target {:?} at {:?} distance {:.0}u sees {} routed {}",
                ai.class.name(),
                transform.translation,
                ai.speed,
                target.map(|t| t.who),
                target.map(|t| t.feet),
                distance,
                sees,
                ai.routed
            );
        }
        if ai.holding().is_some() || matches!(ai.mode, Mode::Tongue { .. }) {
            holding_now.push(entity);
        }
        // Mode transitions are rare; log them for playtests and captures.
        let label = mode_label(&ai.mode);
        if label != ai.logged {
            ai.logged = label;
            println!(
                "AI {} {:.2}s -> {label} at {:?} health {:.0}",
                ai.class.name(),
                state.elapsed,
                transform.translation,
                enemy.health
            );
        }
    }
    if let Ok(mut serials) = CHARGER_POUNDS.lock() {
        *serials = pound_serials;
    }
    if let Ok(mut poses) = CHARGER_POSES.lock() {
        *poses = charger_poses;
    }
    if let Ok(mut holders) = HOLDERS.lock() {
        *holders = holding_now;
    }
    for (actor, mut transform) in &mut actors {
        if let Some((_, rotation, _)) = victim_rotations
            .iter()
            .find(|(who, _, _)| *who == Victim::Bot(actor.0))
        {
            transform.rotation = *rotation;
            if let Some(m) = squad.0.get(actor.0) {
                transform.translation = m.position;
            }
        }
    }
    // A Tank punch or rock knockdown takes the player out of first person
    // (client 1026c880: ACT_TERROR_HIT_BY_TANKPUNCH..TANKPUNCH_LAND and the
    // knockdown get-up) with their survivor shown flying and getting up.
    if let Some(k) = knocks.iter().find(|k| k.who == Victim::Player) {
        victim_rotations.push((Victim::Player, k.facing, knock_activity(k)));
    }
    let player = victim_rotations
        .iter()
        .find(|(who, _, _)| *who == Victim::Player)
        .map(|(_, rotation, activity)| PlayerPin {
            activity,
            rotation: *rotation,
        });
    if let Ok(mut pin) = PLAYER_PIN.lock() {
        *pin = player.filter(|_| state.pinned);
    }
}
fn face_toward(transform: &mut Transform, point: Vec3) {
    let face = (point - transform.translation) * Vec3::new(1., 0., 1.);
    if face.length_squared() > 1e-4 {
        transform.rotation = Quat::from_rotation_y((-face.z).atan2(face.x));
    }
}
/// AI Boomer death: bile and stagger every survivor within the blast.
fn explode(
    collision: &Collision,
    feet: Vec3,
    state: &mut Match,
    body: &Body,
    squad: &mut Squad,
    queue: &mut crate::game_audio::Queue,
    mut director: Option<&mut crate::source_director::Director>,
) {
    let center = feet + Vec3::Y * HEIGHT * 0.5;
    queue.play("BoomerZombie.Detonate", Some(center));
    let mut hit = 0;
    for i in super::boomer::blast_victims(collision, center, squad) {
        let m = &mut squad.0[i];
        super::boomer::bile(m);
        let push = (m.position - center) * Vec3::new(1., 0., 1.);
        m.state.stagger = super::boomer::STAGGER_DURATION;
        m.state.stagger_dir = push.normalize_or_zero();
        hit += 1;
    }
    if !state.infected
        && !state.dead
        && body.feet.distance(center) <= u(super::boomer::EXPLODE_OUTER)
        && clear(collision, center, body.feet + Vec3::Y * EYE)
    {
        state.vomited = super::boomer::IT_DURATION;
        state.stagger = super::boomer::STAGGER_DURATION;
        state.stagger_dir = ((body.feet - center) * Vec3::new(1., 0., 1.)).normalize_or_zero();
        hit += 1;
    }
    if hit > 0 {
        if let Some(director) = director.as_mut() {
            director.it_mob(0);
        }
    }
    println!("AI Boomer exploded: {hit} survivors biled");
}

/// The survivor crosshair is hidden in the third-person pin view.
fn pin_crosshair(
    state: Res<Match>,
    mut crosshair: Query<&mut Visibility, With<crate::game_ui::Crosshair>>,
) {
    let wanted = if state.survivor_third_person() || crate::skate_mod::riding() && !state.infected {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut visibility in &mut crosshair {
        visibility.set_if_neq(wanted);
    }
}
#[derive(Component)]
struct PlayerPinDraw(usize);
#[derive(Component)]
struct PlayerItemDraw(usize);
/// Retail survivor activity and nominal clip time for a backpack action
/// (survivor_*.mdl: Heal_Self_Standing_06 5.0 s, Heal_Friend_Standing 5.76 s,
/// Defibrillate_Incap_Standing 3.0 s, use_ammopack 2.0 s; crouched variants).
pub(crate) fn backpack_activity(kind: u8, crouched: bool) -> &'static str {
    match (kind, crouched) {
        (1, false) => "ACT_TERROR_HEAL_SELF",
        (1, true) => "ACT_TERROR_CROUCH_HEAL_SELF",
        (2, false) => "ACT_TERROR_HEAL_FRIEND",
        (2, true) => "ACT_TERROR_CROUCH_HEAL_FRIEND",
        (3, false) => "ACT_TERROR_USE_DEFIBRILLATOR",
        (3, true) => "ACT_TERROR_CROUCH_USE_DEFIBRILLATOR",
        (_, false) => "ACT_TERROR_USE_UPGRADEPACK",
        (_, true) => "ACT_TERROR_CROUCH_USE_UPGRADEPACK",
    }
}
/// The survivor player's own body, drawn only in the third-person views:
/// a pin (native victim activity of the special holding them) or a
/// backpack action (first aid, defibrillator, upgrade pack), which also
/// shows the item in the survivor's hand.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn player_pin_body(
    mut commands: Commands,
    state: Res<Match>,
    body: Single<&Body>,
    squad: Res<Squad>,
    rules: Res<crate::game_state::Rules>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    time: Res<Time>,
    (mut meshes, mut bindposes, mut joints): (
        ResMut<Assets<Mesh>>,
        ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
        Query<&mut Transform, With<GpuBone>>,
    ),
    mut draws: Query<
        (Entity, &PlayerPinDraw, &Mesh3d, &mut Transform, Option<&GpuSkin>),
        (Without<PlayerItemDraw>, Without<GpuBone>),
    >,
    mut items: Query<
        (Entity, &PlayerItemDraw, &Mesh3d, &mut Transform, Option<&GpuSkin>),
        (Without<PlayerPinDraw>, Without<GpuBone>),
    >,
    mut clock: Local<(f32, &'static str, Option<f32>, String)>,
    skate: Option<Res<crate::skate_mod::Skate>>,
) {
    let riding = skate.as_ref().is_some_and(|s| s.riding) && !state.pinned;
    let session = session.filter(|_| state.survivor_third_person() || riding);
    // (activity, rotation, pose time, held item model)
    let pose = session.as_ref().and_then(|session| {
        let model = format!(
            "models/survivors/survivor_{}.mdl",
            session.survivor.character()
        );
        if state.pinned {
            return player_pin().map(|p| (p.activity, p.rotation, None, String::new(), model));
        }
        if let Some(skate) = skate.as_ref().filter(|_| riding) {
            // Skate mod: standing sideways on the board (pistol stance), the
            // jump pose in the air.
            let activity = if body.grounded { "ACT_IDLE_PISTOL" } else { "ACT_JUMP_PISTOL" };
            return Some((activity, Quat::from_rotation_y(skate.heading), None, rules.pistol.player_model.clone(), model));
        }
        if let Some(hang) = state.ledge {
            // Hanging from a ledge, facing the wall.
            // A standing teammate's revive_time is its revive of us; a
            // downed teammate's is their own revive, which must not play
            // our climb. Online guests: the host's revive of us.
            let revive = squad
                .0
                .iter()
                .filter(|m| !m.state.incap && !m.state.dead)
                .map(|m| m.revive_time)
                .fold(crate::net_play::revive_progress(), f32::max);
            let activity = crate::ledge_hang::activity(&state, revive)?;
            let to_wall = (hang.stand - hang.hang) * Vec3::new(1., 0., 1.);
            let facing = if to_wall.length_squared() > 1e-4 {
                (-to_wall.x).atan2(-to_wall.z)
            } else {
                body.yaw
            };
            let rotation = Quat::from_rotation_y(facing + std::f32::consts::FRAC_PI_2);
            let length = library.duration(&model, &[activity]).max(0.1);
            let time = (revive > 0.).then(|| (revive / 5.).clamp(0., 1.) * length);
            return Some((activity, rotation, time, String::new(), model));
        }
        if state.reviving_teammate() {
            // The reviver's heal-incapacitated loop, facing the teammate.
            let facing = state
                .reviving
                .and_then(|i| squad.0.get(i))
                .map(|m| (m.position - body.feet) * Vec3::new(1., 0., 1.))
                .filter(|d| d.length_squared() > 1e-4)
                .map(|d| (-d.x).atan2(-d.z))
                .unwrap_or(body.yaw);
            let rotation = Quat::from_rotation_y(facing + std::f32::consts::FRAC_PI_2);
            let item = rules
                .weapons
                .get(state.held_class())
                .map(|w| w.player_model.clone())
                .unwrap_or_default();
            return Some(("ACT_TERROR_HEAL_INCAPACITATED", rotation, None, item, model));
        }
        if !state.backpack_action() {
            return None;
        }
        let activity = backpack_activity(state.medical_kind, body.crouched);
        // Face the teammate being healed/defibrillated, else the view yaw at
        // the start of the action (the camera orbits freely meanwhile).
        let facing = state
            .medical_target
            .and_then(|i| squad.0.get(i))
            .map(|m| (m.position - body.feet) * Vec3::new(1., 0., 1.))
            .filter(|d| d.length_squared() > 1e-4)
            .map(|d| (-d.x).atan2(-d.z))
            .or(clock.2)
            .unwrap_or(body.yaw);
        let rotation = Quat::from_rotation_y(facing + std::f32::consts::FRAC_PI_2);
        // The clip runs over the whole use (5 s kit, 2.5 s with adrenaline).
        let length = library.duration(&model, &[activity]).max(0.1);
        let progress = (1. - state.action / state.action_total.max(1e-3)).clamp(0., 1.);
        let item = rules
            .weapons
            .get(state.held_class())
            .map(|w| w.player_model.clone())
            .unwrap_or_default();
        Some((activity, rotation, Some(progress * length), item, model))
    });
    let Some((activity, rotation, scripted_time, item, model)) = pose else {
        for (entity, ..) in &draws {
            commands.entity(entity).despawn();
        }
        for (entity, ..) in &items {
            commands.entity(entity).despawn();
        }
        *clock = (0., "", None, String::new());
        return;
    };
    if clock.1 != activity {
        *clock = (0., activity, Some(body.yaw), clock.3.clone());
    } else if !state.paused {
        clock.0 += time.delta_secs();
    }
    let mut t = scripted_time.unwrap_or(clock.0);
    let mut activity = activity;
    // Rochelle and Coach have no standing choke; like the bots' victim
    // branch, fall back to the ground choke.
    if activity == super::smoker::VictimPose::ChokeStanding.activity()
        && library.scripted_sample_time(&model, activity, 0.).is_none()
    {
        activity = super::smoker::VictimPose::ChokeGround.activity();
    }
    if activity == "ACT_TERROR_HIT_BY_TANKPUNCH" {
        let hit = library.duration(&model, &[activity]);
        if t >= hit {
            activity = "ACT_TERROR_IDLE_FALL_FROM_TANKPUNCH";
            t -= hit;
        }
    }
    let lift = if riding { Vec3::Y * 0.1 } else { Vec3::ZERO };
    let transform = Transform::from_translation(body.feet + lift).with_rotation(rotation);
    // GPU-skinned like the intro survivors: CPU skinning rewrote and
    // re-uploaded the whole survivor mesh every frame (~5 ms per frame of
    // mesh reallocation while pinned, healing or reviving).
    if draws.is_empty() {
        if let Some(parts) = models.0.get(&(model.clone(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let entity = commands
                    .spawn((
                        PlayerPinDraw(index),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        transform,
                        RenderLayers::layer(1),
                        crate::viewer::DynamicModelLighting::default(),
                    ))
                    .id();
                library.scripted_gpu(
                    &mut commands, entity, None, &mut joints, &mut meshes, &mut bindposes, mesh,
                    (&model, &model, index),
                    (activity, t),
                );
            }
        }
    } else {
        for (entity, draw, mesh, mut location, gpu) in &mut draws {
            *location = transform;
            if !state.paused || gpu.is_none() {
                library.scripted_gpu(
                    &mut commands, entity, gpu, &mut joints, &mut meshes, &mut bindposes, &mesh.0,
                    (&model, &model, draw.0),
                    (activity, t),
                );
            }
        }
    }
    // Held item, skinned to the survivor's hand (ValveBiped.weapon_bone) the
    // way survivor bots hold theirs.
    let item = if library.contains(&item) {
        item
    } else {
        String::new()
    };
    if clock.3 != item {
        for (entity, ..) in &items {
            commands.entity(entity).despawn();
        }
        clock.3 = item.clone();
        if let Some(parts) = models.0.get(&(item.clone(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let entity = commands
                    .spawn((
                        PlayerItemDraw(index),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        transform,
                        RenderLayers::layer(1),
                        crate::viewer::DynamicModelLighting::default(),
                    ))
                    .id();
                library.scripted_gpu(
                    &mut commands, entity, None, &mut joints, &mut meshes, &mut bindposes, mesh,
                    (&model, &item, index),
                    (activity, t),
                );
            }
        }
        return;
    }
    for (entity, draw, mesh, mut location, gpu) in &mut items {
        *location = transform;
        if !state.paused || gpu.is_none() {
            library.scripted_gpu(
                &mut commands, entity, gpu, &mut joints, &mut meshes, &mut bindposes, &mesh.0,
                (&model, &item, draw.0),
                (activity, t),
            );
        }
    }
}
#[derive(Component)]
struct AiTongue(Entity);
/// Tongue ropes from each AI Smoker's mouth to its tip or held survivor.
#[allow(clippy::too_many_arguments)]
/// A Smoker's tongue path: mouth, bends, then the tip (flying) or the
/// victim's chest (reeling). None when no tongue is out. Multiplayer sends
/// it to guests (net_infected).
pub(crate) fn tongue_path(transform: &Transform, ai: &AiSpecial, player: Vec3, squad: &Squad) -> Option<Vec<Vec3>> {
    let mouth = transform.translation + Vec3::Y * EYE + transform.rotation * Vec3::X * u(6.);
    let chest = |who: Victim| match who {
        Victim::Player => player + Vec3::Y * 0.9,
        Victim::Bot(i) => squad.0.get(i).map_or(mouth, |m| m.position + Vec3::Y * 0.9),
    };
    let end = match ai.mode {
        Mode::Tongue { tip, .. } => tip,
        Mode::Reel { victim, .. } if ai.class == SpecialClass::Smoker => chest(victim),
        _ => return None,
    };
    let mut path = vec![mouth];
    if matches!(ai.mode, Mode::Reel { .. }) {
        path.extend(ai.bends.iter().copied());
    }
    path.push(end);
    Some(path)
}

/// A guest's puppet Smoker: its tongue path from the host's snapshot.
#[derive(Component)]
pub(crate) struct NetTongue(pub Vec<Vec3>);

fn draw_tongues(
    mut commands: Commands,
    specials: Query<(Entity, &Transform, &AiSpecial)>,
    remote: Query<(Entity, &NetTongue)>,
    body: Single<&Body>,
    squad: Res<Squad>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut assets: (
        ResMut<Assets<Mesh>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<StandardMaterial>>,
    ),
    ropes: Query<(Entity, &AiTongue, &Mesh3d)>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
    time: Res<Time>,
) {
    let (meshes, images, materials) = &mut assets;
    let camera = camera.iter().next().map_or(Vec3::ZERO, |c| c.translation());
    let mut drawn = vec![];
    let paths = specials
        .iter()
        .filter_map(|(entity, transform, ai)| Some((entity, tongue_path(transform, ai, body.feet, &squad)?)))
        .chain(remote.iter().filter(|(_, t)| t.0.len() >= 2).map(|(entity, t)| (entity, t.0.clone())));
    for (entity, path) in paths {
        drawn.push(entity);
        let mesh = super::smoker::rope_mesh(&super::smoker::tongue_points(&path, time.elapsed_secs()), camera);
        if let Some((_, _, handle)) = ropes.iter().find(|(_, owner, _)| owner.0 == entity) {
            if let Some(mut existing) = meshes.get_mut(&handle.0) {
                *existing = mesh;
            }
            continue;
        }
        let Some(source) = source.as_ref() else {
            continue;
        };
        let handle = material
            .get_or_insert_with(|| super::smoker::tongue_material(source, images, materials))
            .clone();
        commands.spawn((
            AiTongue(entity),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(handle),
            Transform::default(),
            bevy::camera::visibility::NoFrustumCulling,
            RenderLayers::layer(1),
        ));
    }
    for (rope, owner, _) in &ropes {
        if !drawn.contains(&owner.0) {
            commands.entity(rope).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shoves_and_flinches_follow_the_survivors_side() {
        // Facing +X (identity rotation).
        let r = Quat::IDENTITY;
        assert_eq!(side_of(r, Vec3::ZERO, Vec3::X), 0);
        assert_eq!(side_of(r, Vec3::ZERO, Vec3::NEG_X), 1);
        assert_eq!(side_of(r, Vec3::ZERO, Vec3::NEG_Z), 2);
        assert_eq!(side_of(r, Vec3::ZERO, Vec3::Z), 3);
        // A shover in front pushes it backward; one on its left pushes it right.
        assert_eq!(shove_activity(SpecialClass::Smoker, false, 0), "ACT_TERROR_SHOVED_BACKWARD");
        assert_eq!(shove_activity(SpecialClass::Smoker, false, 2), "ACT_TERROR_SHOVED_RIGHTWARD");
        assert_eq!(shove_activity(SpecialClass::Hunter, true, 1), "ACT_TERROR_HUNTER_POUNCE_KNOCKOFF_FORWARD");
        // Facing -Z (turned 90 degrees left): a survivor at -Z is in front.
        let left = facing(Vec3::ZERO, Vec3::NEG_Z);
        assert_eq!(side_of(left, Vec3::ZERO, Vec3::NEG_Z), 0);
        assert_eq!(flinch_activity(SpecialClass::Hunter, true, 1), Some("ACT_FLINCH_CROUCH_Back"));
        assert_eq!(flinch_activity(SpecialClass::Jockey, false, 0), None);
    }
    #[test]
    fn ai_special_and_victim_activities_exist_in_the_native_models() {
        use l4d2_formats::anim::Skeleton;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let load = |path: &str| Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let victim = Victim::Player;
        let modes = [
            Mode::Chase,
            Mode::Crouch(1.),
            Mode::Airborne(Vec3::ZERO),
            Mode::Pin(victim),
            Mode::Tongue { victim, tip: Vec3::ZERO },
            Mode::Reel { victim, choking: false },
            Mode::Reel { victim, choking: true },
            Mode::Vomit(1.),
            Mode::Spit(1.),
            Mode::Flee(1.),
            Mode::Warmup(1.),
            Mode::Charge { direction: Vec3::X, carried: None },
            Mode::Stagger(1.),
            Mode::Swing { clock: 0., hit: false, low: false },
            Mode::Swing { clock: 0., hit: false, low: true },
            Mode::Throw { clock: 0., aim: Vec3::ZERO, released: false, variant: 0 },
        ];
        let mut missing = vec![];
        for class in SpecialClass::MINIONS.into_iter().chain([SpecialClass::Tank]) {
            let sk = load(class.model());
            for mode in modes {
                // Only the modes each class can enter.
                let valid = match mode {
                    Mode::Chase | Mode::Stagger(_) => true,
                    Mode::Flee(_) => matches!(class, SpecialClass::Boomer | SpecialClass::Spitter),
                    Mode::Crouch(_) => class == SpecialClass::Hunter,
                    Mode::Airborne(_) => matches!(class, SpecialClass::Hunter | SpecialClass::Jockey),
                    Mode::Pin(_) => matches!(class, SpecialClass::Hunter | SpecialClass::Jockey | SpecialClass::Charger),
                    Mode::Tongue { .. } | Mode::Reel { .. } => class == SpecialClass::Smoker,
                    Mode::Vomit(_) => class == SpecialClass::Boomer,
                    Mode::Spit(_) => class == SpecialClass::Spitter,
                    Mode::Warmup(_) | Mode::Charge { .. } => class == SpecialClass::Charger,
                    Mode::Swing { .. } | Mode::Throw { .. } => class == SpecialClass::Tank,
                };
                if !valid || class == SpecialClass::Tank && matches!(mode, Mode::Stagger(_)) {
                    continue;
                }
                let mut ai = AiSpecial::new(class);
                ai.mode = mode;
                // Timed phases: lunge/pounce starts, landing, tongue phases, falling.
                for (clock, land, end_tongue, sight, falling) in [(0., 0., 0., 0., false), (0.1, 0.5, 0.2, 0.5, false), (0.1, 0., 0., 0., true)] {
                    ai.clock = clock;
                    ai.land = land;
                    ai.end_tongue = end_tongue;
                    ai.sight = sight;
                    ai.falling = falling;
                    let act = ai.activity();
                    if sk.find_sequence(act).is_none() {
                        missing.push(format!("{} {act}", class.name()));
                    }
                }
                ai.clock = 0.;
                ai.land = 0.;
                ai.end_tongue = 0.;
                ai.sight = 0.;
                ai.falling = false;
                for claw in [0., 1.] {
                    ai.claw = claw;
                    for speed in [0., 5.] {
                        ai.speed = speed;
                        ai.still = if speed > 0. { 0. } else { 1. };
                        for act in [Some(ai.activity()), ai.gesture()].into_iter().flatten() {
                            if sk.find_sequence(act).is_none() {
                                missing.push(format!("{} {act}", class.name()));
                            }
                        }
                    }
                }
            }
        }
        // Directional shoves, Hunter knock-offs and hit flinches.
        for class in SpecialClass::MINIONS {
            let sk = load(class.model());
            for side in 0..4 {
                let mut acts = vec![shove_activity(class, false, side), shove_activity(class, true, side)];
                acts.extend(flinch_activity(class, false, side));
                acts.extend(flinch_activity(class, true, side));
                for act in acts {
                    if sk.find_sequence(act).is_none() {
                        missing.push(format!("{} {act}", class.name()));
                    }
                }
            }
        }
        // Survivor victim poses shown on the player's own body.
        let mut acts = vec![
            "ACT_TERROR_INCAP_FROM_POUNCE",
            "ACT_IDLE_POUNCED",
            "ACT_TERROR_JOCKEY_RIDDEN",
            "ACT_TERROR_CHARGER_POUNDED",
            "ACT_TERROR_CHARGER_POUNDED_INCAP",
            "ACT_TERROR_CARRIED",
        ];
        for pose in [
            super::super::smoker::VictimPose::Pulled,
            super::super::smoker::VictimPose::ChokeStanding,
        ] {
            acts.push(pose.activity());
        }
        for by in [SpecialClass::Tank, SpecialClass::Charger] {
            for (airborne, flight) in [(true, 0.), (true, 1.), (false, 1.), (false, 0.)] {
                let k = Knock {
                    who: victim,
                    velocity: Vec3::ZERO,
                    airborne,
                    getup: 0.,
                    flight,
                    facing: Quat::IDENTITY,
                    by,
                };
                acts.push(knock_activity(&k));
            }
        }
        for name in ["gambler", "mechanic", "coach", "producer", "namvet", "biker", "manager", "teenangst"] {
            let sk = load(&format!("models/survivors/survivor_{name}.mdl"));
            for act in &acts {
                let choke = super::super::smoker::VictimPose::ChokeStanding.activity();
                let ground = super::super::smoker::VictimPose::ChokeGround.activity();
                if *act == choke && sk.find_sequence(ground).is_some() {
                    continue;
                }
                if sk.find_sequence(act).is_none() {
                    missing.push(format!("{name} {act}"));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "missing activities: {missing:#?}");
    }
    #[test]
    fn console_spawns_the_six_ai_classes_and_the_tank() {
        assert_eq!(
            command("z_spawn hunter").as_deref(),
            Some("Spawning hunter")
        );
        assert_eq!(
            command("z_spawn Charger").as_deref(),
            Some("Spawning charger")
        );
        assert!(command("z_spawn witch").is_none());
        assert_eq!(command("z_spawn tank").as_deref(), Some("Spawning tank"));
        assert!(command("god 1").is_none());
        let queued = take_requests();
        assert_eq!(queued, vec![SpecialClass::Hunter, SpecialClass::Charger, SpecialClass::Tank]);
    }
    #[test]
    fn retail_class_values() {
        assert_eq!(health(SpecialClass::Hunter), 250.);
        assert_eq!(health(SpecialClass::Smoker), 250.);
        assert_eq!(health(SpecialClass::Boomer), 50.);
        assert_eq!(health(SpecialClass::Charger), 600.);
        assert_eq!(run_speed(SpecialClass::Hunter), 300.);
        assert_eq!(run_speed(SpecialClass::Boomer), 175.);
        assert_eq!(claw_damage(SpecialClass::Hunter), 6.);
        assert_eq!(claw_damage(SpecialClass::Charger), 10.);
        assert_eq!(claw_damage(SpecialClass::Jockey), 4.);
    }
    #[test]
    fn pounce_lofts_with_distance_up_to_the_retail_cap() {
        let near = pounce_direction(Vec3::ZERO, Vec3::NEG_Z * u(100.));
        let far = pounce_direction(Vec3::ZERO, Vec3::NEG_Z * u(500.));
        let pitch = |d: Vec3| d.y.atan2(d.xz().length()).to_degrees();
        assert!((pitch(near) - 3.5).abs() < 0.1, "{}", pitch(near));
        assert!((pitch(far) - 17.5).abs() < 0.1, "{}", pitch(far));
        let very_far = pounce_direction(Vec3::ZERO, Vec3::NEG_Z * u(5000.));
        assert!((pitch(very_far) - 45.).abs() < 0.1);
    }
    #[test]
    fn tank_punch_is_a_gesture_over_locomotion_and_knocks_pose_the_victim() {
        let mut tank = AiSpecial::new(SpecialClass::Tank);
        tank.mode = Mode::Swing { clock: 0., hit: false, low: false };
        tank.speed = 3.;
        assert_eq!(tank.activity(), "ACT_RUN");
        assert_eq!(tank.gesture(), Some("ACT_TERROR_ATTACK_MOVING"));
        tank.speed = 0.;
        tank.still = 1.;
        assert_eq!(tank.activity(), "ACT_IDLE");
        tank.mode = Mode::Swing { clock: 0., hit: false, low: true };
        assert_eq!(tank.gesture(), Some("ACT_HULK_ATTACK_LOW"));
        tank.mode = Mode::Chase;
        assert_eq!(tank.gesture(), None);
        // The knocked player's pose: punch hit/flight, punch get-up, rock get-up.
        let mut k = Knock {
            who: Victim::Player,
            velocity: Vec3::X,
            airborne: true,
            getup: 1.5,
            flight: 0.,
            facing: facing(Vec3::ZERO, Vec3::X),
            by: SpecialClass::Tank,
        };
        assert_eq!(knock_activity(&k), "ACT_TERROR_HIT_BY_TANKPUNCH");
        k.airborne = false;
        k.flight = 0.4;
        assert_eq!(knock_activity(&k), "ACT_TERROR_TANKPUNCH_LAND");
        k.flight = 0.;
        assert_eq!(knock_activity(&k), "ACT_TERROR_TANKROCK_TO_STAND");
        // Facing +X keeps the model's +X forward.
        assert!((k.facing * Vec3::X - Vec3::X).length() < 1e-5);
    }
    #[test]
    fn activities_follow_the_mode() {
        let mut ai = AiSpecial::new(SpecialClass::Hunter);
        assert_eq!(ai.activity(), "ACT_IDLE");
        ai.speed = 3.;
        assert_eq!(ai.activity(), "ACT_RUN");
        ai.mode = Mode::Pin(Victim::Player);
        // Pounce_01 impact, then the Melee_Pounce shredding loop.
        assert_eq!(ai.activity(), "ACT_TERROR_HUNTER_POUNCE");
        ai.clock = 0.5;
        assert_eq!(ai.activity(), "ACT_TERROR_HUNTER_POUNCE_MELEE");
        assert_eq!(ai.holding(), Some(Victim::Player));
        let mut smoker = AiSpecial::new(SpecialClass::Smoker);
        smoker.mode = Mode::Reel {
            victim: Victim::Bot(1),
            choking: true,
        };
        assert_eq!(smoker.activity(), "ACT_TERROR_SMOKER_CRITICAL_ATTACK_IDLE");
        assert_eq!(smoker.holding(), Some(Victim::Bot(1)));
    }
    #[test]
    fn live_counts_follow_the_director_minion_order() {
        let alive = Enemy {
            model: String::new(),
            death_loot: None,
            loot_dropped: false,
            death: Default::default(),
            movement_speed: 0.,
            reaction_serial: 0,
            attack_serial: 0,
            health: 10.,
            attack: 0.,
            reaction: 0,
            corpse_time: 0.,
            burning: 0.,
            vertical: 0.,
        };
        let dead = Enemy {
            model: String::new(),
            death_loot: None,
            loot_dropped: false,
            death: Default::default(),
            movement_speed: 0.,
            reaction_serial: 0,
            attack_serial: 0,
            health: 0.,
            attack: 0.,
            reaction: 0,
            corpse_time: 0.,
            burning: 0.,
            vertical: 0.,
        };
        let hunter = AiSpecial::new(SpecialClass::Hunter);
        let charger = AiSpecial::new(SpecialClass::Charger);
        let counts =
            live_counts([(&hunter, &alive), (&charger, &alive), (&charger, &dead)].into_iter());
        assert_eq!(counts, [1, 0, 0, 0, 0, 1]);
    }
}
