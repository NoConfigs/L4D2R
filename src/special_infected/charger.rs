//! Playable Charger: charge, carry, wall/ground slam, pummel, impact flings
//! and claw. The shared lifecycle (team switch, ghost spawn, death/respawn,
//! survivor-bot opposition) lives in special_infected.rs; this module adds
//! the class ability as a FixedUpdate system scheduled after
//! special_infected::tick.
//!
//! Retail sources (see local-research/si-agents/charger/NOTES.md):
//! running L4D2 build 10097 cvars, owned server.dll pseudocode (CCharge
//! 0x10361880..0x10363770, CTerrorPlayer carry/pummel 0x1030cf70..0x10331590)
//! and retail VScript traces local-research/parity/charger-*.log.
use super::{Hunter, SpecialClass};
use crate::{
    collision::{Collision, HEIGHT, RADIUS, UNIT},
    game::{Body, CompanionActor},
    game_state::{Match, Squad},
};
use bevy::prelude::*;
use std::sync::Mutex;

pub const WEAPON: &str = "weapon_charger_claw";
pub const MODEL: &str = "models/infected/charger.mdl";
pub const VIEW: &str = "models/weapons/arms/v_charger_arms.mdl";
/// Retail z_charger_health=600.
pub const HEALTH: f32 = 600.;
/// Retail m_flMaxspeed=250 outside a charge (weapon_charger_claw
/// MaxPlayerSpeed=250; measured in charger-free.log).
pub const SPEED: f32 = 250.;
pub const PLAYABLE: bool = true;
/// Retail charge, carry, slam stagger and pummel all use a chase/orbit
/// camera (charger-retail-charge-view.jpg, charger-retail-pummel-view.jpg).
/// The Charger keeps special_phase 3 for that whole sequence.
pub const THIRD_PERSON_PIN: bool = true;
/// Extra viewmodel (action, activity) pairs. v_charger_arms.mdl only has
/// idle, deploy, PRIMARYATTACK and two SHOOT swings; no charge activity.
pub const VM_ACTIONS: &[(&str, &str)] = &[("swing", "ACT_VM_SHOOT")];
pub const CONTROLS: &str = "Charger playtest: left click charges (grabs the first survivor, slams walls, then pummels); right click claws.";

/// z_charge_max_speed (m_flMaxspeed during a charge).
pub const CHARGE_SPEED: f32 = 500.;
/// z_charge_duration.
pub const CHARGE_DURATION: f32 = 2.5;
/// z_charge_interval: cooldown after a charge that carried nobody.
pub const CHARGE_INTERVAL: f32 = 12.;
/// Timer set by the pummel end (owned FUN_103612c0: 0.2 seconds).
pub const PUMMEL_END_INTERVAL: f32 = 0.2;
/// z_charge_max_damage (Normal difficulty multiplier 1.0).
pub const IMPACT_DAMAGE: f32 = 10.;
/// z_charge_max_force / z_charge_min_force.
pub const MAX_FORCE: f32 = 800.;
pub const MIN_FORCE: f32 = 550.;
/// Slam damage to the carried survivor (FUN_10331590, Normal: 10).
pub const SLAM_DAMAGE: f32 = 10.;
/// z_charger_pound_dmg per AE_CHARGER_POUND_IMPACT.
pub const POUND_DAMAGE: f32 = 15.;
/// z_charger_impact_epsilon: units moved per 0.1s below which a charge is stuck.
pub const IMPACT_EPSILON: f32 = 8.;
/// z_charge_impact_angle.
pub const IMPACT_ANGLE: f32 = 0.7;
/// z_charger_probe_alone / z_charger_probe_attack.
pub const PROBE_ALONE: f32 = 6.;
pub const PROBE_CARRY: f32 = 24.;
/// Pummel start delays from FUN_10331590 (wall slam 0.75, ground slam 1.0).
pub const WALL_PUMMEL_DELAY: f32 = 0.75;
pub const GROUND_PUMMEL_DELAY: f32 = 1.;
/// Slam stage before the pummel starts (FUN_10331590).
pub(crate) fn slam_delay(wall: bool) -> f32 {
    if wall {
        WALL_PUMMEL_DELAY
    } else {
        GROUND_PUMMEL_DELAY
    }
}
/// Wall stagger without a victim, measured 0.87s (charger-turn.log).
pub const STAGGER_TIME: f32 = 0.87;
/// Approximate stagger drift (retail 55..105 units/sec in the traces).
pub const STAGGER_SPEED: f32 = 80.;
/// charger_pz_claw_dmg, claw_range, claw_swing_interval / miss interval.
pub const CLAW_DAMAGE: f32 = 10.;
pub const CLAW_RANGE: f32 = 52.;
pub const CLAW_INTERVAL: f32 = 1.;
pub const CLAW_MISS_INTERVAL: f32 = 0.5;
/// Native survivor ACT_TERROR_CHARGERHIT_LAND_SLOW (GetUpFrom_Charger).
pub const GETUP_TIME: f32 = 2.833;
/// Native charger.mdl clip lengths (asset test checks them).
const CHARGE_CLIP: f32 = 2.367;

/// Viewmodel activity override per special_phase; None keeps the default.
pub fn vm_activity(phase: u8) -> Option<&'static str> {
    match phase {
        2 | 3 => Some("ACT_VM_IDLE"),
        _ => None,
    }
}
pub fn install(app: &mut App) {
    app.init_resource::<Charger>()
        .add_systems(FixedUpdate, tick.after(super::tick))
        .add_systems(Update, world_model.after(crate::game::controls));
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Stage {
    #[default]
    Idle,
    Charging,
    /// Wall impact without a victim (retail Shoved_* stagger).
    Stagger,
    /// Carried survivor slammed; `wall` selects the wall or ground variant.
    Slam {
        wall: bool,
    },
    Pummel,
}
#[derive(Clone, Copy, Debug)]
struct Flung {
    index: usize,
    feet: Vec3,
    velocity: Vec3,
    clock: f32,
    landed: Option<f32>,
}
/// Charger ability state (kept separate from the shared `Hunter`).
#[derive(Resource, Default)]
pub struct Charger {
    /// `--capture-charger-charge` staging: (spawned, charge fired).
    capture: (bool, bool),
    pub(crate) stage: Stage,
    pub(crate) clock: f32,
    pub(crate) yaw: f32,
    /// Seconds until the ability can activate again (`None`: locked).
    pub(crate) timer: Option<f32>,
    pub(crate) pound: f32,
    pound_label: &'static str,
    sample: Vec3,
    sample_clock: f32,
    hit: Vec<usize>,
    flung: Vec<Flung>,
    getup: Vec<(usize, f32)>,
    previous: Vec3,
    /// Survivor held by this charge/pummel (survives a shared release).
    held: Option<usize>,
    stagger: Vec3,
    slam_point: Vec3,
    swing: bool,
}
impl Charger {
    fn ready(&self) -> bool {
        self.timer.is_some_and(|t| t <= 0.)
    }
    /// HUD meter: 1 when a charge is available, filling during the interval.
    pub(crate) fn progress(&self) -> f32 {
        match self.timer {
            None => 0.,
            Some(t) if t <= 0. => 1.,
            Some(t) => (1. - t / CHARGE_INTERVAL).clamp(0., 1.),
        }
    }
    fn reset(&mut self) {
        let flung = std::mem::take(&mut self.flung);
        let getup = std::mem::take(&mut self.getup);
        *self = Self {
            flung,
            getup,
            ..default()
        };
    }
}

/// Victim pose published for the survivor animation branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VictimPose {
    Carried,
    SlammedWall,
    SlammedGround,
    /// ACT_TERROR_CHARGER_PUMMELED while the Charger plays its pummel intro.
    Pummeled,
    Pounded,
    PoundedNorth,
    PoundedUp,
    PoundedIncap,
    Flung,
    GetUp,
}
static POSES: Mutex<Vec<(usize, VictimPose)>> = Mutex::new(Vec::new());
fn publish(poses: Vec<(usize, VictimPose)>) {
    if let Ok(mut p) = POSES.lock() {
        *p = poses;
    }
}
pub(crate) fn victim_pose(index: usize) -> Option<VictimPose> {
    POSES
        .lock()
        .ok()?
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, p)| *p)
        .or_else(|| super::ai::charger_pose(index))
}
/// Survivor sequence for a Charger victim pose. Returns `None` (and clears a
/// stale `charger_` activity) when the survivor has no Charger pose.
pub(crate) fn victim_sequence(
    sk: &l4d2_formats::anim::Skeleton,
    index: usize,
    activity: &mut String,
    clock: &mut f32,
) -> Option<usize> {
    let Some(pose) = victim_pose(index) else {
        if activity.starts_with("charger_") {
            activity.clear();
        }
        return None;
    };
    let serial = matches!(
        pose,
        VictimPose::Pounded | VictimPose::PoundedNorth | VictimPose::PoundedUp | VictimPose::PoundedIncap
    )
    .then(|| super::ai::charger_pound_serial(index))
    .flatten();
    victim_sequence_serial(sk, pose, activity, clock, serial)
}
/// `victim_sequence_for`, restarting the main clip whenever `serial`
/// changes (one restart per AI Charger pound).
pub(crate) fn victim_sequence_serial(
    sk: &l4d2_formats::anim::Skeleton,
    pose: VictimPose,
    activity: &mut String,
    clock: &mut f32,
    serial: Option<u32>,
) -> Option<usize> {
    let Some(serial) = serial else {
        return victim_sequence_for(sk, pose, activity, clock);
    };
    // The activity carries "#<serial>"; strip it so the pose machine sees
    // its own name, and restart the clip when the pound changes.
    let (base, previous) = match activity.split_once('#') {
        Some((base, s)) => (base.to_string(), s.parse::<u32>().ok()),
        None => (activity.clone(), None),
    };
    let mut plain = base;
    if previous.is_some_and(|p| p != serial) {
        plain.clear();
    }
    let seq = victim_sequence_for(sk, pose, &mut plain, clock);
    *activity = format!("{plain}#{serial}");
    seq
}
pub(crate) fn victim_sequence_for(
    sk: &l4d2_formats::anim::Skeleton,
    pose: VictimPose,
    activity: &mut String,
    clock: &mut f32,
) -> Option<usize> {
    let find = |names: &[&str]| names.iter().find_map(|n| sk.find_sequence(n));
    let duration = |seq: usize| {
        sk.sequence_anim(seq)
            .map_or(0., |a| sk.anims()[a].duration())
    };
    fn enter(name: &str, activity: &mut String, clock: &mut f32) {
        if activity != name {
            *activity = name.into();
            *clock = 0.;
        }
    }
    // (lead-in, lead-in activity) then (main, activity, looping)
    let (lead, main, looping): (Option<(&str, &[&str])>, (&str, &[&str]), bool) = match pose {
        VictimPose::Carried => (
            Some((
                "charger_hit",
                &["ACT_TERROR_HIT_BY_CHARGER", "Hit_by_Charger"],
            )),
            (
                "charger_carried",
                &["ACT_TERROR_CARRIED", "Charger_Carried"],
            ),
            false,
        ),
        VictimPose::Flung => (
            Some((
                "charger_hit",
                &["ACT_TERROR_HIT_BY_CHARGER", "Hit_by_Charger"],
            )),
            (
                "charger_fall",
                &[
                    "ACT_TERROR_IDLE_FALL_FROM_CHARGERHIT",
                    "Idle_Fall_From_charger",
                ],
            ),
            true,
        ),
        VictimPose::SlammedWall => (
            None,
            (
                "charger_slam_wall",
                &["ACT_TERROR_SLAMMED_WALL", "Charger_Slammed_Wall"],
            ),
            false,
        ),
        VictimPose::SlammedGround => (
            None,
            (
                "charger_slam_ground",
                &["ACT_TERROR_SLAMMED_GROUND", "Charger_Slammed_Ground"],
            ),
            false,
        ),
        VictimPose::Pounded => (
            None,
            (
                "charger_pounded",
                &["ACT_TERROR_CHARGER_POUNDED_DOWN", "Charger_pounded_down"],
            ),
            true,
        ),
        VictimPose::Pummeled => (
            None,
            (
                "charger_pummeled",
                &["ACT_TERROR_CHARGER_PUMMELED", "IncapFrom_Charger"],
            ),
            false,
        ),
        VictimPose::PoundedNorth => (
            None,
            (
                "charger_pounded_north",
                &["ACT_TERROR_CHARGER_POUNDED_NORTH", "Charger_pounded_north"],
            ),
            true,
        ),
        VictimPose::PoundedUp => (
            None,
            (
                "charger_pounded_up",
                &["ACT_TERROR_CHARGER_POUNDED_UP", "Charger_pounded_up"],
            ),
            true,
        ),
        // Retail 1033f7f0 returns POUNDED_INCAP straight away for an
        // incapacitated victim (PUMMELED belongs to the pummel intro).
        VictimPose::PoundedIncap => (
            None,
            (
                "charger_pounded_incap",
                &["ACT_TERROR_CHARGER_POUNDED_INCAP", "Charger_pounded_incap"],
            ),
            true,
        ),
        VictimPose::GetUp => (
            None,
            (
                "charger_getup",
                &["ACT_TERROR_CHARGERHIT_LAND_SLOW", "GetUpFrom_Charger"],
            ),
            false,
        ),
    };
    if let Some((name, names)) = lead {
        let in_lead = activity == name;
        let in_main = activity == main.0;
        if !in_lead && !in_main {
            enter(name, activity, clock);
        }
        if activity == name {
            let seq = find(names)?;
            if *clock < duration(seq) {
                return Some(seq);
            }
            enter(main.0, activity, clock);
        }
    } else {
        enter(main.0, activity, clock);
    }
    let seq = find(main.1)?;
    let length = duration(seq);
    if length > 0. && *clock >= length {
        *clock = if looping { *clock % length } else { length };
    }
    Some(seq)
}

/// Charge velocity: ordinary Source ground friction/acceleration toward
/// z_charge_max_speed. Matches the retail ramp 83.3, 155.5, 218.1... u/s.
pub(crate) fn charge_velocity(velocity: Vec3, direction: Vec3, grounded: bool, dt: f32) -> Vec3 {
    crate::player_movement::horizontal(velocity, direction, CHARGE_SPEED * UNIT, grounded, dt)
}
/// FUN_10362630: an empty charge stops only on walls facing it more than
/// z_charge_impact_angle; a carrying charge stops on any obstacle.
pub(crate) fn wall_stops(normal: Vec3, direction: Vec3, carrying: bool) -> bool {
    normal.y.abs() < 0.7 && (carrying || normal.dot(direction).abs() > IMPACT_ANGLE)
}
/// FUN_10361880: velocity given to another survivor hit while carrying.
pub(crate) fn fling_velocity(charge_velocity: Vec3, elapsed: f32) -> Vec3 {
    let t = (elapsed / CHARGE_DURATION).clamp(0.5, 1.);
    let force = (MAX_FORCE * t).max(MIN_FORCE);
    let flat = Vec3::new(charge_velocity.x, 0., charge_velocity.z).normalize_or_zero();
    (flat + Vec3::Y * 0.9).normalize() * force * UNIT
}
/// Obstacle probe ahead of the charger (hull -15,-15,-12 .. 15,15,24 around
/// the world-space centre, 6 units alone or 24 units carrying).
pub(crate) fn probe(
    collision: &Collision,
    feet: Vec3,
    direction: Vec3,
    carrying: bool,
) -> Option<Vec3> {
    let centre = feet + Vec3::Y * (HEIGHT * 0.5 + 6. * UNIT);
    let extents = Vec3::new(15., 18., 15.) * UNIT;
    let distance = if carrying { PROBE_CARRY } else { PROBE_ALONE } * UNIT;
    let hit = collision.trace(centre, centre + direction * distance, extents);
    (!hit.start_solid && hit.fraction < 1. && wall_stops(hit.normal, direction, carrying))
        .then_some(hit.normal)
}
/// FUN_10362ed0: a survivor can be carried when a 26x26x48 hull fits 34
/// units further along the charge; otherwise it is slammed on the spot.
pub(crate) fn carry_clear(collision: &Collision, victim: Vec3, direction: Vec3) -> bool {
    let centre = victim + Vec3::Y * HEIGHT * 0.5;
    let hit = collision.trace(
        centre,
        centre + direction * (34. * UNIT),
        Vec3::new(13., 24., 13.) * UNIT,
    );
    !hit.start_solid && hit.fraction >= 1.
}
/// Horizontal swept contact between the charger hull and a survivor hull.
pub(crate) fn touches(from: Vec3, to: Vec3, survivor: Vec3) -> bool {
    if (survivor.y - to.y).abs() > HEIGHT {
        return false;
    }
    let a = from * Vec3::new(1., 0., 1.);
    let b = to * Vec3::new(1., 0., 1.);
    let p = survivor * Vec3::new(1., 0., 1.);
    let delta = b - a;
    let t = ((p - a).dot(delta) / delta.length_squared().max(1e-8)).clamp(0., 1.);
    p.distance(a + delta * t) <= 2. * RADIUS + UNIT
}
/// Pound events crossed between two cycle clocks: (vocalize, impact).
pub(crate) fn pound_events(label: &str, before: f32, after: f32) -> (bool, bool) {
    let (length, vocal, impact) = pound_clip(label);
    let crossed = |cycle: f32| {
        let at = cycle * length;
        before < at && after >= at
    };
    (crossed(vocal), crossed(impact))
}
/// Native charger.mdl pound clips: (duration, POUND_VOCALIZE, POUND_IMPACT).
pub(crate) fn pound_clip(label: &str) -> (f32, f32, f32) {
    match label {
        "Charger_pound_down_coach" => (1.667, 0.4, 0.58),
        "Charger_pound_down_producer" => (1.6, 0.41666666, 0.6041667),
        "Charger_pound_north" => (1.6, 0.375, 0.6041667),
        "Charger_pound_up" => (1.6, 0.33333334, 0.5625),
        _ => (1.6, 0.41666666, 0.6041667),
    }
}
/// Charger_pummel (ACT_TERROR_CHARGER_PUMMEL, 1.833 s) opens the pummel and
/// gives way to the pounds once its cycle passes 0.99 (server 1033f7f0 /
/// 10342280); its AE_CHARGER_POUND_SOUND (cycle 0.4) plays
/// ChargerZombie.Pummel.
pub const PUMMEL_INTRO: f32 = 1.833;
pub const PUMMEL_INTRO_END: f32 = 0.99;
pub const PUMMEL_INTRO_SOUND: f32 = 0.4;
/// 10342280: ACT_TERROR_CHARGER_POUND_UP joins the choices when something
/// lies 148-158 units above, POUND_NORTH when a wall lies 50-62 units ahead.
pub const POUND_UP_SPAN: (f32, f32) = (148., 158.);
pub const POUND_NORTH_SPAN: (f32, f32) = (50., 62.);
/// The Charger's activity for a pound clip.
pub(crate) fn pound_activity(label: &str) -> &'static str {
    match label {
        "Charger_pound_down_coach" => "ACT_TERROR_CHARGER_POUND_DOWN_COACH",
        "Charger_pound_down_producer" => "ACT_TERROR_CHARGER_POUND_DOWN_PRODUCER",
        "Charger_pound_north" => "ACT_TERROR_CHARGER_POUND_NORTH",
        "Charger_pound_up" => "ACT_TERROR_CHARGER_POUND_UP",
        _ => "ACT_TERROR_CHARGER_POUND_DOWN",
    }
}
/// The victim's pose for a pound clip (server 1033f7f0).
pub(crate) fn pounded_pose(label: &str, incap: bool) -> VictimPose {
    if incap {
        VictimPose::PoundedIncap
    } else {
        match label {
            "Charger_pound_north" => VictimPose::PoundedNorth,
            "Charger_pound_up" => VictimPose::PoundedUp,
            _ => VictimPose::Pounded,
        }
    }
}
/// Next pound (10342280): the victim's down pound, plus up/north where the
/// traces allow; with more than one choice a 50/100 roll drops the one just
/// played, then a uniform pick. `rolls` = (RandomInt(1, 100), any number).
pub(crate) fn next_pound(
    current: &str,
    victim_model: &str,
    up: bool,
    north: bool,
    rolls: (u32, u32),
) -> &'static str {
    let mut choices = vec![pound_label(victim_model)];
    if up {
        choices.push("Charger_pound_up");
    }
    if north {
        choices.push("Charger_pound_north");
    }
    if choices.len() > 1 && rolls.0 < 51 {
        if let Some(i) = choices.iter().position(|c| *c == current) {
            choices.remove(i);
        }
    }
    choices[rolls.1 as usize % choices.len()]
}
pub(crate) fn pound_label(model: &str) -> &'static str {
    if model.contains("coach") {
        "Charger_pound_down_coach"
    } else if model.contains("producer") {
        "Charger_pound_down_producer"
    } else {
        "Charger_pound_down"
    }
}

#[allow(clippy::too_many_arguments)]
fn tick(
    time: Res<Time<Fixed>>,
    collision: Res<Collision>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut charger: ResMut<Charger>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut actors: Query<(&CompanionActor, &mut Transform)>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    if !state.infected || state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    let survivors = collision.for_actor(false, false);
    let collision = collision.for_actor(true, false);
    let mut poses = vec![];
    update_flung(&mut charger, &survivors, &mut squad, dt, &mut poses);
    // `--capture-charger-charge`: spawn, stage the squad ahead and charge once.
    let capture = state.special_class == SpecialClass::Charger
        && std::env::args().any(|a| a == "--capture-charger-charge");
    if capture && hunter.ghost && !charger.capture.0 && state.elapsed > 0.3 {
        charger.capture.0 = true;
        hunter.ghost = false;
        state.special_phase = 0;
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        let side = forward.cross(Vec3::Y);
        for (i, member) in squad.0.iter_mut().enumerate() {
            member.position = body.feet + forward * 9. + side * (i as f32 * 1.5);
            member.cooldown = 10.;
        }
        body.pitch = -0.1;
    }
    if capture && charger.capture.0 && !charger.capture.1 && state.elapsed > 1. {
        charger.capture.1 = true;
        input.fire = true;
    }
    let active = super::active(&state, &hunter, SpecialClass::Charger);
    if !active {
        if charger.stage != Stage::Idle {
            end_hold(&mut charger, &mut squad);
            if state.special_class == SpecialClass::Charger && hunter.pin.is_some() {
                super::release(&mut hunter, &mut squad, &mut state);
            }
            charger.reset();
        }
        if state.special_class == SpecialClass::Charger && charger.timer.is_none() {
            charger.timer = Some(0.);
        }
        collect_getup(&charger, &mut poses);
        publish(poses);
        place_actors(&charger, &squad, &mut actors);
        return;
    }
    // A shared release (death of the victim, team switch) ends our hold.
    if matches!(charger.stage, Stage::Slam { .. } | Stage::Pummel) && hunter.pin.is_none() {
        end_hold(&mut charger, &mut squad);
        charger.stage = Stage::Idle;
        charger.timer = Some(PUMMEL_END_INTERVAL);
        state.special_phase = 0;
    }
    if charger.stage == Stage::Idle && charger.timer.is_none() {
        charger.timer = Some(0.);
    }
    if let Some(t) = &mut charger.timer {
        *t = (*t - dt).max(0.);
    }
    hunter.ability_progress = Some(charger.progress());
    let fire = std::mem::take(&mut input.fire) || input.fire_held;
    let claw = std::mem::take(&mut input.shove) || input.shove_held;
    input.reload = false;
    input.use_item = false;
    let forward = Quat::from_rotation_y(charger.yaw) * Vec3::NEG_Z;
    charger.clock += dt;
    match charger.stage {
        Stage::Idle => {
            if fire && charger.ready() && body.grounded && hunter.pin.is_none() {
                charger.stage = Stage::Charging;
                charger.clock = 0.;
                charger.yaw = body.yaw;
                charger.timer = None;
                charger.hit.clear();
                charger.sample = body.feet;
                // The first charge movement happens in the next move_player.
                charger.sample_clock = -dt;
                charger.previous = body.feet;
                hunter.pin_yaw = body.yaw;
                state.special_phase = 3;
                // Voice through the timer (FUN_10361e50 -> FUN_10347590).
                super::voice::speak(&mut hunter, &mut sounds, "ChargerZombie.Charge", 0.);
            } else if claw && hunter.cooldown <= 0. {
                let direction =
                    Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
                let eye = body.feet + Vec3::Y * crate::collision::EYE;
                let victim = squad
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.state.dead)
                    .find(|(_, m)| claw_reaches(&collision, eye, direction, m.position))
                    .map(|(i, _)| i);
                charger.swing = !charger.swing;
                // Retail: Claw.Swing every swing; a survivor hit plays
                // ChargerZombie.Smash in place of Claw.HitFlesh (103cb960).
                // ChargerZombie.Melee is never emitted by retail code.
                sounds.play("Claw.Swing", None);
                if let Some(i) = victim {
                    squad.0[i].state.damage(CLAW_DAMAGE);
                    sounds.play("ChargerZombie.Smash", Some(squad.0[i].position));
                    hunter.cooldown = CLAW_INTERVAL;
                } else if logic
                    .as_mut()
                    .is_some_and(|l| l.pound_obstacle(body.feet, direction, &collision))
                {
                    sounds.play("Claw.Hit", None);
                    hunter.cooldown = CLAW_INTERVAL;
                } else {
                    hunter.cooldown = CLAW_MISS_INTERVAL;
                }
                state.special_phase = 4;
                state.shot_serial = state.shot_serial.wrapping_add(1);
            } else if state.special_phase == 4 && hunter.cooldown <= 0. {
                state.special_phase = 0;
            }
        }
        Stage::Charging => {
            state.special_phase = 3;
            // Retail locks the view yaw for the whole charge (charger-turn.log).
            body.yaw = charger.yaw;
            body.velocity = charge_velocity(body.velocity, forward, body.grounded, dt);
            let carrying = hunter.pin;
            if let Some(i) = carrying {
                if squad.0.get(i).is_none_or(|m| m.state.dead) {
                    finish_charge(&mut charger, &mut state, CHARGE_INTERVAL);
                    charger.held = None;
                    super::release(&mut hunter, &mut squad, &mut state);
                } else {
                    squad.0[i].position = body.feet;
                    poses.push((i, VictimPose::Carried));
                }
            }
            // Survivor contacts along this tick's movement.
            let from = charger.previous;
            let contacts: Vec<usize> = squad
                .0
                .iter()
                .enumerate()
                .filter(|(i, m)| {
                    !m.state.dead
                        && !m.state.incap
                        && !m.state.pinned
                        && Some(*i) != hunter.pin
                        && !charger.hit.contains(i)
                        && touches(from, body.feet, m.position)
                })
                .map(|(i, _)| i)
                .collect();
            for i in contacts {
                if charger.stage != Stage::Charging {
                    break;
                }
                charger.hit.push(i);
                // Retail 10362ed0: grabbing plays Zombie.Punch; survivors
                // knocked aside play ChargerZombie.HitPerson.
                sounds.play(
                    if hunter.pin.is_none() {
                        "Zombie.Punch"
                    } else {
                        "ChargerZombie.HitPerson"
                    },
                    Some(squad.0[i].position),
                );
                if hunter.pin.is_none() {
                    hunter.pin = Some(i);
                    hunter.pin_yaw = charger.yaw;
                    charger.held = Some(i);
                    super::pin_member(&mut squad, i, SpecialClass::Charger);
                    if carry_clear(&collision, squad.0[i].position, forward) {
                        squad.0[i].position = body.feet;
                        poses.push((i, VictimPose::Carried));
                    } else {
                        slam(&mut charger, &mut body, &mut squad, i, true, &mut sounds);
                    }
                } else {
                    let velocity = fling_velocity(body.velocity, charger.clock);
                    squad.0[i].state.damage(IMPACT_DAMAGE);
                    charger.flung.retain(|f| f.index != i);
                    charger.getup.retain(|(g, _)| *g != i);
                    charger.flung.push(Flung {
                        index: i,
                        feet: squad.0[i].position + Vec3::Y * UNIT,
                        velocity,
                        clock: 0.,
                        landed: None,
                    });
                }
            }
            if charger.stage == Stage::Charging {
                let carrying = hunter.pin.is_some();
                if let Some(normal) = probe(&collision, body.feet, forward, carrying) {
                    sounds.play("ChargerZombie.ImpactHard", None);
                    if let Some(i) = hunter.pin {
                        slam(&mut charger, &mut body, &mut squad, i, true, &mut sounds);
                        poses.retain(|(p, _)| *p != i);
                    } else {
                        finish_charge(&mut charger, &mut state, CHARGE_INTERVAL);
                        stagger(&mut charger, &mut state, &mut body, normal, &mut sounds);
                    }
                } else if !carrying {
                    charger.sample_clock += dt;
                    if charger.sample_clock >= 0.1 - 1e-4 {
                        charger.sample_clock = 0.;
                        let moved = charger.sample.distance(body.feet) / UNIT;
                        charger.sample = body.feet;
                        if moved <= IMPACT_EPSILON {
                            finish_charge(&mut charger, &mut state, CHARGE_INTERVAL);
                            stagger(&mut charger, &mut state, &mut body, -forward, &mut sounds);
                        }
                    }
                }
            }
            if charger.stage == Stage::Charging && charger.clock >= CHARGE_DURATION && body.grounded
            {
                if let Some(i) = hunter.pin {
                    slam(&mut charger, &mut body, &mut squad, i, false, &mut sounds);
                } else {
                    finish_charge(&mut charger, &mut state, CHARGE_INTERVAL);
                    state.special_phase = 0;
                }
            }
        }
        Stage::Stagger => {
            state.special_phase = 3;
            body.velocity = charger.stagger * (STAGGER_SPEED * UNIT) + Vec3::Y * body.velocity.y;
            if charger.clock >= STAGGER_TIME {
                charger.stage = Stage::Idle;
                body.velocity = Vec3::ZERO;
                state.special_phase = 0;
            }
        }
        Stage::Slam { wall } => {
            state.special_phase = 3;
            let delay = if wall {
                WALL_PUMMEL_DELAY
            } else {
                GROUND_PUMMEL_DELAY
            };
            body.velocity = if wall {
                -forward * (STAGGER_SPEED * UNIT) + Vec3::Y * body.velocity.y
            } else {
                Vec3::Y * body.velocity.y
            };
            if let Some(i) = hunter.pin {
                squad.0[i].position = charger.slam_point;
                poses.push((
                    i,
                    if wall {
                        VictimPose::SlammedWall
                    } else {
                        VictimPose::SlammedGround
                    },
                ));
                if charger.clock >= delay {
                    charger.stage = Stage::Pummel;
                    charger.clock = 0.;
                    charger.pound = 0.;
                    charger.pound_label = pound_label(&squad.0[i].model);
                    body.velocity = Vec3::ZERO;
                }
            }
        }
        Stage::Pummel => {
            state.special_phase = 3;
            body.velocity = Vec3::Y * body.velocity.y;
            if let Some(i) = hunter.pin {
                let member = &mut squad.0[i];
                // Retail moves the victim onto the charger origin (~0.4s).
                member.position = member.position.lerp(body.feet, (dt * 8.).min(1.));
                poses.push((
                    i,
                    if member.state.incap {
                        VictimPose::PoundedIncap
                    } else {
                        VictimPose::Pounded
                    },
                ));
                let (length, _, _) = pound_clip(charger.pound_label);
                let before = charger.pound;
                let after = before + dt;
                let (vocal, impact) = pound_events(charger.pound_label, before, after);
                charger.pound = if after >= length {
                    after - length
                } else {
                    after
                };
                if vocal {
                    sounds.play("ChargerZombie.VocalizePummel", None);
                }
                if impact {
                    member.state.damage(POUND_DAMAGE);
                    state.shot_serial = state.shot_serial.wrapping_add(1);
                    sounds.play("ChargerZombie.Pummel", Some(member.position));
                }
                if member.state.dead {
                    super::release(&mut hunter, &mut squad, &mut state);
                    charger.held = None;
                    charger.stage = Stage::Idle;
                    charger.timer = Some(PUMMEL_END_INTERVAL);
                    state.special_phase = 0;
                }
            }
        }
    }
    charger.previous = body.feet;
    hunter.ability_progress = Some(charger.progress());
    // Retail charge view: chase camera behind the Charger looking along the
    // locked charge direction (charger-retail-charge-view.jpg). Slam and
    // pummel keep the orbit view.
    hunter.chase_camera = charger.stage == Stage::Charging;
    collect_getup(&charger, &mut poses);
    publish(poses);
    place_actors(&charger, &squad, &mut actors);
}
/// Claw reach: claw_range from the eye to the survivor hull, inside a cone.
fn claw_reaches(collision: &Collision, eye: Vec3, direction: Vec3, feet: Vec3) -> bool {
    let top = feet + Vec3::Y * HEIGHT;
    let t = ((eye - feet).dot(Vec3::Y) / HEIGHT).clamp(0., 1.);
    let closest = feet.lerp(top, t);
    let delta = closest - eye;
    let horizontal = (delta * Vec3::new(1., 0., 1.)).length();
    (horizontal - RADIUS).max(0.) <= CLAW_RANGE * UNIT
        && (delta.y.abs() <= CLAW_RANGE * UNIT)
        && direction.dot(delta.normalize_or_zero()) > 0.5
        && collision.trace(eye, closest, Vec3::ZERO).fraction >= 1.
}
fn finish_charge(charger: &mut Charger, state: &mut Match, interval: f32) {
    charger.stage = Stage::Idle;
    charger.timer = Some(interval);
    state.special_phase = 0;
}
fn stagger(
    charger: &mut Charger,
    state: &mut Match,
    body: &mut Body,
    away: Vec3,
    sounds: &mut crate::game_audio::Queue,
) {
    charger.stage = Stage::Stagger;
    charger.clock = 0.;
    charger.stagger = (away * Vec3::new(1., 0., 1.)).normalize_or_zero();
    body.velocity = Vec3::Y * body.velocity.y;
    state.special_phase = 3;
    sounds.play("ChargerZombie.Stagger", None);
}
/// FUN_10331590: slam the carried survivor (wall: charger staggers back and
/// pummels after 0.75s; ground/timeout: pummel after 1.0s).
fn slam(
    charger: &mut Charger,
    body: &mut Body,
    squad: &mut Squad,
    victim: usize,
    wall: bool,
    sounds: &mut crate::game_audio::Queue,
) {
    if !wall {
        sounds.play("ChargerZombie.ImpactHard", None);
    } else {
        sounds.play("ChargerZombie.Stagger", None);
    }
    charger.stage = Stage::Slam { wall };
    charger.clock = 0.;
    charger.timer = None;
    charger.slam_point = body.feet;
    body.velocity = Vec3::Y * body.velocity.y;
    if let Some(member) = squad.0.get_mut(victim) {
        member.position = body.feet;
        member.state.damage(SLAM_DAMAGE);
    }
}
/// Releases poses held by an interrupted charge/pummel; a surviving,
/// standing victim plays the native get-up.
fn end_hold(charger: &mut Charger, squad: &mut Squad) {
    if let Some(i) = charger.held.take() {
        if squad
            .0
            .get(i)
            .is_some_and(|m| !m.state.dead && !m.state.incap)
        {
            charger.getup.retain(|(g, _)| *g != i);
            charger.getup.push((i, GETUP_TIME));
        }
    }
}
fn update_flung(
    charger: &mut Charger,
    collision: &Collision,
    squad: &mut Squad,
    dt: f32,
    poses: &mut Vec<(usize, VictimPose)>,
) {
    for (_, left) in &mut charger.getup {
        *left -= dt;
    }
    charger.getup.retain(|(_, left)| *left > 0.);
    let mut landed = vec![];
    for f in &mut charger.flung {
        let Some(member) = squad.0.get_mut(f.index) else {
            continue;
        };
        f.clock += dt;
        if f.landed.is_none() {
            f.velocity.y -= crate::player_movement::GRAVITY * dt;
            let (next, velocity) = collision.walk_velocity(f.feet, f.velocity, dt, HEIGHT, false);
            f.velocity = velocity;
            f.feet = next;
            if (f.velocity.y <= 0. && collision.grounded(f.feet, HEIGHT)) || f.clock > 4. {
                f.landed = Some(0.);
                landed.push(f.index);
            }
        }
        member.position = f.feet;
        member.cooldown = member.cooldown.max(0.2);
        poses.push((f.index, VictimPose::Flung));
    }
    charger.flung.retain(|f| f.landed.is_none());
    for i in landed {
        charger.getup.push((i, GETUP_TIME));
    }
    for (i, _) in &charger.getup {
        if let Some(member) = squad.0.get_mut(*i) {
            member.cooldown = member.cooldown.max(0.2);
        }
    }
}
fn collect_getup(charger: &Charger, poses: &mut Vec<(usize, VictimPose)>) {
    for (i, _) in &charger.getup {
        if !poses.iter().any(|(p, _)| p == i) {
            poses.push((*i, VictimPose::GetUp));
        }
    }
}
fn place_actors(
    charger: &Charger,
    squad: &Squad,
    actors: &mut Query<(&CompanionActor, &mut Transform)>,
) {
    for (actor, mut transform) in actors.iter_mut() {
        let Some(member) = squad.0.get(actor.0) else {
            continue;
        };
        let held = member.state.pinned_by == Some(SpecialClass::Charger);
        if held || charger.flung.iter().any(|f| f.index == actor.0) {
            transform.translation = member.position;
        }
        if held {
            // The survivor Charger victim sequences are authored facing the
            // Charger: Charger_Carried holds the body ~26 units behind its
            // root, Charger_Slammed_Ground ~60, and Charger_pounded_down lays
            // it 13-43 units behind and 20 to the left. Turned to face the
            // Charger, the carried survivor sits in its arm, the slammed one
            // lands where Charger_slam_ground's hand strikes (+52 forward) and
            // the pounded one lies under the right fist of Charger_pound_down
            // (+20 forward, -25 right). Same facing for every held stage.
            transform.rotation = Quat::from_rotation_y(
                charger.yaw + std::f32::consts::FRAC_PI_2 + std::f32::consts::PI,
            );
        }
    }
}

/// World Charger sequence and time for the current third-person stage.
/// Pose parameters for a world sequence. Charger_Charge is a 9-way
/// move_x/move_y locomotion blend: the charge runs straight ahead (move_x 1),
/// not the grid centre, which is the in-place blend.
fn world_params(label: &str) -> &'static [(&'static str, f32)] {
    if label == "Charger_Charge" {
        &[("move_x", 1.), ("move_y", 0.)]
    } else {
        &[]
    }
}
fn world_sequence(charger: &Charger) -> (&'static str, f32) {
    match charger.stage {
        Stage::Charging => ("Charger_Charge", charger.clock % CHARGE_CLIP),
        Stage::Stagger | Stage::Slam { wall: true } => ("Shoved_Backward", charger.clock),
        Stage::Slam { wall: false } => ("Charger_slam_ground", charger.clock),
        Stage::Pummel => (charger.pound_label, charger.pound),
        Stage::Idle => ("Idle_Upper_KNIFE", 0.),
    }
}
#[derive(Component)]
struct ChargerDraw(usize);
#[allow(clippy::too_many_arguments)]
fn world_model(
    mut commands: Commands,
    state: Res<Match>,
    charger: Res<Charger>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut draws: Query<(Entity, &ChargerDraw, &Mesh3d, &mut Transform)>,
) {
    let visible = state.infected_third_person() && state.special_class == SpecialClass::Charger;
    if !visible {
        for (entity, ..) in &draws {
            commands.entity(entity).despawn();
        }
        return;
    }
    let (label, time) = world_sequence(&charger);
    let transform = Transform::from_translation(body.feet).with_rotation(Quat::from_rotation_y(
        charger.yaw + std::f32::consts::FRAC_PI_2,
    ));
    if draws.is_empty() {
        if let Some(parts) = models.0.get(&(MODEL.into(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.scripted_params(MODEL, MODEL, index, label, time, world_params(label), &mut owned);
                commands.spawn((
                    ChargerDraw(index),
                    Mesh3d(meshes.add(owned)),
                    MeshMaterial3d(material.clone()),
                    transform,
                    bevy::camera::visibility::RenderLayers::layer(1),
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
                library.scripted_params(MODEL, MODEL, draw.0, label, time, world_params(label), &mut mesh);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_pound_choice_uses_the_victims_clip_and_walls_or_ceilings() {
        assert_eq!(next_pound("", "models/survivors/survivor_coach.mdl", false, false, (1, 0)), "Charger_pound_down_coach");
        assert_eq!(next_pound("", "producer", false, false, (99, 7)), "Charger_pound_down_producer");
        assert_eq!(next_pound("", "models/survivors/survivor_gambler.mdl", false, false, (1, 0)), "Charger_pound_down");
        // A wall ahead adds POUND_NORTH; a low roll drops the clip just played.
        assert_eq!(next_pound("Charger_pound_down", "gambler", false, true, (10, 0)), "Charger_pound_north");
        assert_eq!(next_pound("Charger_pound_down", "gambler", true, true, (80, 1)), "Charger_pound_up");
        assert_eq!(pound_activity("Charger_pound_north"), "ACT_TERROR_CHARGER_POUND_NORTH");
        assert!(matches!(pounded_pose("Charger_pound_up", false), VictimPose::PoundedUp));
        assert!(matches!(pounded_pose("Charger_pound_up", true), VictimPose::PoundedIncap));
        // Native clip events: Charger_pound_up vocalizes at 0.333, hits at 0.5625.
        assert_eq!(pound_events("Charger_pound_up", 0.85, 0.95), (false, true));
    }
    use crate::game_state::Companion;
    use bevy::ecs::system::RunSystemOnce;

    fn member(position: Vec3, model: &str) -> Companion {
        Companion {
            state: Match::new(&crate::game_state::tests_weapon()),
            position,
            spawn: position,
            model: model.into(),
            vertical: 0.,
            cooldown: 10.,
            revive_time: 0.,
            aim: Vec3::X,
        }
    }
    fn app(collision: Collision, squad: Vec<Companion>) -> App {
        let mut app = App::new();
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        state.paused = false;
        state.special_class = SpecialClass::Charger;
        state.health = HEALTH;
        let mut fixed = Time::<Fixed>::from_hz(30.);
        fixed.advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        app.insert_resource(fixed)
            .insert_resource(collision)
            .insert_resource(state)
            .insert_resource(Hunter::default())
            .insert_resource(Charger {
                timer: Some(0.),
                ..default()
            })
            .insert_resource(crate::game::Input::default())
            .insert_resource(Squad(squad))
            .insert_resource(crate::game_audio::Queue::default());
        app.world_mut().spawn(Body::at(Vec3::ZERO));
        app
    }
    fn step(app: &mut App) {
        app.world_mut().run_system_once(tick).unwrap();
        // Integrate the kept velocity like move_player does for phase 3.
        app.world_mut().resource_scope(|world, c: Mut<Collision>| {
            let c = c.for_actor(true, false);
            let mut q = world.query::<&mut Body>();
            let mut body = q.single_mut(world).unwrap();
            let v = Vec3::new(body.velocity.x, 0., body.velocity.z);
            let (next, velocity) = c.walk_velocity(body.feet, v, 1. / 30., HEIGHT, true);
            body.feet = next;
            body.velocity = velocity;
        });
    }
    fn body(app: &mut App) -> Vec3 {
        let mut q = app.world_mut().query::<&Body>();
        q.single(app.world()).unwrap().feet
    }
    fn sounds(app: &App) -> Vec<String> {
        app.world()
            .resource::<crate::game_audio::Queue>()
            .0
            .iter()
            .map(|r| r.name.clone())
            .collect()
    }

    #[test]
    fn charge_ramp_matches_retail_trace() {
        // charger-turn.log: 83.33, 155.5, 218.1, 272.4, 319.4 ... capped at 500.
        let mut v = Vec3::ZERO;
        let mut speeds = vec![];
        for _ in 0..14 {
            v = charge_velocity(v, Vec3::NEG_Z, true, 1. / 30.);
            speeds.push(v.length() / UNIT);
        }
        for (got, want) in speeds.iter().zip([83.33, 155.55, 218.14, 272.39, 319.41]) {
            assert!((got - want).abs() < 0.2, "{got} vs {want}");
        }
        assert!((speeds[13] - 500.).abs() < 0.01);
        assert!(speeds.windows(2).all(|w| w[1] >= w[0]));
    }
    #[test]
    fn impact_rules_follow_retail_probe_and_force() {
        // A glancing wall only stops a carrying charge.
        let glancing = Vec3::new(0.6, 0., 0.8);
        assert!(!wall_stops(glancing, Vec3::X, false));
        assert!(wall_stops(glancing, Vec3::X, true));
        assert!(wall_stops(Vec3::NEG_X, Vec3::X, false));
        assert!(!wall_stops(Vec3::Y, Vec3::X, true));
        // Early hits use z_charge_min_force, full duration z_charge_max_force.
        let early = fling_velocity(Vec3::X, 0.2) / UNIT;
        let late = fling_velocity(Vec3::X, 2.5) / UNIT;
        assert!((early.length() - MIN_FORCE).abs() < 0.01);
        assert!((late.length() - MAX_FORCE).abs() < 0.01);
        assert!((early.y / early.x - 0.9).abs() < 1e-4);
        assert!(touches(Vec3::ZERO, Vec3::X * 0.5, Vec3::X * 1.2));
        assert!(!touches(Vec3::ZERO, Vec3::X * 0.5, Vec3::new(0.5, 0., 1.2)));
    }
    #[test]
    fn pummel_pounds_on_native_impact_events() {
        let mut clock = 0.;
        let mut impacts = 0;
        let mut vocals = 0;
        for _ in 0..(30 * 16) {
            let (vocal, impact) = pound_events("Charger_pound_down", clock, clock + 1. / 30.);
            impacts += impact as u32;
            vocals += vocal as u32;
            clock += 1. / 30.;
            if clock >= 1.6 {
                clock -= 1.6;
            }
        }
        assert_eq!(impacts, 10);
        assert_eq!(vocals, 10);
        assert_eq!(
            pound_label("models/survivors/survivor_coach.mdl"),
            "Charger_pound_down_coach"
        );
    }
    #[test]
    fn charge_grabs_first_survivor_flings_second_and_slams_wall() {
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(-10., 0., -12.), Vec3::new(10., 5., -11.5));
        let squad = vec![
            member(
                Vec3::new(0., 0., -3.),
                "models/survivors/survivor_coach.mdl",
            ),
            member(
                Vec3::new(0.3, 0., -6.),
                "models/survivors/survivor_gambler.mdl",
            ),
        ];
        let mut app = app(collision, squad);
        app.world_mut().resource_mut::<crate::game::Input>().fire = true;
        step(&mut app);
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Charging);
        assert_eq!(app.world().resource::<Match>().special_phase, 3);
        assert!(sounds(&app).contains(&"ChargerZombie.Charge".to_string()));
        let mut grabbed = false;
        let mut flung = false;
        for _ in 0..90 {
            step(&mut app);
            let squad = app.world().resource::<Squad>();
            grabbed |= squad.0[0].state.pinned_by == Some(SpecialClass::Charger);
            flung |= app
                .world()
                .resource::<Charger>()
                .flung
                .iter()
                .any(|f| f.index == 1)
                || squad.0[1].state.health < 100.;
            if app.world().resource::<Charger>().stage != Stage::Charging {
                break;
            }
        }
        assert!(grabbed && flung);
        let squad = app.world().resource::<Squad>();
        assert_eq!(squad.0[1].state.health, 100. - IMPACT_DAMAGE);
        assert!(!squad.0[1].state.pinned);
        assert!(matches!(
            app.world().resource::<Charger>().stage,
            Stage::Slam { wall: true }
        ));
        assert_eq!(squad.0[0].state.health, 100. - SLAM_DAMAGE);
        let z = body(&mut app).z;
        assert!(z > -11.5 && z < -10., "{z}");
        // Pummel starts after the 0.75s wall delay and pounds every 1.667s
        // (Coach clip) for z_charger_pound_dmg; shoves cannot rescue.
        for _ in 0..(30 + 30 * 5) {
            step(&mut app);
        }
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Pummel);
        let health = app.world().resource::<Squad>().0[0].state.health;
        assert_eq!(health, 100. - SLAM_DAMAGE - 3. * POUND_DAMAGE);
        assert!(app.world().resource::<Hunter>().ability_progress == Some(0.));
        // The victim dies: pummel ends, 0.2s activation timer.
        {
            let mut squad = app.world_mut().resource_mut::<Squad>();
            squad.0[0].state.dead = true;
        }
        step(&mut app);
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Idle);
        assert_eq!(app.world().resource::<Match>().special_phase, 0);
        assert!(app.world().resource::<Hunter>().pin.is_none());
        assert!(!app.world().resource::<Squad>().0[0].state.pinned);
    }
    #[test]
    fn empty_charge_hits_wall_staggers_and_starts_interval() {
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(-10., 0., -6.), Vec3::new(10., 5., -5.5));
        let mut app = app(collision, vec![]);
        app.world_mut().resource_mut::<crate::game::Input>().fire = true;
        let mut stage = Stage::Charging;
        for _ in 0..60 {
            step(&mut app);
            stage = app.world().resource::<Charger>().stage;
            if stage != Stage::Charging {
                break;
            }
        }
        assert_eq!(stage, Stage::Stagger);
        assert!(sounds(&app).contains(&"ChargerZombie.ImpactHard".to_string()));
        assert!(sounds(&app).contains(&"ChargerZombie.Stagger".to_string()));
        let charger = app.world().resource::<Charger>();
        assert!(
            charger
                .timer
                .is_some_and(|t| (t - CHARGE_INTERVAL).abs() < 0.05)
        );
        for _ in 0..30 {
            step(&mut app);
        }
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Idle);
        assert_eq!(app.world().resource::<Match>().special_phase, 0);
        let progress = app.world().resource::<Hunter>().ability_progress.unwrap();
        assert!(progress > 0. && progress < 0.15);
        // Still cooling down: a new click does nothing.
        app.world_mut().resource_mut::<crate::game::Input>().fire = true;
        step(&mut app);
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Idle);
    }
    #[test]
    fn charge_times_out_in_the_open_after_retail_duration() {
        let mut app = app(Collision::test_solid_floor(), vec![]);
        app.world_mut().resource_mut::<crate::game::Input>().fire = true;
        {
            let mut q = app.world_mut().query::<&mut Body>();
            q.single_mut(app.world_mut()).unwrap().feet.z = 14.;
        }
        let mut ticks = 0;
        for _ in 0..120 {
            step(&mut app);
            if app.world().resource::<Charger>().stage != Stage::Charging {
                break;
            }
            ticks += 1;
        }
        assert!(
            (ticks as f32 / 30. - CHARGE_DURATION).abs() < 0.05,
            "{ticks}"
        );
        assert_eq!(app.world().resource::<Charger>().stage, Stage::Idle);
        // ~ (2.5s at up to 500 u/s minus the ramp).
        let travelled = (14. - body(&mut app).z) / UNIT;
        assert!(travelled > 1100. && travelled < 1250., "{travelled}");
    }
    #[test]
    fn carried_victim_against_wall_is_slammed_immediately() {
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(-10., 0., -3.9), Vec3::new(10., 5., -3.4));
        let squad = vec![member(
            Vec3::new(0., 0., -3.),
            "models/survivors/survivor_producer.mdl",
        )];
        let mut app = app(collision, squad);
        app.world_mut().resource_mut::<crate::game::Input>().fire = true;
        for _ in 0..40 {
            step(&mut app);
            if app.world().resource::<Charger>().stage != Stage::Charging {
                break;
            }
        }
        assert!(matches!(
            app.world().resource::<Charger>().stage,
            Stage::Slam { wall: true }
        ));
        assert_eq!(
            app.world().resource::<Squad>().0[0].state.pinned_by,
            Some(SpecialClass::Charger)
        );
    }
    #[test]
    fn claw_uses_retail_damage_and_intervals() {
        let squad = vec![member(
            Vec3::new(0., 0., -1.2),
            "models/survivors/survivor_coach.mdl",
        )];
        let mut app = app(Collision::test_solid_floor(), squad);
        app.world_mut().resource_mut::<crate::game::Input>().shove = true;
        step(&mut app);
        assert_eq!(
            app.world().resource::<Squad>().0[0].state.health,
            100. - CLAW_DAMAGE
        );
        assert_eq!(app.world().resource::<Match>().special_phase, 4);
        assert_eq!(app.world().resource::<Hunter>().cooldown, CLAW_INTERVAL);
        assert!(sounds(&app).contains(&"ChargerZombie.Smash".to_string()));
        assert_eq!(vm_activity(3), Some("ACT_VM_IDLE"));
        assert_eq!(vm_activity(4), None);
    }
    #[test]
    fn charge_body_runs_forward_in_its_locomotion_blend() {
        // Charger_Charge is a 9-way move_x/move_y blend: its centre is the
        // in-place pose, so the charge must sample full forward.
        assert_eq!(world_params("Charger_Charge"), &[("move_x", 1.), ("move_y", 0.)]);
        assert!(world_params("Charger_slam_ground").is_empty());
    }
    #[test]
    fn owned_charger_assets_activities_and_sounds_exist() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [MODEL, VIEW, "materials/vgui/hud/pz_charge_charger.vtf"] {
            assert!(!assets.read(path).unwrap().is_empty(), "{path}");
        }
        let script =
            String::from_utf8(assets.read("scripts/weapon_charger_claw.txt").unwrap()).unwrap();
        assert!(script.contains("\"MaxPlayerSpeed\"\t\t\"250\""));
        assert!(script.contains(VIEW));
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "chargerzombie.alert",
            "chargerzombie.death",
            "chargerzombie.growl",
            "chargerzombie.pain",
            "chargerzombie.charge",
            "chargerzombie.hitperson",
            "chargerzombie.impacthard",
            "chargerzombie.stagger",
            "chargerzombie.pummel",
            "chargerzombie.vocalizepummel",
            "chargerzombie.melee",
            "chargerzombie.smash",
            "claw.swing",
            "claw.hit",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty(), "{name}");
        }
        let assets = std::cell::RefCell::new(assets);
        let load = |path: &str| {
            l4d2_formats::anim::Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok())
                .unwrap()
        };
        let view = load(VIEW);
        for activity in [
            "ACT_VM_DEPLOY",
            "ACT_VM_IDLE",
            "ACT_VM_PRIMARYATTACK",
            "ACT_VM_SHOOT",
        ] {
            assert!(
                view.sequences().iter().any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
        let world = load(MODEL);
        let duration = |label: &str| {
            let seq = world.find_sequence(label).expect(label);
            world
                .sequence_anim(seq)
                .map_or(0., |a| world.anims()[a].duration())
        };
        assert!((duration("Charger_Charge") - CHARGE_CLIP).abs() < 0.01);
        for label in [
            "Charger_pound_down",
            "Charger_pound_down_coach",
            "Charger_pound_down_producer",
        ] {
            let (length, vocal, impact) = pound_clip(label);
            assert!((duration(label) - length).abs() < 0.01, "{label}");
            let seq = &world.sequences()[world.find_sequence(label).unwrap()];
            let event = |name: &str| seq.events.iter().find(|e| e.options == name).unwrap().cycle;
            assert!((event("AE_CHARGER_POUND_VOCALIZE") - vocal).abs() < 1e-4);
            assert!((event("AE_CHARGER_POUND_IMPACT") - impact).abs() < 1e-4);
        }
        for label in ["Shoved_Backward", "Charger_slam_ground"] {
            assert!(world.find_sequence(label).is_some(), "{label}");
        }
        for survivor in ["gambler", "coach", "producer", "mechanic"] {
            let sk = load(&format!("models/survivors/survivor_{survivor}.mdl"));
            for pose in [
                VictimPose::Carried,
                VictimPose::SlammedWall,
                VictimPose::SlammedGround,
                VictimPose::Pounded,
                VictimPose::PoundedIncap,
                VictimPose::Flung,
                VictimPose::GetUp,
            ] {
                let mut activity = String::new();
                let mut clock = 0.;
                assert!(
                    victim_sequence_for(&sk, pose, &mut activity, &mut clock).is_some(),
                    "{survivor} {pose:?}"
                );
            }
            let land = sk.find_sequence("ACT_TERROR_CHARGERHIT_LAND_SLOW").unwrap();
            let length = sk
                .sequence_anim(land)
                .map_or(0., |a| sk.anims()[a].duration());
            assert!((length - GETUP_TIME).abs() < 0.01);
            // Carry lead-in then the held carry pose.
            let mut activity = String::new();
            let mut clock = 0.;
            victim_sequence_for(&sk, VictimPose::Carried, &mut activity, &mut clock);
            assert_eq!(activity, "charger_hit");
            clock = 0.5;
            let seq =
                victim_sequence_for(&sk, VictimPose::Carried, &mut activity, &mut clock).unwrap();
            assert_eq!(activity, "charger_carried");
            assert_eq!(sk.sequences()[seq].activity_name, "ACT_TERROR_CARRIED");
            // The carried body sits behind the survivor root (-X), which is
            // why place_actors turns a carried survivor to face the Charger.
            let pose = sk.pose_sequence(seq, 0.5, true);
            let pelvis = sk
                .bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case("ValveBiped.Bip01_Pelvis"))
                .unwrap();
            assert!(pose[pelvis].w_axis.x < -15., "{survivor}");
            // Slam and pummel bodies also lie behind the survivor root, so the
            // held survivor faces the Charger in every stage (place_actors).
            for (label, time) in [
                ("Charger_Slammed_Ground", 0.6),
                ("Charger_pounded_down", 0.),
                ("IncapFrom_Charger", 0.5),
            ] {
                let seq = sk.find_sequence(label).expect(label);
                let pose = sk.pose_sequence(seq, time, true);
                assert!(pose[pelvis].w_axis.x < -10., "{survivor} {label}");
            }
        }
    }
}
