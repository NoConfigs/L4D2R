//! l4d2r continuous studio animation adapted to Bevy's native Source meshes.
use crate::{
    binary,
    game::{CompanionActor, Enemy, HeldWeapon},
    game_state::{Match, Rules, Squad},
    source_assets::SourceAssets,
    studio::ModelMesh,
    viewer::RenderModels,
};
use bevy::prelude::*;
use bevy::{camera::primitives::MeshAabb, camera::visibility::NoFrustumCulling};
use bevy::{
    camera::visibility::DynamicSkinnedMeshBounds,
    mesh::{
        VertexAttributeValues,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
};
use l4d2_formats::anim::Skeleton;
use l4d2r_math::{Mat4 as XMat, Vec3 as XVec};
use std::{cell::RefCell, collections::HashMap, io, path::Path};
struct Draw {
    mesh: ModelMesh,
    // Bind geometry bone names may come from the arm model, rather than the weapon.
    bones: Vec<BindBone>,
}
struct BindBone {
    name: String,
    parent: i32,
    local: XMat,
    inverse: XMat,
}
struct Model {
    skeleton: Skeleton,
    draws: Vec<Draw>,
    physics: Option<crate::physics_data::Profile>,
}
pub(crate) struct FrozenGorePose {
    pose: Vec<XMat>,
    previous: Option<(Vec<XMat>, Transform, f32)>,
}
#[derive(Resource, Default)]
pub struct AnimationLibrary {
    models: HashMap<String, std::sync::Arc<Model>>,
    surfaces: crate::physics_data::Surfaces,
    pub(crate) gore: crate::infected_gore::Registry,
}
#[derive(Component)]
pub struct AnimatedDraw {
    pub(crate) root: Entity,
    pub(crate) model: String,
    pub(crate) draw: usize,
}
#[derive(Component)]
pub(crate) struct GpuBone;
/// A common infected body draw that shares one GPU-skinned mesh with every
/// other common of the same model and draw. Their bind-pose data is identical
/// and the per-entity joints do the posing, so a private copy per infected
/// only cost memory and a GPU upload on every spawn. Gore takes a private
/// copy before cutting wounds into it (`take_private_skin`).
#[derive(Component)]
pub(crate) struct SharedSkin;
#[derive(Default)]
struct SharedSkins {
    /// Source template mesh -> the shared skinned copy.
    by_source: HashMap<AssetId<Mesh>, Handle<Mesh>>,
    /// Shared copies whose skinning attributes are already set.
    ready: std::collections::HashSet<AssetId<Mesh>>,
}
static SHARED_SKINS: std::sync::LazyLock<std::sync::Mutex<SharedSkins>> =
    std::sync::LazyLock::new(Default::default);
/// Gives a shared common-infected body draw its own mesh copy, so per-entity
/// edits (wound cutouts) don't change every other infected's body.
pub(crate) fn take_private_skin(
    commands: &mut Commands,
    entity: Entity,
    mesh: &mut Mesh3d,
    meshes: &mut Assets<Mesh>,
) {
    if let Some(copy) = meshes.get(&mesh.0).cloned() {
        mesh.0 = meshes.add(copy);
    }
    commands.entity(entity).remove::<SharedSkin>();
}
#[derive(Component)]
pub(crate) struct GpuSkin {
    joints: Vec<Entity>,
}
fn render_bone(matrix: XMat) -> Mat4 {
    let c = XMat::from_cols(
        l4d2r_math::Vec4::new(0.0254, 0., 0., 0.),
        l4d2r_math::Vec4::new(0., 0., -0.0254, 0.),
        l4d2r_math::Vec4::new(0., 0.0254, 0., 0.),
        l4d2r_math::Vec4::W,
    );
    Mat4::from_cols_array(&(c * matrix * c.inverse()).to_cols_array())
}
fn merged_bones(draw: &Draw, pose: &HashMap<String, XMat>) -> Vec<XMat> {
    let mut bones = Vec::with_capacity(draw.bones.len());
    for b in &draw.bones {
        bones.push(pose.get(&b.name).copied().unwrap_or_else(|| {
            if b.parent < 0 {
                b.local
            } else {
                bones[b.parent as usize] * b.local
            }
        }));
    }
    bones
}
fn gpu_weights(draw: &Draw) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
    let mut indices = Vec::with_capacity(draw.mesh.weights.len());
    let mut weights = Vec::with_capacity(draw.mesh.weights.len());
    for (ids, values) in &draw.mesh.weights {
        let mut w = [0.; 4];
        let mut i = [0; 4];
        for k in 0..3 {
            if (ids[k] as usize) < draw.bones.len() && values[k] > 0. {
                i[k] = ids[k] as u16;
                w[k] = values[k];
            }
        }
        let sum = w.iter().sum::<f32>();
        if sum > 0. {
            for v in &mut w {
                *v /= sum;
            }
        } else {
            i[0] = draw.bones.len() as u16;
            w[0] = 1.;
        }
        indices.push(i);
        weights.push(w);
    }
    (indices, weights)
}
#[derive(Default)]
struct GoreMotion {
    clock: f32,
    sequence: usize,
    pose: Vec<XMat>,
    root: Transform,
}
#[derive(Default)]
struct Playback {
    ik: crate::studio_ik::Context,
    ragdoll: Option<crate::ragdoll::Body>,
    reaction_serial: u64,
    attack_serial: u64,
    last_pose: Vec<XMat>,
    gore_history: std::collections::VecDeque<GoreMotion>,
    gore_clock: f32,
    transition_pose: Vec<XMat>,
    transition_time: f32,
    transition_duration: f32,
    motion_velocity: Vec3,
    locomotion_sequence: usize,
    locomotion_clock: f32,
    sequence: usize,
    clock: f32,
    /// Viewmodel idle (m_nSequence) state: cycle and its move_x / ver_aims
    /// pose parameters (client 102880d0).
    view_idle_cycle: f32,
    view_move_x: f32,
    view_ver_aims: f32,
    /// A common died with its pelvis under 24 units (10415d80): sitting,
    /// lying or crawling deaths go limp. Decided on the first dead frame.
    death_low: Option<bool>,
    position: Vec3,
    simulation_clock: f32,
    motion_speed: f32,
    initialized: bool,
    ammo: u32,
    reload: bool,
    reload_remaining: f32,
    cooldown: f32,
    activity: String,
    shove_serial: u64,
    shot_serial: u64,
    /// Last `Match::damage_serial` seen (survivor flinch trigger).
    damage_serial: u64,
    /// Survivor calm fidget: own clock, next fidget time, seconds calm and
    /// fidgets played (seeds the interval and variant).
    fidget_clock: f32,
    /// Item use or revive gesture in slot 0: (activity, cut when the action
    /// ends, action time left when it was last seen).
    medical: Option<(&'static str, bool, f32)>,
    fidget_at: f32,
    calm: f32,
    fidgets: u64,
    reload_phase: u8,
    throw_stage: u8,
    audit_clock: f32,
    item_use_serial: u64,
    /// Survivor bots: retail facing/aim/locomotion state.
    survivor: SurvivorAnimState,
    /// Gesture slots (survivors, AI Tank punch), layered over the main sequence.
    gestures: [Gesture; GESTURE_SLOTS],
    /// Last `Enemy::attack_serial` that restarted an AI gesture.
    gesture_serial: u64,
    /// Common infected: eased lean_yaw and the previous velocity it is
    /// derived from (ZombieBotLocomotion lean).
    lean_yaw: f32,
    lean_velocity: Vec3,
}
/// Survivor gesture slots (server CTerrorPlayerAnimState::DoAnimationEvent
/// 1033cb40): firing, reloads, shoves and item use share slot 0; grenade
/// throws use 1, the jump landing 2, the client ladder idle 3 and flinches 4.
const GESTURE_ATTACK: usize = 0;
const GESTURE_JUMP: usize = 2;
const GESTURE_FLINCH: usize = 4;
const GESTURE_SLOTS: usize = 7;
/// A new flinch is ignored until the current one passes this cycle (server
/// 1033cb40, DAT_105d4b44).
const FLINCH_RESTART_CYCLE: f32 = 0.5;
/// Survivor calm fidget (server CTerrorPlayerAnimState 1033dc80): every
/// RandomInt(fidget_min_interval 10, fidget_max_interval 20) s, a survivor
/// calm for 4 s (DAT_105d3560) that is idle or walking sends animation event
/// 0x62, which plays ACT_TERROR_FIDGET in gesture slot 4 (1033cb40).
const FIDGET_INTERVAL: [u64; 2] = [10, 20];
const FIDGET_CALM: f32 = 4.;
fn fidget_interval(seed: u64) -> f32 {
    let h = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
    (FIDGET_INTERVAL[0] + h % (FIDGET_INTERVAL[1] - FIDGET_INTERVAL[0] + 1)) as f32
}
/// Item use gestures (server 1033cb40). Events 0x2a-0x2d (first aid on
/// self, a friend, an incapacitated friend, one hanging above), 0x32
/// (defibrillator) and 0x34 (upgrade pack) restart slot 0 with the crouched
/// activity while the main activity is ACT_CROUCHIDLE or ACT_RUN_CROUCH, and
/// slot 0 is reset when the action finishes or is cancelled (events 0x2e,
/// 0x36, 0x38). Pills and adrenaline come through the weapon's fire event
/// (0x20) and play out.
const USE_REVIVE: &str = "ACT_TERROR_HEAL_INCAPACITATED";
fn use_gesture_activity(standing: &'static str, crouched: bool) -> (&'static str, bool) {
    const PAIRS: [(&str, &str, bool); 6] = [
        ("ACT_TERROR_HEAL_SELF", "ACT_TERROR_CROUCH_HEAL_SELF", true),
        ("ACT_TERROR_HEAL_FRIEND", "ACT_TERROR_CROUCH_HEAL_FRIEND", true),
        (USE_REVIVE, "ACT_TERROR_CROUCH_HEAL_INCAPACITATED", true),
        ("ACT_TERROR_USE_DEFIBRILLATOR", "ACT_TERROR_CROUCH_USE_DEFIBRILLATOR", true),
        ("ACT_TERROR_USE_ADRENALINE", "ACT_TERROR_USE_ADRENALINE_CROUCHING", false),
        ("ACT_TERROR_USE_PILLS", "ACT_TERROR_USE_PILLS", false),
    ];
    PAIRS
        .iter()
        .find(|(s, c, _)| *s == standing || *c == standing)
        .map_or((standing, false), |&(s, c, cut)| (if crouched { c } else { s }, cut))
}
/// Bot `medical_kind` to its slot 0 activity (see `use_gesture_activity`).
fn use_gesture(kind: u8, crouched: bool) -> Option<(&'static str, bool)> {
    let standing = match kind {
        1 => "ACT_TERROR_HEAL_SELF",
        2 | 5 => "ACT_TERROR_HEAL_FRIEND",
        3 | 6 => "ACT_TERROR_USE_DEFIBRILLATOR",
        7 => "ACT_TERROR_USE_ADRENALINE",
        8 => "ACT_TERROR_USE_PILLS",
        _ => return None,
    };
    Some(use_gesture_activity(standing, crouched))
}
/// One CMultiPlayerAnimState gesture slot.
#[derive(Default, Clone, Copy, Debug)]
struct Gesture {
    sequence: usize,
    cycle: f32,
    active: bool,
}
impl Gesture {
    /// AddToGestureSlot (client 10147c50/101476f0): a new gesture replaces
    /// the slot's current one immediately and starts at cycle 0.
    fn restart(&mut self, sequence: usize) {
        *self = Self { sequence, cycle: 0., active: true };
    }
    /// UpdateGestureLayer (client 10147800). Advances at the sequence's own
    /// cycle rate, removes the gesture once its cycle passes 1, and fades it
    /// in and out over the sequence's fade times with the 3w^2 - 2w^3 spline
    /// (SNAP sequences keep full weight). Returns (sequence, time, weight).
    fn advance(&mut self, sk: &Skeleton, dt: f32) -> Option<(usize, f32, f32)> {
        if !self.active {
            return None;
        }
        let Some(seq) = sk.sequences().get(self.sequence) else {
            self.active = false;
            return None;
        };
        let duration = sk.sequence_anim(self.sequence).map_or(0., |a| sk.anims()[a].duration());
        if duration <= 0. {
            self.active = false;
            return None;
        }
        self.cycle += dt / duration;
        if self.cycle > 1. {
            self.active = false;
            return None;
        }
        let elapsed = duration * self.cycle;
        let remaining = duration - elapsed;
        let weight = if seq.snap() {
            1.
        } else {
            let w = if elapsed < seq.fade_in {
                1. - (seq.fade_in - elapsed) / seq.fade_in
            } else if remaining < seq.fade_out {
                1. - (seq.fade_out - remaining) / seq.fade_out
            } else {
                1.
            };
            3. * w * w - 2. * w * w * w
        };
        Some((self.sequence, elapsed, weight))
    }
}
impl Playback {
    fn record_gore_pose(&mut self, root: &Transform, dt: f32) {
        if dt <= 0. || !dt.is_finite() { return; }
        self.gore_clock += dt;
        if self.gore_history.back().is_some_and(|p|
            self.gore_clock - p.clock < 0.025 && p.sequence == self.sequence) { return; }
        // Keep the two samples bracketing the native 100ms lookback. Reuse
        // buffers; sampling at most 40Hz avoids per-render-frame allocations.
        let mut sample = if self.gore_history.len() > 2 &&
            self.gore_history[1].clock < self.gore_clock - 0.125 {
            self.gore_history.pop_front().unwrap()
        } else { GoreMotion::default() };
        sample.clock = self.gore_clock;
        sample.sequence = self.sequence;
        sample.root = *root;
        sample.pose.clone_from(&self.last_pose);
        self.gore_history.push_back(sample);
    }
    fn gore_sample(&self, sk: &Skeleton) -> Option<(Vec<XMat>, Transform, f32)> {
        let target = (self.gore_clock - 0.1).max(self.gore_history.front()?.clock);
        let a = self.gore_history.iter().rev().find(|p| p.clock <= target)?;
        let b = self.gore_history.iter().find(|p| p.clock >= target).unwrap_or(a);
        let interval = self.gore_clock - target;
        if interval <= 1e-6 || a.pose.len() != sk.bones.len() || b.pose.len() != sk.bones.len() { return None; }
        let alpha = if b.clock > a.clock { (target - a.clock) / (b.clock - a.clock) } else { 0. };
        let root = Transform {
            translation: a.root.translation.lerp(b.root.translation, alpha),
            rotation: a.root.rotation.slerp(b.root.rotation, alpha),
            scale: a.root.scale.lerp(b.root.scale, alpha),
        };
        // Never derive velocity from an animation switch. Retail evaluates
        // the same sequence twice; our history must not differentiate a pop.
        let pose = if a.sequence == self.sequence && b.sequence == self.sequence {
            let mut pose = b.pose.clone();
            blend_local_pose(sk, &a.pose, &mut pose, alpha);
            pose
        } else { self.last_pose.clone() };
        Some((pose, root, interval))
    }
    fn seed_viewmodel_events(&mut self, state: &Match) {
        if !self.initialized {
            // Equipping creates a new root. Historical events belong to the
            // old item; only changes after this baseline should trigger actions.
            self.shove_serial = state.shove_serial;
            self.item_use_serial = state.item_use_serial;
            self.throw_stage = state.throw_stage;
        }
    }
}
#[derive(Resource, Default)]
pub struct PlaybackState(HashMap<Entity, Playback>);
/// Survivor bot flashlights (on/off) by squad index, for drawing.
#[derive(Resource, Default)]
pub struct BotFlashlights(pub Vec<bool>);
// Source animation accumulation blends parent-relative position/quaternion
// channels. Interpolating each model-space bone independently shrinks limbs.
fn blend_local_pose(sk: &Skeleton, from: &[XMat], to: &mut [XMat], alpha: f32) {
    if from.len() != to.len() {
        return;
    }
    let target = to.to_vec();
    for i in 0..to.len() {
        let parent = sk.bones[i].parent;
        let (a, b) = if parent >= 0 {
            let p = parent as usize;
            (from[p].inverse() * from[i], target[p].inverse() * target[i])
        } else {
            (from[i], target[i])
        };
        let (as_, aq, ap) = a.to_scale_rotation_translation();
        let (bs, bq, bp) = b.to_scale_rotation_translation();
        let local = XMat::from_scale_rotation_translation(
            as_.lerp(bs, alpha),
            aq.slerp(bq, alpha),
            ap.lerp(bp, alpha),
        );
        to[i] = if parent >= 0 {
            to[parent as usize] * local
        } else {
            local
        };
    }
}
fn actor_parameters(
    sk: &Skeleton,
    transform: &Transform,
    velocity: Vec3,
    look: Vec3,
) -> Vec<(String, f32)> {
    let movement = transform.rotation.inverse() * velocity.normalize_or_zero();
    let aim = transform.rotation.inverse() * look.normalize_or_zero();
    let yaw = (-aim.z).atan2(aim.x).to_degrees();
    let pitch = (-aim.y).atan2(aim.xz().length()).to_degrees();
    sk.pose_params()
        .iter()
        .map(|p| {
            let v = match p.name.to_lowercase().as_str() {
                "move_x" => movement.x,
                "move_y" => -movement.z,
                "body_yaw" | "head_yaw" | "aim_yaw" => yaw,
                "body_pitch" | "head_pitch" | "aim_pitch" => pitch,
                _ => (p.start + p.end) * 0.5,
            };
            (
                p.name.clone(),
                v.clamp(p.start.min(p.end), p.start.max(p.end)),
            )
        })
        .collect()
}
/// nb_lean_max_angle, nb_lean_rate, nb_acceleration (server cvars).
const NB_LEAN_MAX_ANGLE: f32 = 30.;
const NB_LEAN_RATE: f32 = 3.;
const NB_ACCELERATION: f32 = 500.;
/// Common infected body pose parameters (InfectedBody update, server
/// 103f5c30): move_x/move_y are the velocity along the model's forward and
/// right vectors divided by the common's own run speed (so they shrink
/// while slow or turning), lean_yaw comes from the locomotion lean, and
/// body_yaw/body_pitch keep the aim at the target.
fn common_parameters(
    sk: &Skeleton,
    transform: &Transform,
    velocity: Vec3,
    look: Vec3,
    run_speed: f32,
    lean_yaw: f32,
) -> Vec<(String, f32)> {
    let mut values = actor_parameters(sk, transform, velocity, look);
    let forward = transform.rotation * Vec3::X;
    // Source "right" (-Y) is +Z in the model's Bevy frame.
    let right = transform.rotation * Vec3::Z;
    let flat = velocity * Vec3::new(1., 0., 1.);
    for (name, value) in values.iter_mut() {
        let range = sk.pose_params().iter().find(|p| p.name == *name).map(|p| (p.start.min(p.end), p.start.max(p.end)));
        let raw = match name.to_ascii_lowercase().as_str() {
            "move_x" => flat.dot(forward) / run_speed,
            "move_y" => flat.dot(right) / run_speed,
            "lean_yaw" => lean_yaw,
            _ => continue,
        };
        *value = range.map_or(raw, |(lo, hi)| raw.clamp(lo, hi));
    }
    values
}
/// ZombieBotLocomotion lean (server 103fcb64..103fcc00): the yaw lean eases
/// at nb_lean_rate toward -nb_lean_max_angle x (speed / run speed) x
/// (sideways acceleration / nb_acceleration). Retail feeds the accumulated
/// approach acceleration; the rewrite uses the measured acceleration.
fn common_lean(lean: f32, transform: &Transform, velocity: Vec3, previous: Vec3, run_speed: f32, dt: f32) -> f32 {
    if dt <= 0. || run_speed <= 0. {
        return lean;
    }
    let right = transform.rotation * Vec3::Z;
    let accel = ((velocity - previous) * Vec3::new(1., 0., 1.)) / dt;
    let speed = (velocity * Vec3::new(1., 0., 1.)).length();
    let lateral = accel.dot(right) / crate::collision::UNIT;
    let target = -NB_LEAN_MAX_ANGLE * (speed / run_speed) * (lateral / NB_ACCELERATION);
    lean + (target - lean) * (NB_LEAN_RATE * dt).min(1.)
}
/// Common infected playback rate (InfectedBody update, server 103f5c30):
/// 1, unless the actor moves faster than the sequence's ground speed at the
/// current blend while on the ground; then speed / ground speed, at most 12.
fn common_playback_rate(speed: f32, ground_speed: f32, grounded: bool) -> f32 {
    if grounded && ground_speed > 0. && speed > ground_speed {
        (speed / ground_speed).min(12.)
    } else {
        1.
    }
}
fn ladder_parameters(values: &mut [(String, f32)]) {
    // Ladder_Ascend/Descend blend from the climbing cycle (0) to the
    // dismount/standing pose (1). The generic midpoint can look frozen.
    for (name, value) in values {
        if name.eq_ignore_ascii_case("dismount") { *value = 0.; }
    }
}
fn survivor_locomotion(state: &Match, moving: bool) -> String {
    let class = state.held_class();
    let suffix = if class == "weapon_pistol_dual" {
        "elites"
    } else if matches!(class, "weapon_pumpshotgun" | "weapon_shotgun_chrome") {
        "pumpshotgun"
    } else if class.contains("shotgun") {
        "shotgun"
    } else if class == "weapon_hunting_rifle" {
        "sniper"
    } else if class.contains("sniper") {
        "sniper_military"
    } else if class.contains("smg") {
        "smg"
    } else if matches!(class, "weapon_first_aid_kit" | "weapon_defibrillator") {
        "firstaidkit"
    } else if matches!(
        class,
        "weapon_molotov" | "weapon_pipe_bomb" | "weapon_vomitjar"
    ) {
        "grenade"
    } else if class == "weapon_pistol" || class == "weapon_pistol_magnum" {
        "pistol"
    } else {
        "rifle"
    };
    format!(
        "{}_{}",
        if moving { "Run" } else { "Idle_Standing" },
        suffix
    )
}
/// Retail survivor animation state (client CTerrorPlayerAnimState, see
/// DEVELOPMENT.md "survivor animstate"): feet yaw, aim/head convergence,
/// the 9-way move blend, jump timing and the calm/injured activity choice.
#[derive(Default, Clone, Debug)]
pub(crate) struct SurvivorAnimState {
    initialized: bool,
    /// Degrees, Source convention (counter-clockwise from +x).
    eye_yaw: f32,
    eye_pitch: f32,
    eye_changed: f32,
    goal_feet: f32,
    pub feet_yaw: f32,
    body_yaw: f32,
    head_yaw: f32,
    body_pitch: f32,
    head_pitch: f32,
    airborne_since: Option<f32>,
    /// Landed after more than 0.2 s airborne: play ACT_TERROR_JUMP_LANDING.
    landed: bool,
    /// Director intensity above survivor_calm_intensity.
    intense: bool,
    /// Not calm before this time (survivor_calm_* delays).
    calm_after: f32,
    shot_serial: u64,
    damage_serial: u64,
    weapon: String,
    next_sight_check: f32,
    /// Retail SurvivorBot flashlight (server 1039d7a0): on once the bot has
    /// stood on a non-checkpoint NAV area since spawning (102fc490), except
    /// for 1 s after its nearest visible threat was a Witch.
    left_checkpoint: bool,
    witch_until: f32,
    pub flashlight: bool,
    /// `MapLogic::generation` the checkpoint flag belongs to.
    map_generation: u64,
    /// Main locomotion activity and its chosen sequence.
    activity: String,
    sequence: Option<usize>,
}
/// MOVING_MINIMUM_SPEED (units/s).
const SURVIVOR_MOVING_SPEED: f32 = 0.5;
/// HandleJumping: the jump activity holds for this long after leaving the ground.
const SURVIVOR_JUMP_TIME: f32 = 1.0;
/// Minimum airborne time for the landing gesture.
const SURVIVOR_LANDING_TIME: f32 = 0.2;
/// mp_feetyawrate, mp_feetmaxyawrate (deg/s), mp_facefronttime (s).
const FEET_YAW_RATE: f32 = 180.;
const FEET_MAX_YAW_RATE: f32 = 100.;
const FACE_FRONT_TIME: f32 = 2.;
/// Aim convergence rates (CTerrorPlayerAnimState init 102819c0): body yaw
/// takes the aim slowly at 2/s (x3 standing, x5 moving) and head yaw the
/// remainder at 20/s; body pitch tracks the eye at 12/s and head pitch the
/// remainder at 12/s. Slot roles confirmed against retail m_flPoseParameter
/// recordings (local-research/parity/2026-10-22-retail-bot-animstate*.log).
const BODY_YAW_RATE: f32 = 2.;
const HEAD_YAW_RATE: f32 = 20.;
const PITCH_RATE: f32 = 12.;
/// survivor_limp_health; survivor_calm_weapon_delay / _damage_delay /
/// _recent_enemy_delay / _deploy_delay.
const LIMP_HEALTH: f32 = 40.;
const CALM_WEAPON_DELAY: f32 = 5.;
const CALM_DAMAGE_DELAY: f32 = 5.;
const CALM_ENEMY_DELAY: f32 = 5.;
const CALM_DEPLOY_DELAY: f32 = 2.;
/// survivor_calm_no_flashlight: a lit flashlight blocks calm, then the
/// calm timer holds 4 s (server 103492a0, constant 105d3560).
const CALM_FLASHLIGHT_DELAY: f32 = 4.;
/// Bot flashlight stays off this long after seeing a Witch.
const FLASHLIGHT_WITCH_DELAY: f32 = 1.;
const CALM_INTENSITY: f32 = 0.25;
/// Survivor speed caps (units/s) used as the move blend's max ground speed:
/// run, limping run, walk (survivor_limp_walk_speed / walk), crouch.
const SURVIVOR_RUN_SPEED: f32 = 220.;
const SURVIVOR_LIMP_RUN_SPEED: f32 = 150.;
const SURVIVOR_WALK_SPEED: f32 = 85.;
const SURVIVOR_CROUCH_SPEED: f32 = 75.;

/// Retail run (m_flMaxspeed for survivor guns) and +speed walk speeds.
const RUN_SPEED: f32 = 220.;
const WALK_SPEED: f32 = 85.;
/// Viewmodel pose targets (client 102880d0). `move_x`: 0 below 75% of walk
/// speed, speed / walk * 0.5 up to walk, then 0.5..1 from walk to run;
/// negative when moving more than slightly backwards. `ver_aims`: Source
/// pitch / 90 standing; crouched it folds toward looking down.
fn view_pose_targets(velocity: Vec3, yaw: f32, pitch: f32, crouched: bool, run: f32) -> (f32, f32) {
    let flat = Vec3::new(velocity.x, 0., velocity.z);
    let speed = flat.length() / crate::collision::UNIT;
    // Run equal to walk (limping): only the walk half of the blend.
    let mut move_x = if speed < WALK_SPEED * 0.75 {
        0.
    } else if speed < WALK_SPEED || run <= WALK_SPEED {
        (speed / WALK_SPEED * 0.5).min(if run <= WALK_SPEED { 0.5 } else { 1. })
    } else {
        ((speed - WALK_SPEED) / (run - WALK_SPEED) * 0.5 + 0.5).min(1.)
    };
    let forward = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    if flat.normalize_or_zero().dot(forward) < -0.1 {
        move_x = -move_x;
    }
    // Source pitch is positive looking down; Body::pitch is positive up.
    let source_pitch = (-pitch).to_degrees();
    let stand = if crouched { 0. } else { 1. };
    let blended = (1. - stand) * -source_pitch.abs() + stand * source_pitch;
    (move_x, ((blended + 90.) / 180. * 2. - 1.).clamp(-1., 1.))
}
/// move_x eases at 1/s toward a larger magnitude, 2/s back toward rest.
fn approach_move_x(current: f32, target: f32, dt: f32) -> f32 {
    let rate = if target.abs() < current.abs() { 2. } else { 1. };
    approach_linear(current, target, rate * dt)
}
fn approach_linear(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}
fn angle_normalize(a: f32) -> f32 {
    let a = a.rem_euclid(360.);
    if a > 180. { a - 360. } else { a }
}
/// Source yaw (degrees) of a Bevy-space direction.
fn source_yaw(v: Vec3) -> f32 {
    (-v.z).atan2(v.x).to_degrees()
}
/// Feet yaw convergence (client 1027dc10).
fn converge_feet(goal: f32, eye: f32, rate: f32, max_rate: f32, dt: f32, current: f32) -> f32 {
    let delta = angle_normalize(goal - current);
    let distance = delta.abs();
    let direction = if distance > 90. { angle_normalize(eye - current) } else { delta };
    let scale = if distance < 45. { (distance / 45.).clamp(0.01, 1.) } else { 1. };
    let step = (scale * rate).min(max_rate) * dt;
    if step <= distance {
        angle_normalize(current + if direction <= 0. { -step } else { step })
    } else {
        angle_normalize(goal)
    }
}
/// Moves `current` toward `target` by `rate * difference * dt`, never past it.
fn approach(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let difference = target - current;
    let step = rate * difference * dt;
    current + if difference.abs() < step.abs() { difference } else { step }
}
/// Who a downed survivor is being revived by (retail m_reviveOwner): the
/// player holding use on them, otherwise a standing teammate in reach.
fn survivor_reviver(index: usize, squad: &Squad, state: &Match, player: Option<Vec3>) -> Option<Vec3> {
    let me = squad.0.get(index)?;
    if !me.state.incap || me.revive_time <= 0. {
        return None;
    }
    if state.reviving == Some(index) {
        return player;
    }
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(i, o)| *i != index && !o.state.incap && !o.state.dead && o.position.distance(me.position) < 2.)
        .min_by(|a, b| a.1.position.distance(me.position).total_cmp(&b.1.position.distance(me.position)))
        .map(|(_, o)| o.position)
}
/// Where a standing survivor bot aims its body (client 10280c60): the
/// m_useActionTarget of a medkit/defibrillator use, then m_reviveTarget.
fn survivor_aim_target(index: usize, squad: &Squad, state: &Match, player: Option<Vec3>) -> Option<Vec3> {
    let me = squad.0.get(index)?;
    if me.state.action > 0. {
        match me.state.medical_kind {
            2 | 3 => return me.state.medical_target.and_then(|j| squad.0.get(j)).map(|m| m.position),
            5 | 6 => return player,
            _ => {}
        }
    }
    if me.state.incap || me.state.dead || me.revive_time <= 0. && !squad.0.iter().any(|o| o.revive_time > 0.) {
        return None;
    }
    if state.incap && me.revive_time > 0. {
        if let Some(p) = player.filter(|p| p.distance(me.position) < 2.) {
            return Some(p);
        }
    }
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(i, o)| *i != index && o.state.incap && o.revive_time > 0. && o.position.distance(me.position) < 2.)
        .find(|(i, _)| survivor_reviver(*i, squad, state, player) == Some(me.position))
        .map(|(_, o)| o.position)
}
pub(crate) struct SurvivorInputs {
    pub now: f32,
    pub dt: f32,
    /// Bevy-space velocity (m/s) and aim direction.
    pub velocity: Vec3,
    pub aim: Vec3,
    pub airborne: bool,
    pub crouched: bool,
    /// IN_SPEED held (walk activity, walk max ground speed).
    pub walking: bool,
    pub limping: bool,
    pub max_run_speed: f32,
    /// Retail aim-target override (client 10280c60), Source yaw degrees:
    /// the body and head aim here instead of along the eyes, e.g. at a
    /// teammate being revived or healed. The feet still follow the eyes.
    pub aim_yaw: Option<f32>,
    /// Retail face-front states (server 10340e40 / client 102815d0): item
    /// use, reviving or being revived, ladders and holds. The feet turn to
    /// the eyes at once instead of after mp_facefronttime.
    pub face_front: bool,
    /// Held by a special (attacker handle 0x33b8): server 10341390 snaps the
    /// feet to the eyes.
    pub pinned: bool,
}
impl SurvivorAnimState {
    /// Base activity (server HandleJumping/HandleDucking/HandleMoving): bots
    /// never hold the walk key, so moving is RUN.
    fn base_activity(&mut self, input: &SurvivorInputs) -> &'static str {
        let speed = (input.velocity * Vec3::new(1., 0., 1.)).length() / 0.0254;
        if input.airborne {
            let since = *self.airborne_since.get_or_insert(input.now);
            if input.now - since <= SURVIVOR_JUMP_TIME {
                return "JUMP";
            }
        } else if let Some(since) = self.airborne_since.take() {
            // HandleJumping landing gesture (client 1027e7b0).
            if input.now - since > SURVIVOR_LANDING_TIME {
                self.landed = true;
            }
        }
        if input.crouched {
            return if speed < SURVIVOR_MOVING_SPEED { "CROUCHIDLE" } else { "RUN_CROUCH" };
        }
        if speed < SURVIVOR_MOVING_SPEED {
            "IDLE"
        } else if input.walking {
            "WALK"
        } else {
            "RUN"
        }
    }
    /// GetCurrentMaxGroundSpeed (client 1027e270) for a base activity.
    fn max_ground_speed(base: &str, input: &SurvivorInputs) -> f32 {
        match base {
            "RUN_CROUCH" | "CROUCHIDLE" => SURVIVOR_CROUCH_SPEED,
            "IDLE" | "WALK" => SURVIVOR_WALK_SPEED,
            _ => input.max_run_speed,
        }
    }
    /// One update. Returns (base activity, move_x, move_y).
    pub(crate) fn update(&mut self, input: &SurvivorInputs) -> (&'static str, f32, f32) {
        let eye = source_yaw(input.aim);
        let pitch = (-input.aim.y).atan2(input.aim.xz().length()).to_degrees();
        if !self.initialized {
            *self = Self { initialized: true, eye_yaw: eye, goal_feet: eye, feet_yaw: eye, ..std::mem::take(self) };
        }
        if (eye - self.eye_yaw).abs() > 1e-3 {
            self.eye_changed = input.now;
        }
        self.eye_yaw = eye;
        self.eye_pitch = pitch;
        let base = self.base_activity(input);
        let horizontal = input.velocity * Vec3::new(1., 0., 1.);
        let speed = horizontal.length() / 0.0254;
        let max_ground = Self::max_ground_speed(base, input);
        // Feet yaw (client 10281ab0).
        if speed > SURVIVOR_MOVING_SPEED && speed > max_ground * 0.9 {
            let velocity_yaw = source_yaw(horizontal);
            let relative = angle_normalize(velocity_yaw - eye);
            self.goal_feet = if relative > 100. || relative < -100. {
                angle_normalize(velocity_yaw + 180.)
            } else {
                velocity_yaw
            };
        }
        if input.face_front || input.now - self.eye_changed > FACE_FRONT_TIME {
            self.goal_feet = eye;
        }
        let offset = angle_normalize(self.goal_feet - eye);
        if offset.abs() > 90. {
            self.goal_feet = angle_normalize(eye + 90f32.copysign(offset));
        }
        if input.pinned {
            self.goal_feet = eye;
            self.feet_yaw = eye;
        }
        let multiplier = if speed > SURVIVOR_MOVING_SPEED { 2. } else { 1. };
        if self.feet_yaw != self.goal_feet {
            self.feet_yaw = converge_feet(
                self.goal_feet,
                eye,
                FEET_YAW_RATE * multiplier,
                FEET_MAX_YAW_RATE * multiplier,
                input.dt,
                self.feet_yaw,
            );
        }
        // Aim yaw/pitch split between body and head (client 10280c60, 1027dd10).
        let mut target = angle_normalize(input.aim_yaw.unwrap_or(eye) - self.feet_yaw);
        if target > 90. {
            target = 180. - target;
        } else if target < -90. {
            target = -180. - target;
        }
        let body_rate = BODY_YAW_RATE * if speed > SURVIVOR_MOVING_SPEED { 5. } else { 3. };
        self.body_yaw = approach(self.body_yaw, target, body_rate, input.dt);
        self.head_yaw = approach(self.head_yaw, target - self.body_yaw, HEAD_YAW_RATE, input.dt);
        self.body_pitch = approach(self.body_pitch, pitch, PITCH_RATE, input.dt);
        self.head_pitch = approach(self.head_pitch, pitch - self.body_pitch, PITCH_RATE, input.dt);
        // 9-way blend (client 10281e50): positive move_y is rightward.
        let (mut move_x, mut move_y) = (0., 0.);
        if speed > SURVIVOR_MOVING_SPEED && max_ground > 0. {
            let ratio = (speed / max_ground).clamp(0.01, 1.);
            let yaw = angle_normalize(self.feet_yaw - source_yaw(horizontal)).to_radians();
            move_x = yaw.cos() * ratio;
            move_y = yaw.sin() * ratio;
        }
        (base, move_x, move_y)
    }
    /// Survivor calm (server 103492a0, cvar delays): not after firing,
    /// taking damage, switching weapons or seeing an enemy recently.
    fn update_calm(&mut self, state: &Match, now: f32, saw_enemy: bool) {
        if state.shot_serial != self.shot_serial {
            self.calm_after = self.calm_after.max(now + CALM_WEAPON_DELAY);
        }
        if state.damage_serial != self.damage_serial {
            self.calm_after = self.calm_after.max(now + CALM_DAMAGE_DELAY);
        }
        let weapon = state.held_class();
        if weapon != self.weapon {
            self.calm_after = self.calm_after.max(now + CALM_DEPLOY_DELAY);
            self.weapon = weapon.to_string();
        }
        if saw_enemy {
            self.calm_after = self.calm_after.max(now + CALM_ENEMY_DELAY);
        }
        // Retail order: the timer, intensity and recent-enemy checks come
        // first; only then does a lit flashlight restart the 4 s timer.
        if self.flashlight && now >= self.calm_after && !self.intense {
            self.calm_after = now + CALM_FLASHLIGHT_DELAY;
        }
        self.shot_serial = state.shot_serial;
        self.damage_serial = state.damage_serial;
    }
    fn calm(&self, now: f32) -> bool {
        // `--capture-bot-alert`: survivors hold the alert (weapon raised) pose.
        static ALERT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ALERT.get_or_init(|| std::env::args().any(|a| a == "--capture-bot-alert")) {
            return false;
        }
        now >= self.calm_after && !self.intense
    }
    /// Pose-parameter values for this state.
    fn parameters(&self, sk: &Skeleton, move_x: f32, move_y: f32) -> Vec<(String, f32)> {
        sk.pose_params()
            .iter()
            .map(|p| {
                let v = match p.name.to_ascii_lowercase().as_str() {
                    "move_x" => move_x,
                    "move_y" => move_y,
                    "body_yaw" | "aim_yaw" => self.body_yaw,
                    "head_yaw" => self.head_yaw,
                    "body_pitch" | "aim_pitch" => self.body_pitch,
                    "head_pitch" => self.head_pitch,
                    _ => (p.start + p.end) * 0.5,
                };
                (p.name.clone(), v.clamp(p.start.min(p.end), p.start.max(p.end)))
            })
            .collect()
    }
}
/// Weapon activity suffix (the held weapon's player animation extension).
fn survivor_activity_suffix(state: &Match) -> String {
    let label = survivor_locomotion(state, false);
    match label.trim_start_matches("Idle_Standing_").to_ascii_uppercase().as_str() {
        "GRENADE" => "GREN".into(),
        other => other.into(),
    }
}
/// Native client weapon act tables (acttable_t, read from client.dll; see
/// local-research/parity/2026-10-22-weapon-acttables*.txt): base activity ->
/// weapon activity for each held weapon class.
pub(crate) fn weapon_act_table(class: &str) -> Option<&'static [(&'static str, &'static str)]> {
    Some(match class {
        "weapon_rifle" | "weapon_rifle_ak47" | "weapon_rifle_desert" => &[("ACT_IDLE", "ACT_IDLE_RIFLE"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_RIFLE"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_RIFLE"), ("ACT_WALK", "ACT_WALK_RIFLE"), ("ACT_RUN", "ACT_RUN_RIFLE"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_RIFLE"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_RIFLE"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_RIFLE"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_RIFLE"), ("ACT_WALK_CALM", "ACT_WALK_CALM_RIFLE"), ("ACT_RUN_CALM", "ACT_RUN_CALM_RIFLE"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_rifle_sg552" => &[("ACT_IDLE", "ACT_IDLE_RIFLE"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_RIFLE"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_RIFLE"), ("ACT_WALK", "ACT_WALK_RIFLE"), ("ACT_RUN", "ACT_RUN_RIFLE"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_RIFLE"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_RIFLE"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_RIFLE"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_RIFLE"), ("ACT_WALK_CALM", "ACT_WALK_CALM_RIFLE"), ("ACT_RUN_CALM", "ACT_RUN_CALM_RIFLE"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_rifle_m60" => &[("ACT_IDLE", "ACT_IDLE_SNIPER_MILITARY"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SNIPER_MILITARY"), ("ACT_RUN", "ACT_RUN_SNIPER_MILITARY"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_SNIPER_MILITARY"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SNIPER_MILITARY"), ("ACT_WALK", "ACT_WALK_SNIPER_MILITARY"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_SNIPER_MILITARY"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_SNIPER_MILITARY"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SNIPER_MILITARY"), ("ACT_WALK_CALM", "ACT_WALK_CALM_SNIPER_MILITARY"), ("ACT_RUN_CALM", "ACT_RUN_CALM_SNIPER_MILITARY"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_smg" => &[("ACT_IDLE", "ACT_IDLE_SMG"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SMG"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SMG"), ("ACT_WALK", "ACT_WALK_SMG"), ("ACT_RUN", "ACT_RUN_SMG"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_SMG"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_SMG"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_SMG"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SMG"), ("ACT_WALK_CALM", "ACT_WALK_CALM_SMG"), ("ACT_RUN_CALM", "ACT_RUN_CALM_SMG"), ("ACT_JUMP", "ACT_JUMP_SMG")],
        "weapon_smg_silenced" => &[("ACT_IDLE", "ACT_IDLE_SMG"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SMG"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SMG"), ("ACT_WALK", "ACT_WALK_SMG"), ("ACT_RUN", "ACT_RUN_SMG"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_SMG"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_SMG"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_SMG"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SMG"), ("ACT_WALK_CALM", "ACT_WALK_CALM_SMG"), ("ACT_RUN_CALM", "ACT_RUN_CALM_SMG"), ("ACT_JUMP", "ACT_JUMP_SMG")],
        "weapon_smg_mp5" => &[("ACT_IDLE", "ACT_IDLE_RIFLE"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_RIFLE"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_RIFLE"), ("ACT_WALK", "ACT_WALK_RIFLE"), ("ACT_RUN", "ACT_RUN_RIFLE"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_RIFLE"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_RIFLE"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_RIFLE"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_RIFLE"), ("ACT_WALK_CALM", "ACT_WALK_CALM_RIFLE"), ("ACT_RUN_CALM", "ACT_RUN_CALM_RIFLE"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_pumpshotgun" | "weapon_shotgun_chrome" => &[("ACT_IDLE", "ACT_IDLE_PUMPSHOTGUN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_PUMPSHOTGUN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_PUMPSHOTGUN"), ("ACT_WALK", "ACT_WALK_PUMPSHOTGUN"), ("ACT_RUN", "ACT_RUN_PUMPSHOTGUN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_PUMPSHOTGUN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_PUMPSHOTGUN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_PUMPSHOTGUN"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_PUMPSHOTGUN"), ("ACT_WALK_CALM", "ACT_WALK_CALM_PUMPSHOTGUN"), ("ACT_RUN_CALM", "ACT_RUN_CALM_PUMPSHOTGUN"), ("ACT_JUMP", "ACT_JUMP_SHOTGUN")],
        "weapon_autoshotgun" | "weapon_shotgun_spas" => &[("ACT_IDLE", "ACT_IDLE_SHOTGUN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SHOTGUN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SHOTGUN"), ("ACT_WALK", "ACT_WALK_SHOTGUN"), ("ACT_RUN", "ACT_RUN_SHOTGUN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_PUMPSHOTGUN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_PUMPSHOTGUN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_PUMPSHOTGUN"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SHOTGUN"), ("ACT_WALK_CALM", "ACT_WALK_CALM_PUMPSHOTGUN"), ("ACT_RUN_CALM", "ACT_RUN_CALM_PUMPSHOTGUN"), ("ACT_JUMP", "ACT_JUMP_SHOTGUN")],
        "weapon_hunting_rifle" | "weapon_sniper_awp" | "weapon_sniper_scout" => &[("ACT_IDLE", "ACT_IDLE_SNIPER"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SNIPER"), ("ACT_RUN", "ACT_RUN_SNIPER"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_SNIPER"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SNIPER"), ("ACT_WALK", "ACT_WALK_SNIPER"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_SNIPER"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_SNIPER"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SNIPER"), ("ACT_WALK_CALM", "ACT_WALK_CALM_SNIPER"), ("ACT_RUN_CALM", "ACT_RUN_CALM_SNIPER"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_sniper_military" => &[("ACT_IDLE", "ACT_IDLE_SNIPER_MILITARY"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_SNIPER_MILITARY"), ("ACT_RUN", "ACT_RUN_SNIPER_MILITARY"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_SNIPER_MILITARY"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_SNIPER_MILITARY"), ("ACT_WALK", "ACT_WALK_SNIPER_MILITARY"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_SNIPER_MILITARY"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_SNIPER_MILITARY"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_SNIPER_MILITARY"), ("ACT_WALK_CALM", "ACT_WALK_CALM_SNIPER_MILITARY"), ("ACT_RUN_CALM", "ACT_RUN_CALM_SNIPER_MILITARY"), ("ACT_JUMP", "ACT_JUMP_RIFLE")],
        "weapon_pistol" => &[("ACT_IDLE", "ACT_IDLE_PISTOL"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_PISTOL"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_PISTOL"), ("ACT_WALK", "ACT_WALK_PISTOL"), ("ACT_RUN", "ACT_RUN_PISTOL"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_PISTOL"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_PISTOL"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_PISTOL"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_PISTOL"), ("ACT_WALK_CALM", "ACT_WALK_CALM_PISTOL"), ("ACT_RUN_CALM", "ACT_RUN_CALM_PISTOL"), ("ACT_JUMP", "ACT_JUMP_PISTOL")],
        "weapon_pistol_magnum" => &[("ACT_IDLE", "ACT_IDLE_PISTOL"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_PISTOL"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_PISTOL"), ("ACT_WALK", "ACT_WALK_PISTOL"), ("ACT_RUN", "ACT_RUN_PISTOL"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_PISTOL"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_PISTOL"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_PISTOL"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_PISTOL"), ("ACT_WALK_CALM", "ACT_WALK_CALM_PISTOL"), ("ACT_RUN_CALM", "ACT_RUN_CALM_PISTOL"), ("ACT_JUMP", "ACT_JUMP_PISTOL")],
        "weapon_grenade_launcher" => &[("ACT_IDLE", "ACT_IDLE_PUMPSHOTGUN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_PUMPSHOTGUN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_PUMPSHOTGUN"), ("ACT_WALK", "ACT_WALK_PUMPSHOTGUN"), ("ACT_RUN", "ACT_RUN_PUMPSHOTGUN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_PUMPSHOTGUN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_PUMPSHOTGUN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_PUMPSHOTGUN"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_PUMPSHOTGUN"), ("ACT_WALK_CALM", "ACT_WALK_CALM_PUMPSHOTGUN"), ("ACT_RUN_CALM", "ACT_RUN_CALM_PUMPSHOTGUN"), ("ACT_JUMP", "ACT_JUMP_SHOTGUN")],
        "weapon_pipe_bomb" | "weapon_molotov" | "weapon_vomitjar" => &[("ACT_IDLE", "ACT_IDLE_GREN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_GREN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_GREN"), ("ACT_WALK", "ACT_WALK_GREN"), ("ACT_RUN", "ACT_RUN_GREN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_GREN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_GREN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_GREN"), ("ACT_IDLE_CALM", "ACT_IDLE_GREN"), ("ACT_WALK_CALM", "ACT_WALK_GREN"), ("ACT_RUN_CALM", "ACT_RUN_GREN"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_first_aid_kit" => &[("ACT_IDLE", "ACT_IDLE_FIRSTAIDKIT"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_FIRSTAIDKIT"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_FIRSTAIDKIT"), ("ACT_WALK", "ACT_WALK_FIRSTAIDKIT"), ("ACT_RUN", "ACT_RUN_FIRSTAIDKIT"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_FIRSTAIDKIT"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_FIRSTAIDKIT"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_FIRSTAIDKIT"), ("ACT_IDLE_CALM", "ACT_IDLE_FIRSTAIDKIT"), ("ACT_WALK_CALM", "ACT_WALK_FIRSTAIDKIT"), ("ACT_RUN_CALM", "ACT_RUN_FIRSTAIDKIT"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_pain_pills" => &[("ACT_IDLE", "ACT_IDLE_GREN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_GREN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_GREN"), ("ACT_WALK", "ACT_WALK_GREN"), ("ACT_RUN", "ACT_RUN_GREN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_GREN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_GREN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_GREN"), ("ACT_IDLE_CALM", "ACT_IDLE_GREN"), ("ACT_WALK_CALM", "ACT_WALK_GREN"), ("ACT_RUN_CALM", "ACT_RUN_GREN"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_adrenaline" => &[("ACT_IDLE", "ACT_IDLE_GREN"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_GREN"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_GREN"), ("ACT_WALK", "ACT_WALK_GREN"), ("ACT_RUN", "ACT_RUN_GREN"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_GREN"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_GREN"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_GREN"), ("ACT_IDLE_CALM", "ACT_IDLE_GREN"), ("ACT_WALK_CALM", "ACT_WALK_GREN"), ("ACT_RUN_CALM", "ACT_RUN_GREN"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_defibrillator" => &[("ACT_IDLE", "ACT_IDLE_ELITES"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_ELITES"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_ELITES"), ("ACT_WALK", "ACT_WALK_ELITES"), ("ACT_RUN", "ACT_RUN_ELITES"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_ELITES"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_ELITES"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_ELITES"), ("ACT_IDLE_CALM", "ACT_IDLE_CALM_ELITES"), ("ACT_WALK_CALM", "ACT_WALK_CALM_ELITES"), ("ACT_RUN_CALM", "ACT_RUN_CALM_ELITES"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_upgradepack_explosive" | "weapon_upgradepack_incendiary" => &[("ACT_IDLE", "ACT_IDLE_FIRSTAIDKIT"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_FIRSTAIDKIT"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_FIRSTAIDKIT"), ("ACT_WALK", "ACT_WALK_FIRSTAIDKIT"), ("ACT_RUN", "ACT_RUN_FIRSTAIDKIT"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_FIRSTAIDKIT"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_FIRSTAIDKIT"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_FIRSTAIDKIT"), ("ACT_IDLE_CALM", "ACT_IDLE_FIRSTAIDKIT"), ("ACT_WALK_CALM", "ACT_WALK_FIRSTAIDKIT"), ("ACT_RUN_CALM", "ACT_RUN_FIRSTAIDKIT"), ("ACT_JUMP", "ACT_JUMP_ITEM")],
        "weapon_chainsaw" => &[("ACT_IDLE", "ACT_IDLE_CHAINSAW"), ("ACT_CROUCHIDLE", "ACT_CROUCHIDLE_CHAINSAW"), ("ACT_RUN_CROUCH", "ACT_RUN_CROUCH_CHAINSAW"), ("ACT_WALK", "ACT_WALK_CHAINSAW"), ("ACT_RUN", "ACT_RUN_CHAINSAW"), ("ACT_IDLE_INJURED", "ACT_IDLE_INJURED_CHAINSAW"), ("ACT_WALK_INJURED", "ACT_WALK_INJURED_CHAINSAW"), ("ACT_RUN_INJURED", "ACT_RUN_INJURED_CHAINSAW"), ("ACT_IDLE_CALM", "ACT_IDLE_CHAINSAW"), ("ACT_WALK_CALM", "ACT_WALK_CHAINSAW"), ("ACT_RUN_CALM", "ACT_RUN_CHAINSAW"), ("ACT_JUMP", "ACT_JUMP_CHAINSAW")],
        _ => return None,
    })
}

/// Retail TranslateActivity (client 1027e8f0) for a survivor's base
/// activity: injured below survivor_limp_health (health + temporary), calm
/// variants when healthy and calm, then the weapon suffix. Returns the
/// candidate activity names in preference order.
fn survivor_activity_names(
    base: &str,
    class: &str,
    suffix: &str,
    limping: bool,
    calm: bool,
) -> Vec<String> {
    let translated = match (base, limping, calm) {
        ("IDLE", true, _) => "IDLE_INJURED",
        ("WALK", true, _) => "WALK_INJURED",
        ("RUN", true, _) => "RUN_INJURED",
        ("IDLE", false, true) => "IDLE_CALM",
        ("WALK", false, true) => "WALK_CALM",
        ("RUN", false, true) => "RUN_CALM",
        _ => base,
    };
    let mut names = vec![];
    for activity in [translated, base] {
        let act = format!("ACT_{activity}");
        if class == "weapon_pistol_dual" {
            // C_Pistol::TranslateActivity (client 10305690) while dual wielding.
            names.push(if activity == "JUMP" { "ACT_JUMP_DUAL_PISTOL".into() } else { format!("{act}_ELITES") });
        } else if let Some((_, weapon)) =
            weapon_act_table(class).and_then(|t| t.iter().find(|(a, _)| *a == act))
        {
            names.push(weapon.to_string());
        } else {
            names.push(format!("{act}_{suffix}"));
        }
        if translated == base {
            break;
        }
    }
    names
}
fn companion_weapon_action(
    sk: &Skeleton,
    state: &Match,
    action: &str,
    moving: bool,
) -> Option<usize> {
    let class = state.gun().class.as_str();
    let pump = matches!(class, "weapon_pumpshotgun" | "weapon_shotgun_chrome");
    let shotgun = pump || class.contains("shotgun");
    let name = match action {
        "shove" => format!(
            "ACT_MELEE_Shove_Rifle_{}",
            if moving { "RUN" } else { "IDLE" }
        ),
        "reload" if shotgun => format!(
            "ACT_RELOAD_{}_{}",
            if pump { "PUMPSHOTGUN" } else { "SHOTGUN" },
            match state.reload_phase {
                2 => "LOOP",
                3 => "END",
                _ => "START",
            }
        ),
        "reload" => format!(
            "ACT_RELOAD_{}",
            if class == "weapon_pistol_dual" {
                "ELITES"
            } else if class.contains("pistol") {
                "PISTOL"
            } else if class.contains("smg") {
                "SMG"
            } else if class.contains("sniper") || class == "weapon_hunting_rifle" {
                "RIFLE"
            } else {
                "M4"
            }
        ),
        "shoot" => format!(
            "ACT_PRIMARYATTACK_{}",
            if class == "weapon_pistol_dual" {
                if state.gun().ammo % 2 == 0 {
                    "ELITES_L"
                } else {
                    "ELITES_R"
                }
            } else if class.contains("pistol") {
                "PISTOL"
            } else if pump {
                "PUMPSHOTGUN"
            } else if class == "weapon_shotgun_spas" {
                "M3S90"
            } else if shotgun {
                "XM1014"
            } else if class.contains("smg") {
                "SMG"
            } else {
                "RIFLE"
            }
        ),
        _ => return None,
    };
    choose(sk, &[&name])
}
fn draw_bind(mesh: ModelMesh, skeleton: &Skeleton) -> Draw {
    let bones = skeleton
        .bones
        .iter()
        .map(|b| {
            let local = XMat::from_rotation_translation(b.quat, b.pos);
            BindBone {
                name: b.name.to_ascii_lowercase(),
                parent: b.parent,
                local,
                inverse: b.pose_to_bone,
            }
        })
        .collect();
    Draw { mesh, bones }
}
impl AnimationLibrary {
    /// Extracted root motion of sequence `label` on model `path` between two
    /// times after its start (seconds, played at rate 1): (forward, left)
    /// distance in Source units and the yaw turned in degrees, as Source's
    /// Studio_SeqMovement moves a wandering common by its clip.
    pub(crate) fn clip_motion(&self, path: &str, label: &str, from: f32, to: f32) -> Option<(Vec2, f32)> {
        let sk = &self.models.get(path)?.skeleton;
        let anim = sk.sequence_anim(sk.find_sequence(label)?)?;
        let duration = sk.anims()[anim].duration().max(1e-3);
        let (a, a_yaw) = sk.anim_position(anim, (from / duration).clamp(0., 1.));
        let (b, b_yaw) = sk.anim_position(anim, (to / duration).clamp(0., 1.));
        // The step is taken in the clip's frame at `from`: undo the yaw the
        // clip had already turned.
        let step = Vec2::new(b.x - a.x, b.y - a.y);
        let undo = Vec2::from_angle(-a_yaw.to_radians());
        Some((undo.rotate(step), b_yaw - a_yaw))
    }
    pub(crate) fn gore_pose_hash(&self, root: Entity, playback: &PlaybackState) -> Option<u64> {
        Some(playback.0.get(&root)?.last_pose.iter().flat_map(|m| m.to_cols_array())
            .fold(0u64, |h, f| h.wrapping_mul(16777619) ^ f.to_bits() as u64))
    }
    pub(crate) fn gore_attachment(&self, path: &str, root: Entity, playback: &PlaybackState, name: &str) -> Option<Vec3> {
        let model = self.models.get(path)?;
        let pose = &playback.0.get(&root)?.last_pose;
        let matrix = model.skeleton.attachment_matrix(name, pose)?;
        Some(Vec3::from_array(crate::binary::source_position(matrix.w_axis.truncate().to_array())))
    }
    pub(crate) fn merge_gore_mesh(&self, parent: &str, root: Entity, playback: &PlaybackState,
        path: &str, draw: usize, mesh: &mut Mesh) {
        let Some(parent) = self.models.get(parent) else { return };
        let Some(pose) = playback.0.get(&root).map(|p| &p.last_pose) else { return };
        let Some(draw) = self.models.get(path).and_then(|m| m.draws.get(draw)) else { return };
        let globals = parent.skeleton.bones.iter().zip(pose).map(|(b,m)| (b.name.to_ascii_lowercase(), *m)).collect();
        let (positions, normals) = skin(draw, &globals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    pub(crate) fn gib_body(&self, parent: &str, root: Entity, playback: &PlaybackState,
        path: &str, transform: &Transform, velocity: Vec3) -> Option<crate::ragdoll::Body> {
        let pose = &playback.0.get(&root)?.last_pose;
        let previous = playback.0.get(&root)?.gore_sample(&self.models.get(parent)?.skeleton);
        self.gib_body_pose(parent, path, transform, velocity, pose, previous.as_ref(), None)
    }
    pub(crate) fn freeze_gore_pose(&self, parent: &str, root: Entity, playback: &PlaybackState) -> Option<FrozenGorePose> {
        let parent = self.models.get(parent)?;
        let p = playback.0.get(&root)?;
        (p.last_pose.len() == parent.skeleton.bones.len()).then(|| FrozenGorePose {
            pose: p.last_pose.clone(), previous: p.gore_sample(&parent.skeleton),
        })
    }
    pub(crate) fn remap_gore_pose(&self, parent: &str, snapshot: &FrozenGorePose, target: &str) -> Option<FrozenGorePose> {
        let parent = &self.models.get(parent)?.skeleton;
        let target = &self.models.get(target)?.skeleton;
        Some(FrozenGorePose {
            pose: merge_skeleton_pose(parent, &snapshot.pose, target),
            previous: snapshot.previous.as_ref().map(|(pose, root, dt)| (merge_skeleton_pose(parent, pose, target), *root, *dt)),
        })
    }
    pub(crate) fn merge_frozen_gore(&self, parent: &str, snapshot: &FrozenGorePose,
        path: &str, draw: usize, mesh: &mut Mesh) {
        let Some(parent) = self.models.get(parent) else { return };
        let Some(draw) = self.models.get(path).and_then(|m| m.draws.get(draw)) else { return };
        let globals = parent.skeleton.bones.iter().zip(&snapshot.pose).map(|(b,m)| (b.name.to_ascii_lowercase(), *m)).collect();
        let (positions, normals) = skin(draw, &globals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    pub(crate) fn frozen_gib_body(&self, parent: &str, snapshot: &FrozenGorePose, path: &str,
        transform: &Transform, boomer_force: Option<Vec3>) -> Option<crate::ragdoll::Body> {
        self.gib_body_pose(parent, path, transform, Vec3::ZERO, &snapshot.pose, snapshot.previous.as_ref(), boomer_force)
    }
    pub(crate) fn replace_corpse(&self, path: &str, root: Entity, body: crate::ragdoll::Body,
        transform: &Transform, playback: &mut PlaybackState) {
        let p = playback.0.entry(root).or_default();
        p.last_pose = body.pose(transform);
        p.ragdoll = Some(body);
        p.sequence = 0;
        p.activity = path.into();
        p.gore_history.clear();
        p.transition_pose.clear();
    }
    pub(crate) fn corpse_center(&self, root: Entity, playback: &PlaybackState) -> Option<Vec3> {
        Some(playback.0.get(&root)?.ragdoll.as_ref()?.center())
    }
    fn gib_body_pose(&self, parent: &str, path: &str, transform: &Transform, velocity: Vec3,
        source_pose: &[XMat], previous: Option<&(Vec<XMat>, Transform, f32)>, boomer_force: Option<Vec3>) -> Option<crate::ragdoll::Body> {
        let parent = self.models.get(parent)?;
        if source_pose.len() != parent.skeleton.bones.len() { return None; }
        let model = self.models.get(path)?;
        let profile = model.physics.as_ref()?;
        if profile.solids.is_empty() { return None; }
        let pose = merge_skeleton_pose(&parent.skeleton, source_pose, &model.skeleton);
        let mut body = crate::ragdoll::Body::new(&model.skeleton, &pose, transform, velocity)
            .with_fragment_profile(&model.skeleton, profile, &self.surfaces);
        let fallback = (previous.is_none() && boomer_force.is_some()).then(|| (source_pose.to_vec(), *transform, 0.1));
        let previous = previous.or(fallback.as_ref());
        if let Some((old_pose, old_root, interval)) = previous {
            let mut previous_pose = merge_skeleton_pose(&parent.skeleton, old_pose, &model.skeleton);
            if let Some(force) = boomer_force {
                // Native 10222720 offsets the first two historical world-bone
                // origins by 500 / 50 units before the 0.1 s ragdoll init.
                // Apply these in old model space; current geometry stays intact.
                let world_direction = Vec3::from_array(binary::source_normal(force.normalize_or_zero().to_array()));
                let local = old_root.rotation.inverse() * world_direction;
                let source = XVec::new(local.x, -local.z, local.y);
                for (matrix, distance) in previous_pose.iter_mut().take(2).zip([500., 50.]) {
                    matrix.w_axis -= (source * distance).extend(0.);
                }
            }
            body.inherit_pose_motion(&previous_pose, old_root, if boomer_force.is_some() { 0.1 } else { *interval });
        }
        if let Some(force) = boomer_force {
            body.apply_fragment_impulse(Vec3::from_array(binary::source_position(force.to_array())));
        }
        Some(body)
    }
    pub(crate) fn skin_gib(&self, path: &str, draw: usize, pose: &[XMat], mesh: &mut Mesh) {
        let Some(model) = self.models.get(path) else { return };
        let Some(draw) = model.draws.get(draw) else { return };
        let globals = model.skeleton.bones.iter().zip(pose)
            .map(|(b,m)| (b.name.to_ascii_lowercase(), *m)).collect();
        let (positions, normals) = skin(draw, &globals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    pub(crate) fn cut_wounds(&self, path: &str, draw: usize, wounds: &[usize], mesh: &mut Mesh) -> Option<[Vec4; 12]> {
        let model = self.models.get(path)?;
        let draw = model.draws.get(draw)?;
        let pelvis = model.skeleton.bones.iter().find(|b| b.name.eq_ignore_ascii_case("ValveBiped.Bip01_Pelvis"))?;
        // Keep the complete original triangle topology. Interpolated bind-space
        // coordinates let the fragment shader cut only pixels inside the wound,
        // independent of CPU/GPU skinning, animation, and ragdoll deformation.
        let coordinates = draw.mesh.positions.iter().map(|p| {
            let source = XVec::new(p[0], -p[2], p[1]) / 0.0254;
            let p = pelvis.pose_to_bone.transform_point3(source);
            [p.x, p.y, p.z]
        }).collect::<Vec<_>>();
        mesh.insert_attribute(crate::infected_gore::WOUND_BIND_POSITION, coordinates);
        let mut rows = [Vec4::ZERO; 12];
        for (slot, wound) in wounds.iter().filter_map(|i| self.gore.wounds.get(*i)).take(2).enumerate() {
            let volume_rows = crate::infected_gore::wound_rows(&wound.volumes);
            rows[slot * 6..slot * 6 + 6].copy_from_slice(&volume_rows[..6]);
        }
        Some(rows)
    }
    pub(crate) fn playback_info(
        &self,
        path: &str,
        entity: Entity,
        state: &PlaybackState,
    ) -> Option<(String, f32, f32)> {
        let model = self.models.get(path)?;
        let playback = state.0.get(&entity)?;
        let label = model
            .skeleton
            .sequences()
            .get(playback.sequence)?
            .label
            .clone();
        let duration = model
            .skeleton
            .sequence_anim(playback.sequence)
            .and_then(|i| model.skeleton.anims().get(i))
            .map_or(0., |a| a.duration());
        Some((label, playback.clock, duration))
    }
    pub fn hitboxes(
        &self,
        path: &str,
        entity: Entity,
        root: &Transform,
        playback: Option<&PlaybackState>,
    ) -> Option<crate::studio_hitboxes::Boxes> {
        let model = self.models.get(path)?;
        let pose = playback
            .and_then(|p| p.0.get(&entity))
            .map(|p| p.last_pose.as_slice());
        let fallback;
        let pose = match pose.filter(|p| p.len() == model.skeleton.bones.len()) {
            Some(p) => p,
            None => {
                let seq = choose(&model.skeleton, &["ACT_TERROR_IDLE_ALERT", "ACT_IDLE"])?;
                fallback = model.skeleton.pose_sequence(seq, 0., true);
                &fallback
            }
        };
        let boxes = crate::studio_hitboxes::Boxes::from_pose(&model.skeleton, pose, root);
        (!boxes.is_empty()).then_some(boxes)
    }
    pub fn muzzle(
        &self,
        path: &str,
        entity: Entity,
        root: &GlobalTransform,
        playback: &PlaybackState,
        second: bool,
    ) -> Option<(Vec3, Vec3, Vec3)> {
        let model = self.models.get(path)?;
        let pose = &playback.0.get(&entity)?.last_pose;
        let mut attachments: Vec<_> = model
            .skeleton
            .attachments
            .iter()
            .filter(|a| a.name.to_ascii_lowercase().contains("muzzle"))
            .collect();
        attachments.sort_by_key(|a| {
            let name = a.name.to_ascii_lowercase();
            (
                if name == "muzzle_flash"
                    || name == "muzzle_flash_r"
                    || name == "muzzle_flash_right"
                {
                    0
                } else if name.starts_with("muzzle_flash") {
                    1
                } else if name.starts_with("attach_muzzle") {
                    2
                } else {
                    3
                },
                name,
            )
        });
        let a = attachments.get(if second && attachments.len() > 1 {
            1
        } else {
            0
        })?;
        let matrix = *pose.get(a.bone)? * a.local;
        let local = Vec3::from_array(crate::binary::source_position(
            matrix.w_axis.truncate().to_array(),
        ));
        let direction = root
            .affine()
            .transform_vector3(render_bone(matrix).transform_vector3(Vec3::X))
            .normalize_or_zero();
        let up = root
            .affine()
            .transform_vector3(render_bone(matrix).transform_vector3(Vec3::Y))
            .normalize_or_zero();
        Some((root.transform_point(local), direction, up))
    }
    pub fn hitbox_target(
        &self,
        path: &str,
        entity: Entity,
        root: &Transform,
        playback: Option<&PlaybackState>,
        group: i32,
    ) -> Option<Vec3> {
        let model = self.models.get(path)?;
        let p = playback?.0.get(&entity)?;
        let b = if group == 6 || group == 7 {
            model.skeleton.hitboxes.iter().find(|b| {
                b.group == group
                    && model.skeleton.bones[b.bone]
                        .name
                        .to_ascii_lowercase()
                        .contains("calf")
            })
        } else {
            model
                .skeleton
                .hitboxes
                .iter()
                .filter(|b| b.group == group)
                .last()
        }?;
        let point = p
            .last_pose
            .get(b.bone)?
            .transform_point3((b.min + b.max) * 0.5);
        Some(root.transform_point(Vec3::from_array(binary::source_position(point.to_array()))))
    }
    pub fn audit(&self, weapons: &HashMap<String, crate::game_state::Weapon>) -> io::Result<()> {
        let mut report = String::from("class\taction\tsequence\tduration\tpaired_layer\n");
        let mut missing = vec![];
        let mut weapons = weapons.values().collect::<Vec<_>>();
        weapons.sort_by_key(|w| &w.class);
        for weapon in weapons {
            let Some(model) = self.models.get(&weapon.view_model) else {
                missing.push(weapon.class.clone());
                continue;
            };
            let mut actions = vec![
                ("idle", "ACT_VM_IDLE"),
                ("deploy", "ACT_VM_DEPLOY"),
                ("shove", "ACT_VM_MELEE"),
            ];
            if let Some(role) = crate::special_infected::SpecialClass::from_weapon(&weapon.class) {
                actions.retain(|(action, _)| *action != "shove");
                actions.extend(role.vm_actions().iter().copied());
            }
            if weapon.throwable() {
                actions.extend([("pullpin", "ACT_VM_PULLPIN"), ("throw", "ACT_VM_THROW")]);
            } else if weapon.melee() {
                actions.push(("attack", "ACT_VM_PRIMARYATTACK"));
            } else if weapon.medical() {
                actions.push(("use", medical_activity(&weapon.class, false)));
            } else {
                actions.extend([
                    ("attack", "ACT_VM_PRIMARYATTACK"),
                    ("reload", "ACT_VM_RELOAD"),
                ]);
            }
            for (action, name) in actions {
                if let Some(sequence) = choose(&model.skeleton, &[name, name]) {
                    let s = &model.skeleton.sequences()[sequence];
                    let duration = model
                        .skeleton
                        .sequence_anim(sequence)
                        .map_or(0., |a| model.skeleton.anims()[a].duration());
                    report += &format!(
                        "{}\t{action}\t{}\t{duration:.3}\t{}\n",
                        weapon.class,
                        s.label,
                        model
                            .skeleton
                            .find_sequence(&format!("{}_layer", s.label))
                            .is_some()
                    );
                    let pose = model.skeleton.pose_retargeted_view_sequence(
                        sequence,
                        duration * 0.4,
                        s.looping(),
                    );
                    if pose.iter().any(|m| !m.is_finite()) {
                        missing.push(format!("{} nonfinite {action}", weapon.class));
                    }
                } else {
                    report += &format!("{}\t{action}\tunavailable\t0\tfalse\n", weapon.class);
                    missing.push(format!("{} {action}", weapon.class));
                }
            }
        }
        std::fs::create_dir_all("local-research")?;
        std::fs::write("local-research/weapon-animation-audit.tsv", report)?;
        println!("Weapon animation audit: unavailable {:?}", missing);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(binary::invalid("See weapon-animation-audit.tsv"))
        }
    }
    pub fn duration(&self, path: &str, names: &[&str]) -> f32 {
        self.models
            .get(path)
            .and_then(|m| {
                choose(&m.skeleton, names)
                    .and_then(|s| m.skeleton.sequence_anim(s))
                    .and_then(|a| m.skeleton.anims().get(a))
            })
            .map_or(0.5, |a| a.duration().max(0.05))
    }
    pub fn infected_reaction_duration(&self, path: &str, activity: &str, seed: u64) -> f32 {
        self.models
            .get(path)
            .and_then(|m| {
                weighted_activity(&m.skeleton, activity, seed)
                    .and_then(|s| m.skeleton.sequence_anim(s))
                    .and_then(|a| m.skeleton.anims().get(a))
            })
            .map_or(0.5, |a| {
                a.duration()
                    .clamp(0.1, if activity.contains("DIE") { 10. } else { 2. })
            })
    }
    pub fn reload_times(&self, path: &str) -> [f32; 3] {
        [
            self.duration(path, &["ACT_VM_RELOAD", "reload"]),
            self.duration(path, &["ACT_VM_RELOAD_LOOP", "reload_loop"]),
            self.duration(path, &["ACT_VM_RELOAD_END", "reload_end"]),
        ]
    }
    pub fn empty_reload_time(&self, path: &str) -> f32 {
        self.models
            .get(path)
            .and_then(|model| {
                choose(&model.skeleton, &["ACT_VM_RELOAD_EMPTY", "reload_empty"])
                    .and_then(|s| model.skeleton.sequence_anim(s))
                    .map(|a| model.skeleton.anims()[a].duration())
            })
            .unwrap_or(0.)
    }
    pub fn death_duration(
        &self,
        path: &str,
        context: crate::infected_death::Context,
        seed: u64,
    ) -> f32 {
        self.models
            .get(path)
            .and_then(|m| {
                crate::infected_death::select(&m.skeleton, context, seed)
                    .and_then(|s| m.skeleton.sequence_anim(s))
                    .map(|a| m.skeleton.anims()[a].duration())
            })
            .unwrap_or(3.)
            .clamp(0.65, 10.)
    }
    pub fn default_preview_sequence(&self, path: &str) -> usize {
        self.models
            .get(path)
            .and_then(|m| {
                weighted_activity(&m.skeleton, "ACT_TERROR_IDLE_ALERT", 0)
                    .or_else(|| choose(&m.skeleton, &["Idle_Standing_pistol", "ACT_IDLE"]))
            })
            .unwrap_or(0)
    }
    pub fn sequence_names(&self, path: &str) -> Vec<String> {
        self.models
            .get(path)
            .map(|m| {
                m.skeleton
                    .sequences()
                    .iter()
                    .map(|s| s.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn preview(
        &self,
        pose_path: &str,
        sequence: usize,
        time: f32,
        mesh_path: &str,
        index: usize,
        mesh: &mut Mesh,
    ) {
        let Some(model) = self.models.get(pose_path) else {
            return;
        };
        if sequence >= model.skeleton.sequences().len() {
            return;
        }
        let Some(draw) = self.models.get(mesh_path).and_then(|m| m.draws.get(index)) else {
            return;
        };
        let pose = model.skeleton.pose_sequence_params(
            sequence,
            time,
            true,
            &[
                ("move_x", 1.),
                ("move_y", 0.),
                ("body_pitch", 0.),
                ("body_yaw", 0.),
                ("lean_yaw", 0.),
            ],
        );
        let globals = model
            .skeleton
            .bones
            .iter()
            .zip(pose)
            .map(|(b, m)| (b.name.to_ascii_lowercase(), m))
            .collect();
        let (positions, normals) = skin(draw, &globals);
        if positions.iter().any(|p| !Vec3::from_array(*p).is_finite()) {
            return;
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    /// Whether `path` was loaded as an animated model.
    pub fn contains(&self, path: &str) -> bool {
        self.models.contains_key(path)
    }
    pub fn scripted_sample_time(&self,path:&str,name:&str,elapsed:f32)->Option<f32> {
        let sk=&self.models.get(path)?.skeleton;
        let seq=sk.find_sequence(name)?;
        let duration=sk.sequence_anim(seq).map_or(0.,|a|sk.anims()[a].duration());
        Some(if duration<=0. {0.} else if sk.sequences()[seq].looping() {elapsed.max(0.)%duration} else {elapsed.clamp(0.,duration)})
    }
    /// CPU-skinned triangle-list positions (the draw's own render space) and
    /// material of one draw of a model playing `name` at `time`, for CPU
    /// visibility queries against GPU-skinned scripted models.
    pub(crate) fn scripted_triangles(&self, path: &str, draw: usize, name: &str, time: f32) -> Option<(Vec<[f32; 3]>, &str)> {
        let globals = self.scripted_globals(path, name, time, &[])?;
        let d = self.models.get(path)?.draws.get(draw)?;
        Some((skin(d, &globals).0, d.mesh.material.as_str()))
    }
    pub fn scripted(&self, path: &str, draw: usize, name: &str, time: f32, mesh: &mut Mesh) {
        self.scripted_merged(path, path, draw, name, time, mesh);
    }
    /// Duration and authored sound events (5004: seconds, soundscript) of
    /// one model sequence, for sounds the rewrite drives without playing
    /// that sequence (e.g. the Hunter's Melee_Pounce shred slices).
    pub fn sequence_sounds(
        &self,
        model: &str,
        sequence: &str,
    ) -> Option<(f32, Vec<(f32, String)>)> {
        let sk = &self.models.get(model)?.skeleton;
        let index = sk.find_sequence(sequence)?;
        let duration = sk
            .sequence_anim(index)
            .map_or(0., |i| sk.anims()[i].duration());
        let events = sk.sequences()[index]
            .events
            .iter()
            .filter(|e| e.event == 5004)
            .map(|e| (e.cycle * duration, e.options.clone()))
            .collect();
        (duration > 0.).then_some((duration, events))
    }
    pub fn hunter_pin(&self, draw: usize, clock: f32, attack: Option<f32>, mesh: &mut Mesh) {
        let Some(model) = self.models.get(crate::special_infected::HUNTER_MODEL) else {
            return;
        };
        let sk = &model.skeleton;
        let Some(base) = sk.find_sequence("Idle_Crouching_01") else {
            return;
        };
        let mut layers = vec![];
        if let Some(time) = attack {
            if let Some(seq) = sk.find_sequence("hunter_pummel") {
                layers.push((seq, time, true));
            }
        }
        let pose = sk.pose_layers_params(
            base,
            clock,
            true,
            &layers,
            &[
                ("body_pitch", 0.),
                ("body_yaw", 0.),
                ("move_x", 0.),
                ("move_y", 0.),
                ("dismount", 0.),
            ],
        );
        let Some(draw) = model.draws.get(draw) else {
            return;
        };
        let globals = sk
            .bones
            .iter()
            .zip(pose)
            .map(|(b, m)| (b.name.to_ascii_lowercase(), m))
            .collect();
        let (positions, normals) = skin(draw, &globals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    /// Model-space (render units) position of a named attachment while a
    /// scripted sequence plays, matching `scripted` for the same time.
    pub fn scripted_attachment(
        &self,
        path: &str,
        name: &str,
        time: f32,
        attachment: &str,
    ) -> Option<Vec3> {
        let model = self.models.get(path)?;
        let seq = model.skeleton.find_sequence(name)?;
        let pose = model.skeleton.pose_sequence(seq, time, false);
        let matrix = model.skeleton.attachment_matrix(attachment, &pose)?;
        Some(Vec3::from_array(crate::binary::source_position(
            matrix.w_axis.truncate().to_array(),
        )))
    }
    pub fn scripted_merged(
        &self,
        path: &str,
        mesh_path: &str,
        draw: usize,
        name: &str,
        time: f32,
        mesh: &mut Mesh,
    ) {
        self.scripted_params(path, mesh_path, draw, name, time, &[], mesh);
    }
    /// Bone pose (lowercase bone name -> model-space matrix) of a scripted
    /// sequence at `time`.
    fn scripted_globals(&self, path: &str, name: &str, time: f32, params: &[(&str, f32)]) -> Option<HashMap<String, XMat>> {
        let model = self.models.get(path)?;
        let seq = model.skeleton.find_sequence(name)?;
        let pose = model.skeleton.pose_sequence_params(seq, time, model.skeleton.sequences()[seq].looping(), params);
        Some(
            model
                .skeleton
                .bones
                .iter()
                .zip(pose)
                .map(|(b, mat)| (b.name.to_ascii_lowercase(), mat))
                .collect(),
        )
    }
    /// GPU-skinned `scripted_merged`: the first call gives the draw's mesh
    /// joint attributes and a `SkinnedMesh` (once); later calls only move
    /// its joints. CPU skinning rewrote and re-uploaded every intro/scripted
    /// mesh each frame (c1m1 intro: ~20-27 ms of prepare_assets per frame).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn scripted_gpu(
        &self,
        commands: &mut Commands,
        entity: Entity,
        gpu: Option<&GpuSkin>,
        joints: &mut Query<&mut Transform, With<GpuBone>>,
        meshes: &mut Assets<Mesh>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
        mesh_handle: &Handle<Mesh>,
        (path, mesh_path, draw): (&str, &str, usize),
        (name, time): (&str, f32),
    ) {
        let (Some(globals), Some(source)) = (
            self.scripted_globals(path, name, time, &[]),
            self.models.get(mesh_path).and_then(|m| m.draws.get(draw)),
        ) else {
            return;
        };
        let merged = merged_bones(source, &globals);
        if let Some(gpu) = gpu {
            for (&joint, bone) in gpu.joints.iter().zip(merged) {
                if let Ok(mut transform) = joints.get_mut(joint) {
                    transform.set_if_neq(Transform::from_matrix(render_bone(bone)));
                }
            }
            return;
        }
        // Map props can share one mesh asset: skin a private copy, or the
        // other users would draw joint attributes without a SkinnedMesh
        // (skinned pipeline vs model-only bind group validation error).
        let Some(mut mesh) = meshes.get(mesh_handle).cloned() else {
            return;
        };
        let (indices, weights) = gpu_weights(source);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, source.mesh.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, source.mesh.normals.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(indices));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
        let _ = mesh.generate_skinned_mesh_bounds();
        let private = meshes.add(mesh);
        let nodes: Vec<Entity> = merged
            .into_iter()
            .chain(std::iter::once(XMat::IDENTITY))
            .map(|bone| {
                commands
                    .spawn((GpuBone, Transform::from_matrix(render_bone(bone)), ChildOf(entity)))
                    .id()
            })
            .collect();
        let mut bind = source.bones.iter().map(|b| render_bone(b.inverse)).collect::<Vec<_>>();
        bind.push(Mat4::IDENTITY);
        commands.entity(entity).insert((
            Mesh3d(private),
            SkinnedMesh {
                joints: nodes.clone(),
                inverse_bindposes: bindposes.add(SkinnedMeshInverseBindposes::from(bind)),
            },
            GpuSkin { joints: nodes },
            DynamicSkinnedMeshBounds,
        ));
    }
    /// `scripted_merged` with pose parameters for the sequence's blend grid.
    /// Without them a locomotion blend (move_x/move_y) samples its centre,
    /// the standing-still blend.
    #[allow(clippy::too_many_arguments)]
    pub fn scripted_params(
        &self,
        path: &str,
        mesh_path: &str,
        draw: usize,
        name: &str,
        time: f32,
        params: &[(&str, f32)],
        mesh: &mut Mesh,
    ) {
        let Some(model) = self.models.get(path) else {
            return;
        };
        let Some(seq) = model.skeleton.find_sequence(name) else {
            return;
        };
        let Some(draw) = self.models.get(mesh_path).and_then(|m| m.draws.get(draw)) else {
            return;
        };
        let pose = model.skeleton.pose_sequence_params(
            seq,
            time,
            model.skeleton.sequences()[seq].looping(),
            params,
        );
        let globals = model
            .skeleton
            .bones
            .iter()
            .zip(pose)
            .map(|(b, mat)| (b.name.to_ascii_lowercase(), mat))
            .collect();
        let (positions, normals) = skin(draw, &globals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    }
    /// Loads a prop model's skeleton and draws on first use, so a prop made at
    /// runtime (`prop_dynamic_create`, testscripts) can SetAnimation like a
    /// map prop (bmap.txt's jukebox: "jukeeey2,setanimation,diskspin").
    pub(crate) fn ensure_model(&mut self, assets: &mut SourceAssets, path: &str) {
        if self.models.contains_key(path) {
            return;
        }
        let assets = RefCell::new(assets);
        let Ok(skeleton) = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()) else { return };
        let Ok(parts) = crate::studio::load(&mut assets.borrow_mut(), path, 0, 0) else { return };
        let draws = parts.into_iter().map(|d| draw_bind(d, &skeleton)).collect();
        self.models.insert(path.to_string(), Model { skeleton, draws, physics: None }.into());
    }
    pub fn load(
        root: &Path,
        actors: &[String],
        weapons: &HashMap<String, crate::game_state::Weapon>,
        arms: &str,
    ) -> io::Result<Self> {
        crate::performance::memory_checkpoint("Animation library begin");
        let assets = RefCell::new(SourceAssets::new(root, vec![])?);
        let read = |p: &str| assets.borrow_mut().read(p).ok();
        let mut out = Self::default();
        out.gore = crate::infected_gore::Registry::load(&mut assets.borrow_mut())?;
        out.surfaces =
            crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap_or_default();
        let arms_skeleton =
            Skeleton::load_with(arms, read).map_err(|e| binary::invalid(e.to_string()))?;
        let mut paths = actors.to_vec();
        paths.extend(out.gore.paths());
        paths.extend(crate::special_infected::boomer_gibs::paths());
        paths.extend(
            weapons
                .values()
                .filter(|w| !w.view_model.is_empty())
                .map(|w| w.view_model.clone()),
        );
        paths.extend(
            weapons
                .values()
                .filter(|w| !w.player_model.is_empty())
                .map(|w| w.player_model.clone()),
        );
        paths.sort();
        if let Some(pistol) = weapons.get("weapon_pistol") {
            if !pistol.player_model.is_empty() {
                paths.push(pistol.player_model.clone());
            }
        }
        paths.sort();
        paths.dedup();
        // Map preparation overlaps the old library. Share immutable models
        // instead of doubling actor/weapon geometry and skeleton allocations.
        type ModelCache = HashMap<(std::path::PathBuf, String, String), std::sync::Weak<Model>>;
        static CACHE: std::sync::OnceLock<std::sync::Mutex<ModelCache>> = std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let mut reused = 0;
        for path in paths {
            let arm_key = if weapons.values().any(|w| w.view_model == path) { arms } else { "" };
            let cache_key = (root.to_path_buf(), path.clone(), arm_key.to_owned());
            if let Some(model) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key).and_then(std::sync::Weak::upgrade) {
                out.models.insert(path, model);
                reused += 1;
                continue;
            }
            let mut skeleton = match Skeleton::load_with(&path, read) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("l4d2r animation {path}: {e}");
                    continue;
                }
            };
            if weapons.values().any(|w| {
                w.view_model == path
                    && matches!(
                        w.class.as_str(),
                        "weapon_first_aid_kit" | "weapon_defibrillator"
                    )
            }) {
                let character = if arms.contains("mechanic") {
                    "mechanic"
                } else if arms.contains("producer") {
                    "producer"
                } else if arms.contains("gambler") {
                    "gambler"
                } else if arms.contains("coach") {
                    "coach"
                } else if arms.contains("bill") {
                    "namvet"
                } else if arms.contains("zoey") {
                    "teenangst"
                } else if arms.contains("francis") {
                    "biker"
                } else {
                    "manager"
                };
                let animation = format!("models/survivors/anim_{character}.mdl");
                if let Some(bytes) = read(&animation) {
                    let ani = l4d2_formats::anim::anim_block_name(&bytes)
                        .ok()
                        .flatten()
                        .and_then(|p| read(&p));
                    skeleton
                        .add_include(&animation, &bytes, ani.as_deref())
                        .map_err(|e| binary::invalid(e.to_string()))?;
                }
            }
            let meshes = match crate::studio::load(&mut assets.borrow_mut(), &path, 0, 0) {
                Ok(meshes) => meshes,
                Err(e) if !weapons.values().any(|w| w.view_model == path) => {
                    eprintln!(
                        "l4d2r animation mesh {path}: {e}; retaining available static scene geometry"
                    );
                    continue;
                }
                Err(e) => return Err(binary::invalid(format!("Animation mesh {path}: {e}"))),
            };
            let mut draws: Vec<_> = meshes
                .into_iter()
                .map(|d| draw_bind(d, &skeleton))
                .collect();
            if !path.contains("v_claw_") && weapons.values().any(|w| w.view_model == path) {
                draws.extend(
                    crate::studio::load(&mut assets.borrow_mut(), arms, 0, 0)?
                        .into_iter()
                        .map(|d| draw_bind(d, &arms_skeleton)),
                );
            }
            println!(
                "l4d2r animation: {path}: {} sequences / {} draws",
                skeleton.sequences().len(),
                draws.len()
            );
            let physics = crate::physics_data::Profile::load(&mut assets.borrow_mut(), &path).ok();
            let model = std::sync::Arc::new(Model {
                    skeleton,
                    draws,
                    physics,
                });
            {
                let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                cache.retain(|_, model| model.strong_count() > 0);
                cache.insert(cache_key, std::sync::Arc::downgrade(&model));
            }
            out.models.insert(path, model);
        }
        println!("Animation transition cache: reused {reused}/{} models", out.models.len());
        crate::performance::memory_checkpoint("Animation library ready");
        Ok(out)
    }
}
pub fn medical_activity(class: &str, teammate: bool) -> &'static str {
    match class {
        "weapon_upgradepack_incendiary" | "weapon_upgradepack_explosive" => "ACT_VM_PRIMARYATTACK",
        "weapon_first_aid_kit" => {
            if teammate {
                "ACT_TERROR_HEAL_FRIEND"
            } else {
                "ACT_TERROR_HEAL_SELF"
            }
        }
        "weapon_defibrillator" => "ACT_TERROR_USE_DEFIBRILLATOR",
        "weapon_pain_pills" => "ACT_VM_USE",
        _ => "ACT_VM_PRIMARYATTACK",
    }
}
pub(crate) fn fire_sequence(sk: &Skeleton, class: &str, remaining: u32) -> Option<usize> {
    if class == "weapon_pistol_dual" {
        return choose(
            sk,
            match remaining {
                0 => &["fire_last_left", "ACT_VM_DRYFIRE_LEFT", "fire_left"],
                1 => &["fire_last_right", "ACT_VM_DRYFIRE", "fire_right"],
                n if n % 2 == 0 => &["fire_left", "ACT_VM_SECONDARYATTACK"],
                _ => &["fire_right", "ACT_VM_PRIMARYATTACK"],
            },
        );
    }
    choose(sk, &["ACT_VM_PRIMARYATTACK", "fire", "a_fire", "a_fire_1"])
}
fn choose(sk: &Skeleton, names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|name| sk.find_sequence(name))
        .or_else(|| {
            names.iter().find_map(|name| {
                let aliases: &[&str] = match *name {
                    "ACT_VM_DEPLOY" => &["ACT_VM_DRAW", "deploy"],
                    "ACT_VM_MELEE" => &[
                        "ACT_VM_SECONDARYATTACK",
                        "secondary_swing",
                        "ACT_VM_MELEE_LAYER",
                    ],
                    "ACT_VM_PRIMARYATTACK" => {
                        &["ACT_VM_HITCENTER", "ACT_VM_HITRIGHT", "fire", "swing_e_w"]
                    }
                    _ => &[],
                };
                aliases.iter().find_map(|n| sk.find_sequence(n))
            })
        })
        .or_else(|| {
            names.iter().find_map(|name| {
                sk.sequences().iter().position(|s| {
                    s.activity_name
                        .to_ascii_lowercase()
                        .starts_with(&format!("{}_", name.to_ascii_lowercase()))
                        && !s.activity_name.to_ascii_lowercase().contains("layer")
                })
            })
        })
}
/// The view model's item-reach step once the current clip has finished:
/// extend toward a focused pickup, hold the loop, retract when it is lost,
/// else the ordinary idle (None).
#[cfg(test)]
fn item_reach(activity: &str, focused: bool) -> Option<&'static [&'static str]> {
    hand_reach(activity, focused, false)
}
/// Like `item_reach`, with reviving a teammate taking the helping-hand set
/// (ACT_VM_HELPINGHAND_EXTEND/LOOP/RETRACT) first.
fn hand_reach(activity: &str, item: bool, helping: bool) -> Option<&'static [&'static str]> {
    const ITEM: [&[&str]; 3] = [
        &["ACT_VM_ITEMPICKUP_EXTEND", "item_extend"],
        &["ACT_VM_ITEMPICKUP_LOOP", "item_loop"],
        &["ACT_VM_ITEMPICKUP_RETRACT", "item_retract"],
    ];
    const HELP: [&[&str]; 3] = [
        &["ACT_VM_HELPINGHAND_EXTEND", "helping_hand_extend"],
        &["ACT_VM_HELPINGHAND_LOOP", "helping_hand_loop"],
        &["ACT_VM_HELPINGHAND_RETRACT", "helping_hand_retract"],
    ];
    let in_item = matches!(activity, "ACT_VM_ITEMPICKUP_EXTEND" | "ACT_VM_ITEMPICKUP_LOOP");
    let in_help = matches!(activity, "ACT_VM_HELPINGHAND_EXTEND" | "ACT_VM_HELPINGHAND_LOOP");
    // Leave the other reach first, then start this one.
    if helping {
        return Some(if in_item { ITEM[2] } else if in_help { HELP[1] } else { HELP[0] });
    }
    if in_help {
        return Some(HELP[2]);
    }
    match (item, in_item) {
        (true, true) => Some(ITEM[1]),
        (true, false) => Some(ITEM[0]),
        (false, true) => Some(ITEM[2]),
        (false, false) => None,
    }
}
fn reload_sequence(sk: &Skeleton, phase: u8, empty: bool) -> Option<usize> {
    match phase {
        2 => choose(sk, &["ACT_VM_RELOAD_LOOP", "reload_loop"]),
        3 => choose(sk, &["ACT_VM_RELOAD_END", "reload_end"]),
        _ if empty => choose(sk, &["ACT_VM_RELOAD_EMPTY", "reload_empty"])
            .or_else(|| choose(sk, &["ACT_VM_RELOAD", "reload", "a_reload"])),
        _ => choose(sk, &["ACT_VM_RELOAD", "reload", "a_reload"]),
    }
}
fn is_reload_sequence(sk: &Skeleton, index: usize) -> bool {
    sk.sequences().get(index).is_some_and(|s| {
        s.activity_name.contains("RELOAD") || s.label.to_ascii_lowercase().contains("reload")
    })
}
// Select among the retail activity's authored weights; keep the selection for its lifetime.
/// Survivor last-stand state machine. Returns (playback key, candidate
/// activities/labels, looping) or `None` when the survivor is not in, or
/// leaving, a last stand. `activity`/`clock` are the playback's current key
/// and time; `duration` is the current sequence's length.
fn incap_activity(
    incap: bool,
    reviving: bool,
    dead: bool,
    dual: bool,
    moving: bool,
    activity: &str,
    clock: f32,
    duration: f32,
) -> Option<(&'static str, &'static [&'static str], bool)> {
    const COLLAPSE: &[&str] = &["Collapse_to_Incap", "ACT_DIESIMPLE"];
    const IDLE: &[&str] = &[
        "ACT_IDLE_INCAP_PISTOL",
        "Idle_Incap_Pistol",
        "ACT_IDLE_INCAP",
    ];
    const IDLE_DUAL: &[&str] = &[
        "ACT_IDLE_INCAP_ELITES",
        "Idle_Incap_Elites",
        "ACT_IDLE_INCAP",
    ];
    const GETUP: &[&str] = &["ACT_TERROR_INCAP_TO_STAND", "GetUpFrom_Incap"];
    const DIE: &[&str] = &["ACT_DIE_INCAP", "Die_Incap"];
    let downed = activity == "incap/collapse" || activity == "incap/idle";
    if dead {
        // Death from standing or a special-infected pin must also leave
        // locomotion. An empty candidate list keeps the current pose until
        // physics takes it over; bleed-out plays the authored floor death.
        return Some(if downed || activity == "incap/die" {
            ("incap/die", DIE, false)
        } else {
            ("survivor/dead", &[], false)
        });
    }
    if incap {
        // Retail plays the get-up while being revived, so the survivor is on
        // their feet as the 5 s revive completes.
        if reviving {
            return Some(("incap/getup", GETUP, false));
        }
        if activity == "incap/idle"
            || activity == "incap/getup"
            || activity == "incap/collapse" && clock >= duration
        {
            return Some(("incap/idle", if dual { IDLE_DUAL } else { IDLE }, true));
        }
        return Some(("incap/collapse", COLLAPSE, false));
    }
    // Revived: get up from the floor unless the bot is already running.
    if downed || activity == "incap/getup" && clock < duration && !moving {
        if moving && downed {
            return None;
        }
        return Some(("incap/getup", GETUP, false));
    }
    None
}
fn weighted_activity(sk: &Skeleton, activity: &str, seed: u64) -> Option<usize> {
    let choices: Vec<_> = sk
        .sequences()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.activity_name.eq_ignore_ascii_case(activity))
        .map(|(i, s)| (i, s.activity_weight.unsigned_abs().max(1) as u64))
        .collect();
    let total: u64 = choices.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let mut value = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        % total;
    for (i, weight) in choices {
        if value < weight {
            return Some(i);
        }
        value -= weight;
    }
    None
}
// Geometry starts in our renderer's bind space, not yesterday's deformed mesh.
fn merge_skeleton_pose(parent: &Skeleton, source: &[XMat], target: &Skeleton) -> Vec<XMat> {
    let globals = parent.bones.iter().zip(source).map(|(b,m)| (b.name.to_ascii_lowercase(), *m)).collect::<HashMap<_,_>>();
    let mut merged = Vec::with_capacity(target.bones.len());
    for bone in &target.bones {
        let matrix = globals.get(&bone.name.to_ascii_lowercase()).copied().unwrap_or_else(|| {
            let bind = bone.pose_to_bone.inverse();
            if bone.parent < 0 { bind } else {
                merged[bone.parent as usize] * target.bones[bone.parent as usize].pose_to_bone * bind
            }
        });
        merged.push(matrix);
    }
    merged
}
fn skin(draw: &Draw, globals: &HashMap<String, XMat>) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    // Arms have helper/twist bones absent from the weapon skeleton. Their local
    // transforms must follow the merged animated parent, not the bind-space parent.
    let mut merged: Vec<XMat> = Vec::with_capacity(draw.bones.len());
    let mut palette = Vec::with_capacity(draw.bones.len());
    let source_to_render = XMat::from_cols(
        l4d2r_math::Vec4::new(0.0254, 0., 0., 0.),
        l4d2r_math::Vec4::new(0., 0., -0.0254, 0.),
        l4d2r_math::Vec4::new(0., 0.0254, 0., 0.),
        l4d2r_math::Vec4::W,
    );
    let render_to_source = source_to_render.inverse();
    for bone in &draw.bones {
        let global = globals.get(&bone.name).copied().unwrap_or_else(|| {
            if bone.parent < 0 {
                bone.local
            } else {
                merged[bone.parent as usize] * bone.local
            }
        });
        merged.push(global);
        palette.push(source_to_render * global * bone.inverse * render_to_source);
    }
    let mut positions = Vec::with_capacity(draw.mesh.positions.len());
    let mut normals = Vec::with_capacity(draw.mesh.positions.len());
    for i in 0..draw.mesh.positions.len() {
        let p = draw.mesh.positions[i];
        let p = XVec::from_array(p);
        let n = draw.mesh.normals[i];
        let n = XVec::from_array(n);
        let (ids, weights) = draw.mesh.weights[i];
        let mut pos = XVec::ZERO;
        let mut normal = XVec::ZERO;
        let mut sum = 0.;
        for k in 0..3 {
            if weights[k] > 0. {
                if let Some(m) = palette.get(ids[k] as usize) {
                    pos += m.transform_point3(p) * weights[k];
                    normal += m.transform_vector3(n) * weights[k];
                    sum += weights[k];
                }
            }
        }
        if sum > 0. {
            pos /= sum;
            normal = normal.normalize_or_zero();
        } else {
            pos = p;
            normal = n;
        }
        positions.push(pos.to_array());
        normals.push(normal.to_array());
    }
    (positions, normals)
}
#[cfg(test)]
mod merge_tests {
    use super::*;
    #[test]
    fn item_reach_extends_holds_and_retracts() {
        assert_eq!(item_reach("ACT_VM_IDLE", false), None);
        assert_eq!(item_reach("ACT_VM_IDLE", true).unwrap()[0], "ACT_VM_ITEMPICKUP_EXTEND");
        assert_eq!(item_reach("ACT_VM_ITEMPICKUP_EXTEND", true).unwrap()[0], "ACT_VM_ITEMPICKUP_LOOP");
        assert_eq!(item_reach("ACT_VM_ITEMPICKUP_LOOP", true).unwrap()[0], "ACT_VM_ITEMPICKUP_LOOP");
        assert_eq!(item_reach("ACT_VM_ITEMPICKUP_LOOP", false).unwrap()[0], "ACT_VM_ITEMPICKUP_RETRACT");
        assert_eq!(item_reach("ACT_VM_ITEMPICKUP_RETRACT", false), None);
        // After a shot (or any other clip) the reach starts over.
        assert_eq!(item_reach("ACT_VM_PRIMARYATTACK", true).unwrap()[0], "ACT_VM_ITEMPICKUP_EXTEND");
        // Reviving: the helping hand, after leaving an item reach.
        assert_eq!(hand_reach("ACT_VM_IDLE", true, true).unwrap()[0], "ACT_VM_HELPINGHAND_EXTEND");
        assert_eq!(hand_reach("ACT_VM_ITEMPICKUP_LOOP", true, true).unwrap()[0], "ACT_VM_ITEMPICKUP_RETRACT");
        assert_eq!(hand_reach("ACT_VM_HELPINGHAND_EXTEND", false, true).unwrap()[0], "ACT_VM_HELPINGHAND_LOOP");
        assert_eq!(hand_reach("ACT_VM_HELPINGHAND_LOOP", false, false).unwrap()[0], "ACT_VM_HELPINGHAND_RETRACT");
        assert_eq!(hand_reach("ACT_VM_HELPINGHAND_RETRACT", false, false), None);
    }
    #[test]
    fn transition_libraries_share_models_without_retaining_unused_models() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let actors = vec!["models/infected/boomer.mdl".to_string()];
        let weapons = HashMap::new();
        let first = AnimationLibrary::load(root, &actors, &weapons, "models/weapons/arms/v_arms_coach_new.mdl").unwrap();
        let second = AnimationLibrary::load(root, &actors, &weapons, "models/weapons/arms/v_arms_coach_new.mdl").unwrap();
        let a = &first.models[&actors[0]];
        assert!(std::sync::Arc::ptr_eq(a, &second.models[&actors[0]]));
        let weak = std::sync::Arc::downgrade(a);
        drop(first);
        assert!(weak.upgrade().is_some());
        drop(second);
        assert!(weak.upgrade().is_none(), "cache must not keep old models alive");
    }
    #[test]
    fn native_boomer_chunks_keep_current_pose_and_independent_snapshot_after_body_swap() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let mut library = AnimationLibrary::default();
        library.surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let parents = ["models/infected/boomer.mdl", "models/infected/boomette.mdl"];
        for path in parents.into_iter().map(str::to_owned).chain(crate::special_infected::boomer_gibs::paths()) {
            let skeleton = Skeleton::load_with(&path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            let draws = crate::studio::load(&mut assets.borrow_mut(), &path, 0, 0).unwrap().into_iter()
                .map(|d| draw_bind(d, &skeleton)).collect();
            let physics = Some(crate::physics_data::Profile::load(&mut assets.borrow_mut(), &path).unwrap());
            library.models.insert(path, Model { skeleton, draws, physics }.into());
        }
        let entity = Entity::PLACEHOLDER;
        let transform = Transform::from_translation(Vec3::new(3., 2., -1.)).with_rotation(Quat::from_rotation_y(1.2));
        for parent in parents {
            let sk = &library.models[parent].skeleton;
            let pose = sk.pose_sequence(0, 0.2, true);
            let mut playback = PlaybackState::default();
            playback.0.entry(entity).or_default().last_pose = pose.clone();
            let snapshot = library.freeze_gore_pose(parent, entity, &playback).unwrap();
            let corpse = crate::special_infected::boomer_gibs::corpse(parent).unwrap();
            let body = library.frozen_gib_body(parent, &snapshot, corpse, &transform, None).unwrap();
            library.replace_corpse(corpse, entity, body, &transform, &mut playback);
            assert_eq!(playback.0[&entity].last_pose.len(), library.models[corpse].skeleton.bones.len());
            let snapshot = library.remap_gore_pose(parent, &snapshot, corpse).unwrap();
            let parent = corpse;
            let sk = &library.models[parent].skeleton;
            for path in crate::special_infected::boomer_gibs::PARTS {
                let target = &library.models[path];
                assert!(!target.draws.is_empty());
                for solid in &target.physics.as_ref().unwrap().solids {
                    let mut index = target.skeleton.bones.iter().position(|b| b.name.eq_ignore_ascii_case(&solid.name)).unwrap();
                    loop {
                        let bone = &target.skeleton.bones[index];
                        if sk.bones.iter().any(|b| b.name.eq_ignore_ascii_case(&bone.name)) { break; }
                        assert!(bone.parent >= 0, "{parent}/{path}: {} has no animated anchor", solid.name);
                        index = bone.parent as usize;
                    }
                }
                // Unmatched helper bones follow the animated anchor's local
                // bind offset, exactly as mesh bonemerging already does.
                let mapped = merge_skeleton_pose(sk, &snapshot.pose, &target.skeleton);
                for (index, bone) in target.skeleton.bones.iter().enumerate() {
                    if bone.parent >= 0 && !sk.bones.iter().any(|b| b.name.eq_ignore_ascii_case(&bone.name)) {
                        let expected = mapped[bone.parent as usize] * target.skeleton.bones[bone.parent as usize].pose_to_bone * bone.pose_to_bone.inverse();
                        assert_eq!(mapped[index].to_cols_array(), expected.to_cols_array());
                    }
                }
                let still = library.frozen_gib_body(parent, &snapshot, path, &transform, None).unwrap();
                let mut launched = library.frozen_gib_body(parent, &snapshot, path, &transform, Some(Vec3::new(0., 0., 100.))).unwrap();
                // Force modifies historical poses/velocity, never current bind positions.
                for (a,b) in still.pose(&transform).iter().zip(launched.pose(&transform)) {
                    assert_eq!(a.to_cols_array(), b.to_cols_array(), "{parent}/{path}");
                }
                let before = launched.center();
                launched.tick(1. / 60., &crate::collision::Collision::test_solid_floor());
                assert!(launched.center().is_finite());
                assert!(launched.center().distance(before) > 0.001, "{path}: no inherited launch");
                assert!(launched.pose(&transform).iter().all(|m| m.is_finite()));
            }
        }
    }
    #[test]
    fn owned_wound_caps_keep_native_bone_bindings_and_triangle_topology() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let registry = crate::infected_gore::Registry::load(&mut assets.borrow_mut()).unwrap();
        let mut checked = 0;
        for (profile, parent) in [
            ("male", "models/infected/common_male01.mdl"),
            ("female", "models/infected/common_female01.mdl"),
            ("ceda", "models/infected/common_male_ceda.mdl"),
            ("laughriot", "models/infected/common_male_riot.mdl"),
        ] {
            let parent = Skeleton::load_with(parent, |p| assets.borrow_mut().read(p).ok()).unwrap();
            let pose = parent.pose_sequence(0, 0.23, true);
            let globals = parent.bones.iter().zip(&pose)
                .map(|(b, m)| (b.name.to_ascii_lowercase(), *m)).collect::<HashMap<_, _>>();
            for wound in registry.wounds.iter().filter(|w| w.profile == profile) {
                let sk = Skeleton::load_with(&wound.model, |p| assets.borrow_mut().read(p).ok()).unwrap();
                let meshes = crate::studio::load(&mut assets.borrow_mut(), &wound.model, 0, 0).unwrap();
                assert!(!meshes.is_empty(), "missing wound interior {}", wound.model);
                for mesh in meshes {
                    assert_eq!(mesh.positions.len() % 3, 0, "{}", wound.model);
                    for (ids, weights) in &mesh.weights {
                        for (id, weight) in ids.iter().zip(weights) {
                            if *weight > 0. {
                                let bone = &sk.bones[*id as usize];
                                assert!(globals.contains_key(&bone.name.to_ascii_lowercase()),
                                    "cap {} uses a bone absent from its parent: {}", wound.model, bone.name);
                            }
                        }
                    }
                    let draw = draw_bind(mesh, &sk);
                    let (positions, normals) = skin(&draw, &globals);
                    assert_eq!(positions.len(), draw.mesh.positions.len(), "cut cap triangles {}", wound.model);
                    assert!(positions.iter().chain(&normals).all(|p| p.iter().all(|v| v.is_finite())),
                        "invalid cap deformation {}", wound.model);
                    checked += 1;
                }
            }
        }
        assert!(checked >= 30, "owned cap audit did not exercise enough native models: {checked}");
    }
    #[test]
    fn shooting_wounds_never_replace_visible_vertex_colours_with_bind_coordinates() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let path = "models/infected/common_male_tshirt_cargos.mdl";
        let assets = RefCell::new(assets);
        let skeleton = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let mut assets = assets.into_inner();
        let draws = crate::studio::load(&mut assets, path, 0, 0).unwrap().into_iter()
            .map(|mesh| draw_bind(mesh, &skeleton)).collect::<Vec<_>>();
        let count = draws[0].mesh.positions.len();
        let mut library = AnimationLibrary::default();
        library.gore = crate::infected_gore::Registry::load(&mut assets).unwrap();
        library.models.insert(path.into(), Model { skeleton, draws, physics: None }.into());
        let mut mesh = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::MAIN_WORLD);
        let colours = vec![[0.2f32, 0.4, 0.6, 1.]; count];
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours.clone());
        // Simulate both the first hit and a second wound/material update.
        for wounds in [vec![0], vec![0, 1]] {
            library.cut_wounds(path, 0, &wounds, &mut mesh).unwrap();
            assert_eq!(mesh.attribute(Mesh::ATTRIBUTE_COLOR),
                Some(&VertexAttributeValues::Float32x4(colours.clone())),
                "wound upload leaked projection data into visible RGB");
            let Some(VertexAttributeValues::Float32x3(bind)) =
                mesh.attribute(crate::infected_gore::WOUND_BIND_POSITION) else {
                    panic!("wound coordinates must have their own vertex attribute");
                };
            assert_eq!(bind.len(), count);
            assert!(bind.iter().flatten().all(|v| v.is_finite()));
        }
    }
    #[test]
    fn gore_motion_samples_native_window_at_low_and_high_fps_without_pose_pop_impulses() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let bind = sk.bones.iter().map(|b| b.pose_to_bone.inverse()).collect::<Vec<_>>();
        for fps in [30, 60, 144, 300] {
            let mut p = Playback { last_pose: bind.clone(), ..default() };
            for _ in 0..fps * 2 {
                let clock = p.gore_clock + 1. / fps as f32;
                let root = Transform::from_translation(Vec3::X * clock * 3.)
                    .with_rotation(Quat::from_rotation_y(clock * 0.5));
                p.record_gore_pose(&root, 1. / fps as f32);
            }
            assert!(p.gore_history.len() <= 9, "unbounded pose cache at {fps}fps");
            let (pose, root, interval) = p.gore_sample(&sk).unwrap();
            assert!((interval - 0.1).abs() < 0.00001);
            assert!((root.translation.x - (p.gore_clock - 0.1) * 3.).abs() < 0.0001);
            assert!(root.rotation.angle_between(Quat::from_rotation_y((p.gore_clock - 0.1) * 0.5)) < 0.001);
            assert!(pose.iter().all(|m| m.is_finite()));
            let count = p.gore_history.len();
            p.record_gore_pose(&root, 0.);
            assert_eq!(count, p.gore_history.len(), "paused history advanced");
            p.sequence = 1;
            p.last_pose[0].w_axis.x += 100.;
            p.record_gore_pose(&root, 1. / fps as f32);
            let (pose, _, _) = p.gore_sample(&sk).unwrap();
            assert_eq!(pose, p.last_pose, "sequence transition created fake motion");
        }
    }

    #[test]
    fn last_stand_collapses_idles_gets_up_and_dies_incapacitated() {
        // Going down plays the collapse, then the pistol (or dual) idle.
        let (key, names, _) = incap_activity(true, false, false, false, false, "", 0., 0.).unwrap();
        assert_eq!(key, "incap/collapse");
        assert_eq!(names[0], "Collapse_to_Incap");
        assert_eq!(
            incap_activity(true, false, false, false, false, "incap/collapse", 0.5, 1.9)
                .unwrap()
                .0,
            "incap/collapse"
        );
        let (key, names, _) =
            incap_activity(true, false, false, false, false, "incap/collapse", 1.9, 1.9).unwrap();
        assert_eq!((key, names[0]), ("incap/idle", "ACT_IDLE_INCAP_PISTOL"));
        let (_, names, _) =
            incap_activity(true, false, false, true, false, "incap/idle", 9., 4.8).unwrap();
        assert_eq!(names[0], "ACT_IDLE_INCAP_ELITES");
        // Revived: the native get-up, interrupted once the bot runs.
        let (key, names, _) =
            incap_activity(false, false, false, false, false, "incap/idle", 3., 4.8).unwrap();
        assert_eq!(
            (key, names[0]),
            ("incap/getup", "ACT_TERROR_INCAP_TO_STAND")
        );
        assert!(incap_activity(false, false, false, false, true, "incap/getup", 1., 6.).is_none());
        assert!(incap_activity(false, false, false, false, false, "incap/getup", 6.1, 6.).is_none());
        // Bleeding out plays Die_Incap; direct/pinned deaths go to physics.
        assert_eq!(
            incap_activity(false, false, true, false, false, "incap/idle", 1., 4.8)
                .unwrap()
                .0,
            "incap/die"
        );
        assert!(incap_activity(false, false, false, false, false, "bot/shoot", 0., 1.).is_none());
        // Being revived gets up during the revive; a cancelled revive lies
        // back down to the idle.
        let (key, names, _) =
            incap_activity(true, true, false, false, false, "incap/idle", 2., 4.8).unwrap();
        assert_eq!((key, names[0]), ("incap/getup", "ACT_TERROR_INCAP_TO_STAND"));
        let (key, _, _) =
            incap_activity(true, false, false, false, false, "incap/getup", 2., 5.3).unwrap();
        assert_eq!(key, "incap/idle");
        for previous in [
            "",
            "bot/shoot",
            "smoker_victim/choke",
            "hunter_victim_idle",
            "survivor/dead",
        ] {
            assert_eq!(
                incap_activity(false, false, true, false, false, previous, 0., 1.)
                    .unwrap()
                    .0,
                "survivor/dead"
            );
        }
    }
    #[test]
    fn owned_native_ik_chains_and_mdl_ani_error_tracks_solve_finite_poses() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let mut report =
            String::from("model\tchains\tself_samples\tground_samples\trelease_samples\n");
        for path in [
            "models/v_models/v_rifle.mdl",
            "models/survivors/survivor_gambler.mdl",
            "models/infected/common_male01.mdl",
        ] {
            let sk = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            assert!(!sk.ik_chains.is_empty(), "{path}");
            let mut counts = [0usize; 7];
            for anim in 0..sk.anims().len() {
                for cycle in [0.25, 0.5, 0.75] {
                    let rules = sk
                        .animation_ik_rules_checked(anim, cycle)
                        .unwrap_or_else(|e| panic!("{path} anim {anim} cycle {cycle}: {e}"));
                    for rule in rules {
                        assert!(rule.target.is_finite() && rule.weight.is_finite());
                        if (0..7).contains(&rule.kind) {
                            counts[rule.kind as usize] += 1;
                        }
                        if rule.kind != 1 {
                            continue;
                        }
                        let mut pose =
                            sk.pose_anim(anim, cycle * sk.anims()[anim].duration(), false);
                        let chain = &sk.ik_chains[rule.chain];
                        let old = pose.clone();
                        let target = rule.bone.map_or(XMat::IDENTITY, |b| pose[b]) * rule.target;
                        assert!(sk.solve_ik(&mut pose, rule.chain, target, rule.weight));
                        assert!(pose.iter().all(|m| m.is_finite()));
                        for pair in chain.bones.windows(2) {
                            let before = old[pair[0]].w_axis.distance(old[pair[1]].w_axis);
                            let after = pose[pair[0]].w_axis.distance(pose[pair[1]].w_axis);
                            assert!(
                                (before - after).abs() < 0.002,
                                "{path} anim {anim} cycle {cycle} chain {} lengths {before} -> {after}",
                                chain.name
                            );
                        }
                    }
                }
            }
            if path.contains("v_rifle") {
                assert!(counts[1] > 0);
            }
            if path.contains("survivor_gambler") {
                assert!(counts[1] > 0, "external ANI self-target tracks absent");
            }
            report.push_str(&format!(
                "{path}\t{}\t{}\t{}\t{}\n",
                sk.ik_chains.len(),
                counts[1],
                counts[3],
                counts[4]
            ));
        }
        std::fs::create_dir_all("local-research/parity").unwrap();
        std::fs::write("local-research/parity/native-ik-coverage.tsv", report).unwrap();
    }
    #[test]
    fn dead_survivor_native_bodies_fall_from_standing_and_pin_poses() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let native = l4d2r_game_core::trace::Tracer::new(std::sync::Arc::new(
            l4d2_formats::bsp::Bsp::parse(std::fs::read("maps/rewrite_lab.bsp").unwrap()).unwrap(),
        ));
        let floor = crate::collision::Collision::test_native(std::sync::Arc::new(
            std::sync::Mutex::new(native),
        ));
        for character in ["coach", "gambler", "mechanic", "producer"] {
            let path = format!("models/survivors/survivor_{character}.mdl");
            let sk = Skeleton::load_with(&path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            let profile =
                crate::physics_data::Profile::load(&mut assets.borrow_mut(), &path).unwrap();
            for activity in ["Idle_Standing_pistol", "ACT_TERROR_CHOKING_TONGUE_GROUND"] {
                let seq = choose(&sk, &[activity]).unwrap();
                let pose = sk.pose_sequence(seq, 0.3, false);
                let transform = Transform::from_translation(Vec3::new(3.2512, 0.0015875, -7.2512));
                let mut body = crate::ragdoll::Body::new(&sk, &pose, &transform, Vec3::ZERO)
                    .with_profile(&sk, &profile, &surfaces);
                for _ in 0..180 {
                    body.tick(1. / 30., &floor);
                }
                let final_pose = body.pose(&transform);
                assert!(
                    final_pose.iter().all(|m| m.is_finite()),
                    "{character}/{activity}"
                );
                let head = sk
                    .bones
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case("ValveBiped.Bip01_Head1"))
                    .unwrap();
                let head_height = (transform.to_matrix() * render_bone(final_pose[head]))
                    .w_axis
                    .y;
                assert!(
                    (-0.1..0.8).contains(&head_height),
                    "{character}/{activity} did not settle on the floor: head {head_height}"
                );
            }
        }
    }
    #[test]
    fn worker_ragdolls_match_serial_body_poses() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let model = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(model, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let profile = crate::physics_data::Profile::load(&mut assets.borrow_mut(), model).unwrap();
        let surfaces = crate::physics_data::Surfaces::load(&mut assets.borrow_mut()).unwrap();
        let seq = sk.find_sequence("HeadshotFront").unwrap();
        let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
        let pose = sk.pose_sequence(seq, duration, false);
        let transform = Transform::from_translation(Vec3::Y * 2.);
        let make = || {
            (0..8)
                .map(|_| {
                    (
                        Entity::PLACEHOLDER,
                        crate::ragdoll::Body::new(&sk, &pose, &transform, Vec3::new(0.3, 1., -0.2))
                            .with_profile(&sk, &profile, &surfaces),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut serial = make();
        let mut parallel = make();
        let floor = crate::collision::Collision::test_solid_floor();
        let mut cache = RagdollWorkers::default();
        for _ in 0..60 {
            for (_, body) in &mut serial {
                body.tick(1. / 60., &floor);
            }
            tick_ragdolls(&mut parallel, &floor, 1. / 60., &mut cache);
        }
        for ((_, a), (_, b)) in serial.iter().zip(&parallel) {
            for (a, b) in a.pose(&transform).iter().zip(b.pose(&transform)) {
                assert_eq!(a.to_cols_array(), b.to_cols_array());
            }
        }
    }
    #[test]
    fn all_campaign_survivors_have_native_weapon_action_gestures() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let mut state = Match::new(&crate::game_state::tests_weapon());
        for character in ["gambler", "mechanic", "coach", "producer"] {
            let sk =
                Skeleton::load_with(&format!("models/survivors/survivor_{character}.mdl"), |p| {
                    assets.borrow_mut().read(p).ok()
                })
                .unwrap();
            for class in [
                "weapon_pistol",
                "weapon_pistol_dual",
                "weapon_pistol_magnum",
                "weapon_rifle",
                "weapon_smg",
                "weapon_hunting_rifle",
                "weapon_sniper_military",
                "weapon_pumpshotgun",
                "weapon_shotgun_chrome",
                "weapon_autoshotgun",
                "weapon_shotgun_spas",
            ] {
                state.pistol.class = class.into();
                let label = survivor_locomotion(&state, true);
                let activity = format!("ACT_{}", label.to_ascii_uppercase());
                let base = choose(&sk, &[&label, &activity])
                    .unwrap_or_else(|| panic!("{character} {class} locomotion"));
                for action in ["shoot", "shove", "reload"] {
                    for phase in 1..=3 {
                        state.reload_phase = phase;
                        let sequence = companion_weapon_action(&sk, &state, action, false)
                            .unwrap_or_else(|| {
                                panic!("{character} {class} {action} phase {phase}")
                            });
                        let duration = sk.anims()[sk.sequence_anim(sequence).unwrap()].duration();
                        for fraction in [0., 0.5, 1.] {
                            let pose = sk.pose_gesture_params(
                                base,
                                0.3,
                                sequence,
                                duration * fraction,
                                &[
                                    ("body_yaw", 0.),
                                    ("body_pitch", 0.),
                                    ("move_x", 1.),
                                    ("move_y", 0.),
                                ],
                            );
                            assert!(
                                pose.iter().all(|m| m.is_finite()),
                                "{character} {class} {action}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn common_infected_climb_activities_have_native_clips() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        for reaction in 4..=10 {
            let activity = crate::infected_movement::climb_activity(reaction);
            let sequence = weighted_activity(&sk, &activity, 0).expect(&activity);
            assert!(sk.sequence_anim(sequence).is_some());
        }
        // NAV ladder traversal (crate::ai_ladder).
        for activity in ["ACT_CLIMB_UP", "ACT_CLIMB_DOWN"] {
            let sequence = weighted_activity(&sk, activity, 0).expect(activity);
            assert!(sk.sequence_anim(sequence).is_some());
        }
    }
    #[test]
    fn campaign_survivors_and_ai_specials_have_animated_native_ladder_poses() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let paths = ["gambler", "mechanic", "coach", "producer", "namvet", "biker", "manager", "teenangst"]
            .map(|name| format!("models/survivors/survivor_{name}.mdl"))
            .into_iter().chain([
                crate::special_infected::SpecialClass::Hunter,
                crate::special_infected::SpecialClass::Smoker,
                crate::special_infected::SpecialClass::Boomer,
                crate::special_infected::SpecialClass::Spitter,
                crate::special_infected::SpecialClass::Jockey,
                crate::special_infected::SpecialClass::Charger,
                crate::special_infected::SpecialClass::Tank,
            ].map(|class| class.model().to_owned()));
        for path in paths {
            let sk = Skeleton::load_with(&path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            for descending in [false, true] {
                let activity = crate::ai_ladder::Pose { facing: Vec3::X, descending }.activity();
                let seq = choose(&sk, &[activity]).unwrap_or_else(|| panic!("{path}: {activity}"));
                let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
                println!("LADDERMETA {path} {activity} {:?} {:?}", sk.sequences()[seq].blend_params, sk.sequences()[seq].autolayers);
                let mut values = actor_parameters(&sk, &Transform::IDENTITY, Vec3::ZERO, Vec3::X);
                ladder_parameters(&mut values);
                let params = values.iter().map(|(n,v)| (n.as_str(), *v)).collect::<Vec<_>>();
                let first = sk.pose_sequence_params(seq, duration * 0.1, true, &params);
                let next = sk.pose_sequence_params(seq, duration * 0.37, true, &params);
                assert!(first.iter().chain(&next).all(|m| m.is_finite()), "{path}: {activity}");
                assert!(first.iter().zip(&next).any(|(a,b)| !a.abs_diff_eq(*b, 0.001)), "static {path}: {activity}");
                println!("LADDERPOSE {path} {activity} {} duration={duration}", sk.sequences()[seq].label);
            }
        }
    }
    #[test]
    fn new_equipped_items_do_not_replay_old_shoves_or_uses() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.shove_serial = 7;
        state.item_use_serial = 4;
        state.throw_stage = 3;
        for slot in 1..=5 {
            state.slot = slot;
            let mut playback = Playback::default();
            playback.seed_viewmodel_events(&state);
            assert_eq!(playback.shove_serial, state.shove_serial);
            assert_eq!(playback.item_use_serial, state.item_use_serial);
            assert_eq!(playback.throw_stage, state.throw_stage);
            playback.initialized = true;
            state.shove_serial += 1;
            playback.seed_viewmodel_events(&state);
            assert_ne!(
                playback.shove_serial, state.shove_serial,
                "new shove must still trigger"
            );
        }
    }
    #[test]
    fn owned_weapon_reload_and_shove_tracks_reach_finite_last_frames() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let classes = [
            "weapon_pistol",
            "weapon_pistol_dual",
            "weapon_pistol_magnum",
            "weapon_rifle",
            "weapon_rifle_ak47",
            "weapon_rifle_desert",
            "weapon_rifle_sg552",
            "weapon_smg",
            "weapon_smg_silenced",
            "weapon_smg_mp5",
            "weapon_pumpshotgun",
            "weapon_shotgun_chrome",
            "weapon_autoshotgun",
            "weapon_shotgun_spas",
            "weapon_hunting_rifle",
            "weapon_sniper_military",
            "weapon_sniper_scout",
            "weapon_sniper_awp",
            "weapon_first_aid_kit",
            "weapon_defibrillator",
            "weapon_pain_pills",
            "weapon_adrenaline",
            "weapon_pipe_bomb",
            "weapon_molotov",
            "weapon_vomitjar",
            "weapon_melee_fireaxe",
            "weapon_melee_crowbar",
            "weapon_melee_katana",
            "weapon_melee_machete",
            "weapon_melee_baseball_bat",
            "weapon_melee_cricket_bat",
            "weapon_melee_electric_guitar",
            "weapon_melee_frying_pan",
            "weapon_melee_golfclub",
            "weapon_melee_knife",
            "weapon_melee_pitchfork",
            "weapon_melee_shovel",
            "weapon_melee_tonfa",
        ];
        let weapons = classes
            .into_iter()
            .map(|c| {
                (
                    c.into(),
                    crate::game_state::Weapon::load(&mut assets, c).unwrap(),
                )
            })
            .collect();
        let library = AnimationLibrary::load(
            root,
            &[],
            &weapons,
            "models/weapons/arms/v_arms_mechanic_new.mdl",
        )
        .unwrap();
        let mut report = String::from("class\taction\tlabel\tduration\n");
        for weapon in weapons.values() {
            let sk = &library.models[&weapon.view_model].skeleton;
            let shove = choose(sk, &["ACT_VM_MELEE", "melee"]).unwrap();
            assert!(
                !is_reload_sequence(sk, shove),
                "{} shove must keep its own clock",
                weapon.class
            );
            let mut actions = vec![("shove", shove)];
            if !weapon.melee() && !weapon.medical() && !weapon.throwable() {
                for empty in [false, true] {
                    let seq = reload_sequence(sk, 0, empty).unwrap();
                    assert!(is_reload_sequence(sk, seq));
                    actions.push((if empty { "reload_empty" } else { "reload" }, seq));
                }
                if weapon.shell_reload() {
                    actions.push(("reload_loop", reload_sequence(sk, 2, false).unwrap()));
                    actions.push(("reload_end", reload_sequence(sk, 3, false).unwrap()));
                }
            }
            for (action, seq) in actions {
                let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
                assert!(duration > 0. && duration.is_finite());
                let mut changed = false;
                let start = sk.pose_retargeted_view_sequence(seq, 0., false);
                for fraction in [0., 0.25, 0.5, 0.75, 1.] {
                    let pose = sk.pose_retargeted_view_sequence(seq, duration * fraction, false);
                    if let Some(layer) =
                        sk.find_sequence(&format!("{}_layer", sk.sequences()[seq].label))
                    {
                        let layer_duration =
                            sk.anims()[sk.sequence_anim(layer).unwrap()].duration();
                        let layered =
                            sk.pose_view_sequence(layer, layer_duration * fraction, false);
                        for (a, b) in pose.iter().zip(layered) {
                            assert!(
                                a.to_cols_array()
                                    .iter()
                                    .zip(b.to_cols_array())
                                    .all(|(x, y)| (x - y).abs() < 0.001),
                                "{} {action} duplicated its native action layer",
                                weapon.class
                            );
                        }
                    }
                    assert!(
                        pose.iter().all(|p| p.is_finite()),
                        "{} {action}",
                        weapon.class
                    );
                    changed |= pose.iter().zip(&start).any(|(a, b)| {
                        a.to_cols_array()
                            .iter()
                            .zip(b.to_cols_array())
                            .any(|(x, y)| (x - y).abs() > 0.001)
                    });
                }
                assert!(changed, "{} {action} is frozen", weapon.class);
                report += &format!(
                    "{}\t{action}\t{}\t{duration}\n",
                    weapon.class,
                    sk.sequences()[seq].label
                );
            }
        }
        std::fs::write(
            "local-research/parity/reload-shove-native-tracks.tsv",
            report,
        )
        .unwrap();
    }
    #[test]
    fn native_included_parameters_and_parent_relative_transitions_remain_valid() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        for name in ["move_x", "move_y", "body_yaw", "body_pitch", "lean_yaw"] {
            assert!(
                sk.pose_params().iter().any(|p| p.name == name),
                "Missing included {name}"
            );
        }
        let transform = Transform::from_rotation(Quat::from_rotation_y(0.7));
        let values = actor_parameters(
            &sk,
            &transform,
            transform.rotation * Vec3::X,
            transform.rotation * Vec3::NEG_Z,
        );
        let value = |name: &str| values.iter().find(|(n, _)| n == name).unwrap().1;
        assert!((value("move_x") - 1.).abs() < 1e-5);
        assert!(value("move_y").abs() < 1e-5);
        assert!((value("body_yaw") - 90.).abs() < 1e-4);
        let sequence = sk.find_sequence("Run_01").unwrap();
        let from = sk.pose_sequence_params(sequence, 0.3, true, &[("body_yaw", 0.)]);
        let rotation = XMat::from_rotation_z(1.5);
        let mut to: Vec<_> = from.iter().map(|m| rotation * *m).collect();
        blend_local_pose(&sk, &from, &mut to, 0.5);
        for (i, bone) in sk.bones.iter().enumerate().filter(|(_, b)| b.parent >= 0) {
            let parent = bone.parent as usize;
            let before = (from[parent].inverse() * from[i])
                .w_axis
                .truncate()
                .length();
            let after = (to[parent].inverse() * to[i]).w_axis.truncate().length();
            assert!(
                (before - after).abs() < 0.001,
                "Transition shrank {}",
                bone.name
            );
            assert!(to[i].is_finite());
        }
    }
    #[test]
    fn native_hunter_pin_clips_and_blend_parameters_are_available() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        for (model, names) in [
            (
                crate::special_infected::HUNTER_MODEL,
                vec!["Pounce_01", "pounce_idle_01", "hunter_pummel"],
            ),
            (
                "models/survivors/survivor_producer.mdl",
                vec!["Collapse_to_Incap_Pounced", "Idle_Incap_Pounced"],
            ),
        ] {
            let sk = Skeleton::load_with(model, |p| assets.borrow_mut().read(p).ok()).unwrap();
            for name in names {
                let index = sk.find_sequence(name).expect(name);
                let s = &sk.sequences()[index];
                println!(
                    "PIN CLIP {model} {name}: axes={:?} flags={} weights={:?}",
                    s.blend_params,
                    s.flags,
                    s.weight_list.iter().copied().fold(f32::INFINITY, f32::min)
                );
                println!(
                    "PIN PARAMETERS {:?}; LAYERS {:?}",
                    sk.pose_params(),
                    s.autolayers
                );
                let pose = sk.pose_sequence(index, 0.5, true);
                for part in ["head", "pelvis", "hand"] {
                    for (i, b) in sk
                        .bones
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.name.to_ascii_lowercase().contains(part))
                        .take(2)
                    {
                        println!(
                            "PIN DIRECTION {name} {} {:?}",
                            b.name,
                            pose[i].w_axis.truncate()
                        );
                    }
                }
                assert!(pose.iter().all(|p| p.is_finite()));
            }
        }
    }
    #[test]
    fn grounded_pin_base_composes_native_pummel_without_airborne_pose() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with(crate::special_infected::HUNTER_MODEL, |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let base = sk.find_sequence("Idle_Crouching_01").unwrap();
        let attack = sk.find_sequence("hunter_pummel").unwrap();
        assert_eq!(sk.sequences()[base].activity_name, "ACT_CROUCHIDLE");
        assert_eq!(
            sk.sequences()[attack].activity_name,
            "ACT_TERROR_ATTACK_PUMMEL"
        );
        let parameters = [
            ("body_pitch", 0.),
            ("body_yaw", 0.),
            ("move_x", 0.),
            ("move_y", 0.),
        ];
        let crouch = sk.pose_sequence_params(base, 0.2, true, &parameters);
        let empty = sk.pose_layers_params(base, 0.2, true, &[], &parameters);
        let attacked = sk.pose_layers_params(base, 0.2, true, &[(attack, 0.2, false)], &parameters);
        assert!(
            empty
                .iter()
                .zip(&crouch)
                .all(|(a, b)| a.abs_diff_eq(*b, 0.001))
        );
        assert!(
            attacked
                .iter()
                .zip(&crouch)
                .any(|(a, b)| !a.abs_diff_eq(*b, 0.001))
        );

        assert!(attacked.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn survivor_feet_converge_at_the_retail_rate_and_snap_when_close() {
        // Standing: min(180 * scale, 100) deg/s; 90 degrees away is full scale.
        let next = converge_feet(90., 90., FEET_YAW_RATE, FEET_MAX_YAW_RATE, 0.1, 0.);
        assert!((next - 10.).abs() < 1e-4, "{next}");
        // Within 45 degrees the rate scales down (never below 1%).
        let next = converge_feet(9., 9., FEET_YAW_RATE, FEET_MAX_YAW_RATE, 0.1, 0.);
        assert!((next - 3.6).abs() < 1e-4, "{next}");
        // A step that would overshoot snaps to the goal; wrap-around is shortest.
        assert_eq!(converge_feet(1., 1., FEET_YAW_RATE, FEET_MAX_YAW_RATE, 0.5, 0.), 1.);
        let wrapped = converge_feet(-170., -170., FEET_YAW_RATE, FEET_MAX_YAW_RATE, 0.1, 170.);
        assert!(wrapped > 170. || wrapped < -170., "{wrapped}");
    }
    #[test]
    fn survivor_feet_hold_until_face_front_time_and_stay_within_ninety_degrees() {
        let mut s = SurvivorAnimState::default();
        let input = |now: f32, aim: Vec3| SurvivorInputs {
            now,
            dt: 0.1,
            velocity: Vec3::ZERO,
            aim,
            airborne: false,
            crouched: false,
            walking: false,
            limping: false,
            max_run_speed: SURVIVOR_RUN_SPEED,
            aim_yaw: None,
            face_front: false,
            pinned: false,
        };
        // Facing +x (Source yaw 0); turn the view 60 degrees left.
        s.update(&input(0., Vec3::X));
        let left = Vec3::new(0.5, 0., -(3f32.sqrt() / 2.));
        for k in 1..=10 {
            s.update(&input(k as f32 * 0.1, left));
        }
        assert!(s.feet_yaw.abs() < 1e-3, "feet hold for mp_facefronttime: {}", s.feet_yaw);
        // The head/body split carries the aim meanwhile (they sum to it).
        assert!((s.body_yaw + s.head_yaw - 60.).abs() < 6., "{} {}", s.body_yaw, s.head_yaw);
        for k in 11..=40 {
            s.update(&input(k as f32 * 0.1, left));
        }
        assert!((s.feet_yaw - 60.).abs() < 1e-3, "faces front after 2 s: {}", s.feet_yaw);
        // Looking 150 degrees away: the goal is clamped to 90 from the eye.
        let back = Vec3::new(-(3f32.sqrt() / 2.), 0., -0.5);
        s.update(&input(5., back));
        assert!((angle_normalize(s.goal_feet - 150.)).abs() <= 90. + 1e-3);
    }
    #[test]
    fn survivor_feet_face_front_while_busy_and_snap_when_pinned() {
        let input = |now: f32, aim: Vec3, face_front: bool, pinned: bool| SurvivorInputs {
            now,
            dt: 0.1,
            velocity: Vec3::ZERO,
            aim,
            airborne: false,
            crouched: false,
            walking: false,
            limping: false,
            max_run_speed: SURVIVOR_RUN_SPEED,
            aim_yaw: None,
            face_front,
            pinned,
        };
        let left = Vec3::new(0.5, 0., -(3f32.sqrt() / 2.));
        // Busy (healing, reviving, ladder): the feet head for the eyes at
        // once instead of waiting mp_facefronttime.
        let mut s = SurvivorAnimState::default();
        s.update(&input(0., Vec3::X, false, false));
        s.update(&input(0.1, left, true, false));
        assert!((angle_normalize(s.goal_feet - 60.)).abs() < 1e-3);
        assert!(s.feet_yaw > 0., "turning toward the eyes: {}", s.feet_yaw);
        // Pinned: feet snap to the eyes.
        let mut s = SurvivorAnimState::default();
        s.update(&input(0., Vec3::X, false, false));
        s.update(&input(0.1, left, false, true));
        assert!((s.feet_yaw - 60.).abs() < 1e-3, "{}", s.feet_yaw);
    }
    #[test]
    fn survivor_move_blend_is_feet_relative_with_rightward_positive_move_y() {
        let mut s = SurvivorAnimState::default();
        let mut input = SurvivorInputs {
            now: 0.,
            dt: 1. / 60.,
            velocity: Vec3::ZERO,
            aim: Vec3::X,
            airborne: false,
            crouched: false,
            walking: false,
            limping: false,
            max_run_speed: SURVIVOR_RUN_SPEED,
            aim_yaw: None,
            face_front: false,
            pinned: false,
        };
        s.update(&input);
        // Strafing right (Source -y = Bevy +z) at half run speed.
        input.velocity = Vec3::Z * 110. * 0.0254;
        let (base, move_x, move_y) = s.update(&input);
        assert_eq!(base, "RUN");
        assert!(move_x.abs() < 0.05 && (move_y - 0.5).abs() < 0.02, "{move_x} {move_y}");
        // Full speed forward clamps the ratio to 1.
        input.velocity = Vec3::X * 300. * 0.0254;
        let (_, move_x, _) = s.update(&input);
        assert!((move_x - 1.).abs() < 0.01, "{move_x}");
        // Airborne: JUMP for the first second only.
        input.airborne = true;
        input.now = 1.;
        assert_eq!(s.update(&input).0, "JUMP");
        input.now = 2.5;
        assert_ne!(s.update(&input).0, "JUMP");
        // Landing after more than 0.2 s airborne requests the landing gesture;
        // a shorter hop does not.
        input.airborne = false;
        s.update(&input);
        assert!(std::mem::take(&mut s.landed));
        input.airborne = true;
        input.now = 3.;
        s.update(&input);
        input.airborne = false;
        input.now = 3.1;
        s.update(&input);
        assert!(!s.landed);
    }
    /// Replays retail survivor-bot recordings (VScript m_flPoseParameter
    /// samples of Coach, see DEVELOPMENT.md) through SurvivorAnimState.
    fn retail_replay_errors(path: &str, until: f32) -> Option<[f32; 6]> {
        let text = std::fs::read_to_string(path).ok()?;
        // Coach pose order: body_pitch body_yaw head_pitch head_yaw move_y move_x.
        let walk = [120, 242];
        let mut samples = vec![];
        for line in text.lines().filter(|l| l.starts_with("ANIM ")) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let num = |i: usize| f[i].parse::<f32>().unwrap();
            if num(1) > until {
                break;
            }
            let e = f.iter().position(|x| *x == "eye")?;
            let pose: Vec<f32> = (5..11).map(num).collect();
            samples.push((num(1), f[2].parse::<i32>().ok()?, pose, num(e + 1), num(e + 2), num(e + 4), num(e + 5)));
        }
        let mut s = SurvivorAnimState::default();
        let mut errors = [0f32; 6];
        let mut count = 0.;
        let mut previous = samples.first()?.0;
        for (t, seq, pose, pitch, yaw, vx, vy) in &samples {
            let pitch = angle_normalize(*pitch).to_radians();
            let yaw = yaw.to_radians();
            let aim = Vec3::new(pitch.cos() * yaw.cos(), -pitch.sin(), -pitch.cos() * yaw.sin());
            let velocity = Vec3::new(*vx, 0., -*vy) * 0.0254;
            // Server animstate ticks at 30 Hz; samples arrive at ~15 Hz.
            let steps = (((t - previous) * 30.).round() as usize).max(1);
            let mut result = ("", 0., 0.);
            for k in 0..steps {
                let now = previous + (t - previous) * (k + 1) as f32 / steps as f32;
                result = s.update(&SurvivorInputs {
                    now,
                    dt: (t - previous) / steps as f32,
                    velocity,
                    aim,
                    airborne: false,
                    crouched: false,
                    walking: walk.contains(seq),
                    limping: false,
                    max_run_speed: SURVIVOR_RUN_SPEED,
                    aim_yaw: None,
                    face_front: false,
                    pinned: false,
                });
            }
            previous = *t;
            let degrees = |n: f32| -90. + 180. * n;
            // body_yaw is [-180, 180] in the anim_*.mdl includes that the
            // engine's merged pose parameters use; the rest are [-90, 90].
            let expected = [degrees(pose[0]), -180. + 360. * pose[1], degrees(pose[2]), degrees(pose[3]), -1. + 2. * pose[4], -1. + 2. * pose[5]];
            let actual = [s.body_pitch, s.body_yaw, s.head_pitch, s.head_yaw, result.2, result.1];
            if std::env::var_os("L4D2R_REPLAY_TRACE").is_some() {
                println!("TRACE t{t:.2} seq{seq} eye{:.1} feet{:.1} goal{:.1} by {:.1}/{:.1} hy {:.1}/{:.1} mx {:.2}/{:.2}", s.eye_yaw, s.feet_yaw, s.goal_feet, expected[1], actual[1], expected[3], actual[3], expected[5], actual[5]);
            }
            for i in 0..6 {
                errors[i] += (expected[i] - actual[i]).powi(2);
            }
            count += 1.;
        }
        Some(errors.map(|e| (e / count).sqrt()))
    }
    #[test]
    fn survivor_animstate_replays_retail_bot_recordings() {
        // The catch-up recording ends where the player fell out of the map and
        // the bot entered a non-locomotion state (face-front special state).
        for (path, until) in [
            ("local-research/parity/2026-10-22-retail-bot-animstate.log", f32::INFINITY),
            ("local-research/parity/2026-10-22-retail-bot-animstate-catchup.log", 504.9),
        ] {
            let Some(rms) = retail_replay_errors(path, until) else { continue };
            println!("RETAIL REPLAY {path}: rms body_pitch {:.2} body_yaw {:.2} head_pitch {:.2} head_yaw {:.2} (deg) move_y {:.3} move_x {:.3}", rms[0], rms[1], rms[2], rms[3], rms[4], rms[5]);
            assert!(rms[0] < 1.5 && rms[2] < 1.5, "pitch {rms:?}");
            assert!(rms[1] < 6. && rms[3] < 4., "yaw {rms:?}");
            assert!(rms[4] < 0.05 && rms[5] < 0.05, "move blend {rms:?}");
        }
    }
    #[test]
    fn survivor_translation_prefers_injured_then_calm_variants() {
        let names = |base, class, limping, calm| survivor_activity_names(base, class, "RIFLE", limping, calm);
        assert_eq!(names("RUN", "weapon_rifle", true, true)[0], "ACT_RUN_INJURED_RIFLE");
        assert_eq!(names("IDLE", "weapon_rifle", false, true)[0], "ACT_IDLE_CALM_RIFLE");
        assert_eq!(names("RUN", "weapon_smg", false, false), vec!["ACT_RUN_SMG".to_string()]);
        // Native table quirks: the auto shotgun's calm run is the pump's, snipers
        // jump as rifles, dual pistols use ELITES and their own jump, the
        // defibrillator shares the ELITES table and the first aid kit has no calm set.
        assert_eq!(names("RUN", "weapon_autoshotgun", false, true)[0], "ACT_RUN_CALM_PUMPSHOTGUN");
        assert_eq!(names("JUMP", "weapon_hunting_rifle", false, false)[0], "ACT_JUMP_RIFLE");
        assert_eq!(names("IDLE", "weapon_pistol_dual", false, true)[0], "ACT_IDLE_CALM_ELITES");
        assert_eq!(names("JUMP", "weapon_pistol_dual", false, false)[0], "ACT_JUMP_DUAL_PISTOL");
        assert_eq!(names("RUN", "weapon_defibrillator", false, false)[0], "ACT_RUN_ELITES");
        assert_eq!(names("IDLE", "weapon_first_aid_kit", false, true)[0], "ACT_IDLE_FIRSTAIDKIT");
        // Calm stops for the retail delays after firing or a weapon switch.
        let mut s = SurvivorAnimState::default();
        let mut state = Match::new(&crate::game_state::tests_weapon());
        s.update_calm(&state, 0., false);
        assert!(!s.calm(1.), "deploy delay");
        assert!(s.calm(2.));
        state.shot_serial += 1;
        s.update_calm(&state, 10., false);
        assert!(!s.calm(14.9) && s.calm(15.));
        // A lit flashlight blocks calm; it returns 4 s after it goes off
        // (retail: c2m1 bots outside the start area stay Idle_Standing).
        s.flashlight = true;
        for t in [20., 25., 30.] {
            s.update_calm(&state, t, false);
            assert!(!s.calm(t), "flashlight on at {t}");
        }
        s.flashlight = false;
        s.update_calm(&state, 31., false);
        assert!(!s.calm(33.9) && s.calm(34.));
    }
    #[test]
    fn survivor_gesture_slots_fade_like_retail_and_layer_independently() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/survivors/survivor_gambler.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let land = weighted_activity(&sk, "ACT_TERROR_JUMP_LANDING", 0).unwrap();
        let flinch = weighted_activity(&sk, "ACT_TERROR_FLINCH", 0).unwrap();
        let shoot = choose(&sk, &["ACT_PRIMARYATTACK_RIFLE"]).unwrap();
        let idle = choose(&sk, &["ACT_IDLE_RIFLE"]).unwrap();
        // Fade-in follows 3w^2 - 2w^3 over the sequence's own fade time, full
        // weight in the middle, then removal once the cycle passes 1.
        let seq = &sk.sequences()[land];
        let duration = sk.anims()[sk.sequence_anim(land).unwrap()].duration();
        assert!(duration > seq.fade_in + seq.fade_out, "{}", seq.label);
        let mut g = Gesture::default();
        g.restart(land);
        let (_, time, weight) = g.advance(&sk, seq.fade_in * 0.25).unwrap();
        assert!((time - seq.fade_in * 0.25).abs() < 1e-5);
        assert!((weight - 0.15625).abs() < 1e-4, "{weight}");
        let (_, _, weight) = g.advance(&sk, duration * 0.5 - seq.fade_in * 0.25).unwrap();
        assert_eq!(weight, 1.);
        let (_, _, weight) = g.advance(&sk, duration * 0.5 - seq.fade_out * 0.5).unwrap();
        assert!((weight - 0.5).abs() < 1e-3, "{weight}");
        assert!(g.advance(&sk, duration).is_none());
        assert!(!g.active);
        // Separate slots: a shot over a flinch keeps both contributions,
        // where the old single slot replaced one with the other.
        let values = [("move_x", 0.), ("move_y", 0.)];
        let flat = |pose: Vec<XMat>| pose.iter().flat_map(|m| m.to_cols_array()).collect::<Vec<_>>();
        let differs = |a: &[f32], b: &[f32]| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-3);
        let only_shot = flat(sk.pose_weighted_layers_params(idle, 0.3, true, &[(shoot, 0.05, false, 1.)], &values));
        let only_flinch = flat(sk.pose_weighted_layers_params(idle, 0.3, true, &[(flinch, 0.3, false, 1.)], &values));
        let both = flat(sk.pose_weighted_layers_params(
            idle,
            0.3,
            true,
            &[(shoot, 0.05, false, 1.), (flinch, 0.3, false, 1.)],
            &values,
        ));
        assert!(differs(&both, &only_shot) && differs(&both, &only_flinch));
        // Zero weight contributes nothing.
        let faded = flat(sk.pose_weighted_layers_params(
            idle,
            0.3,
            true,
            &[(shoot, 0.05, false, 1.), (flinch, 0.3, false, 0.)],
            &values,
        ));
        assert!(!differs(&faded, &only_shot));
    }
    /// `L4D2R_DUMP_MODEL=models/infected/hulk.mdl cargo test -- --ignored dump_model_sequences`
    #[test]
    #[ignore]
    fn dump_model_sequences() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = std::env::var("L4D2R_DUMP_MODEL").unwrap_or("models/infected/hulk.mdl".into());
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with(&path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        for p in sk.pose_params() {
            println!("POSE {} {} {}", p.name, p.start, p.end);
        }
        for (i, s) in sk.sequences().iter().enumerate() {
            let duration = sk.sequence_anim(i).map_or(0., |a| sk.anims()[a].duration());
            println!(
                "SEQ {i}\t{}\t{}\tw{}\tloop{}\tdelta{}\tdur {:.2}\tspeed {:.1}\tblend {:?}\tlayers {}\tfade {:.2}/{:.2}\tevents {:?}",
                s.label,
                s.activity_name,
                s.activity_weight,
                s.looping() as u8,
                s.is_delta() as u8,
                duration,
                sk.sequence_ground_speed(i),
                s.blend_params.iter().map(|b| b.as_ref().map(|b| b.0.clone())).collect::<Vec<_>>(),
                s.autolayers.len(),
                s.fade_in,
                s.fade_out,
                s.events.iter().map(|e| (e.event, e.cycle)).collect::<Vec<_>>(),
            );
        }
    }
    #[test]
    fn tank_running_punch_keeps_the_legs_running_and_moves_the_arms() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/hulk.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let run = weighted_activity(&sk, "ACT_RUN", 0).unwrap();
        let values = [("move_x", 1.), ("move_y", 0.)];
        let foot = sk.bone_index("ValveBiped.Bip01_R_Foot").unwrap();
        let hand = sk.bone_index("ValveBiped.Bip01_R_Hand").unwrap();
        let mut checked = 0;
        for seed in 0..12 {
            let punch = weighted_activity(&sk, "ACT_TERROR_ATTACK_MOVING", seed).unwrap();
            let duration = sk.anims()[sk.sequence_anim(punch).unwrap()].duration();
            let base = sk.pose_sequence_params(run, 0.7, true, &values);
            let layered = sk.pose_weighted_layers_params(run, 0.7, true, &[(punch, duration * 0.45, false, 1.)], &values);
            let moved = |i: usize| (base[i].w_axis - layered[i].w_axis).truncate().length();
            // Hulk_RunAttack gestures (the common picks) only drive the upper body.
            if sk.sequences()[punch].label.contains("RunAttack") {
                assert!(moved(foot) < 1., "{}: foot moved {}", sk.sequences()[punch].label, moved(foot));
                assert!(moved(hand) > 3., "{}: hand moved {}", sk.sequences()[punch].label, moved(hand));
                checked += 1;
            }
        }
        assert!(checked > 0);
    }
    #[test]
    #[ignore]
    fn nick_jacket_folds_by_sequence() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let model = "models/survivors/survivor_gambler";
        let sk = Skeleton::load_with(&format!("{model}.mdl"), |p| assets.borrow_mut().read(p).ok()).unwrap();
        let read = |p: &str| assets.borrow_mut().read(p).unwrap();
        let mdl = read(&format!("{model}.mdl"));
        let vvd = read(&format!("{model}.vvd"));
        let vtx = l4d2_formats::mdl::read_vtx(model, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let mesh = l4d2_formats::anim::load_skinned(&mdl, &vvd, &vtx).unwrap();
        let bind: Vec<XVec> = mesh.vertices.iter().map(|v| XVec::from(v.pos)).collect();
        let tris: Vec<[usize; 3]> = mesh.batches.iter()
            .filter(|b| b.material_index == 1)
            .flat_map(|b| b.indices.chunks(3).map(|t| [t[0] as usize, t[1] as usize, t[2] as usize]))
            .collect();
        let normal = |p: &[XVec], t: &[usize; 3]| (p[t[1]] - p[t[0]]).cross(p[t[2]] - p[t[0]]);
        let mut worst = Vec::new();
        for seq in 0..sk.sequences().len() {
            let pose = sk.pose_sequence(seq, 0.4, true);
            let skin = sk.skin_matrices(&pose);
            let posed: Vec<XVec> = l4d2_formats::anim::skin_cpu(&mesh, &skin).into_iter().map(XVec::from).collect();
            // A triangle folds when its posed normal turns against its bind normal
            // as rotated by its first vertex's dominant bone.
            let mut folds = Vec::new();
            for t in &tris {
                let b = normal(&bind, t);
                if b.length() < 1e-6 { continue; }
                let v = &mesh.vertices[t[0]];
                let bone = v.bone_ids[0] as usize;
                let expect = skin[bone].transform_vector3(b);
                let got = normal(&posed, t);
                if got.dot(expect) < 0. { folds.push(bind[t[0]]); }
            }
            worst.push((folds.len(), seq, folds));
        }
        // Folded triangles under the left arm in the standing pistol idle.
        for name in ["Idle_Calm_Pistol", "Idle_Standing_Pistol", "Idle_Standing_Elites", "Idle_Calm_Elites"] {
            let Some(seq) = sk.find_sequence(name) else { continue };
            let pose = sk.pose_sequence(seq, 0.4, true);
            let skin = sk.skin_matrices(&pose);
            let posed: Vec<XVec> = l4d2_formats::anim::skin_cpu(&mesh, &skin).into_iter().map(XVec::from).collect();
            let mut bones = std::collections::BTreeMap::<String, f32>::new();
            let mut count = 0;
            for t in &tris {
                let c = (bind[t[0]] + bind[t[1]] + bind[t[2]]) / 3.;
                if !(c.z > 38. && c.z < 58.) { continue; }
                let b = normal(&bind, t);
                if b.length() < 1e-6 { continue; }
                let v = &mesh.vertices[t[0]];
                let expect = skin[v.bone_ids[0] as usize].transform_vector3(b);
                if normal(&posed, t).dot(expect) >= 0. { continue; }
                count += 1;
                for &i in t {
                    let v = &mesh.vertices[i];
                    for k in 0..3 {
                        if v.weights[k] > 0. {
                            *bones.entry(sk.bones[v.bone_ids[k] as usize].name.clone()).or_default() += v.weights[k];
                        }
                    }
                }
            }
            let mut list: Vec<_> = bones.into_iter().collect();
            list.sort_by(|a, b| b.1.total_cmp(&a.1));
            println!("{name}: {count} folded chest/arm triangles; bones {:?}", &list[..list.len().min(8)]);
        }
        worst.sort_by(|a, b| b.0.cmp(&a.0));
        for (n, seq, folds) in worst.iter().take(15) {
            let s = &sk.sequences()[*seq];
            let c = folds.iter().fold(XVec::ZERO, |a, p| a + *p) / (folds.len().max(1) as f32);
            println!("{n:4} folds  {} ({})  centre {c:?}", s.label, s.activity_name);
        }
    }
    #[test]
    fn survivor_helper_bones_follow_their_control_bones() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/survivors/survivor_gambler.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let helpers: Vec<usize> = (0..sk.bones.len()).filter(|&i| sk.bones[i].quat_interp.is_some()).collect();
        assert_eq!(helpers.len(), 16, "Nick's shoulder/arm/knee helpers are parsed");
        let mut turned = 0.0f32;
        for name in ["Idle_Calm_Pistol", "Idle_Incap_Hanging1", "Heal_Incap_Standing_02", "GetUpFrom_Incap"] {
            let Some(seq) = sk.find_sequence(name) else { continue };
            for time in [0., 0.5, 1.5] {
                let pose = sk.pose_sequence(seq, time, false);
                for &h in &helpers {
                    let b = &sk.bones[h];
                    let parent = pose[b.parent as usize];
                    // Rigid follow (the old behaviour): parent * rest local.
                    let rigid = parent * XMat::from_rotation_translation(b.quat, b.pos);
                    let m = pose[h];
                    assert!(m.is_finite(), "{name} {}", b.name);
                    let gap = (m.w_axis - rigid.w_axis).truncate().length();
                    assert!(gap < 8., "{name} {} moved {gap} units off its parent", b.name);
                    let (_, a, _) = m.to_scale_rotation_translation();
                    let (_, r, _) = rigid.to_scale_rotation_translation();
                    turned = turned.max(a.angle_between(r).to_degrees());
                }
            }
        }
        println!("largest helper twist from rigid follow: {turned:.1} deg");
        assert!(turned > 5., "helpers are driven by their control bones");
    }
    #[test]
    fn turned_survivor_foot_locks_keep_the_authored_knees() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/survivors/survivor_gambler.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let seq = sk.find_sequence("Idle_Calm_Pistol").unwrap();
        let values = [("move_x", 0.), ("move_y", 0.)];
        let knee = |pose: &[XMat], side: &str| {
            let b = |n: &str| pose[sk.bone_index(&format!("ValveBiped.Bip01_{side}_{n}")).unwrap()].w_axis.truncate();
            let (t, c, f) = (b("Thigh"), b("Calf"), b("Foot"));
            (t - c).normalize().dot((f - c).normalize()).clamp(-1., 1.).acos().to_degrees()
        };
        let authored = sk.pose_sequence_params(seq, 0.4, true, &values);
        let rules = sk.ik_rules(seq, 0.4, true, &values);
        assert!(rules.iter().any(|r| r.kind == 1 && r.bone.is_none()), "foot locks are model-space IK_SELF rules");
        // The pose turned to the feet yaw, as the animation update does.
        let turn = XMat::from_rotation_z(40f32.to_radians());
        let turned: Vec<XMat> = authored.iter().map(|m| turn * *m).collect();
        let solve = |rules: &[l4d2_formats::anim::IkRule]| {
            let mut pose = turned.clone();
            crate::studio_ik::Context::default().apply(
                &sk,
                &mut pose,
                rules,
                &Transform::default(),
                &crate::collision::Collision::default(),
                true,
            );
            pose
        };
        let mut with_turn = rules.clone();
        for rule in with_turn.iter_mut().filter(|r| r.kind == 1 && r.bone.is_none()) {
            rule.target = turn * rule.target;
        }
        let fixed = solve(&with_turn);
        let stale = solve(&rules);
        for side in ["L", "R"] {
            assert!((knee(&fixed, side) - knee(&authored, side)).abs() < 0.5, "{side}: {} vs {}", knee(&fixed, side), knee(&authored, side));
        }
        // Unturned targets (the old behaviour) drag the feet and bend the knees.
        assert!(["L", "R"].iter().any(|s| (knee(&stale, s) - knee(&authored, s)).abs() > 2.));
    }
    #[test]
    fn common_infected_follow_the_retail_body_rules() {
        // Run speed: z_speed x [0.9, 1.1], stable per actor.
        let speeds: Vec<f32> = (0..2000u64).map(crate::infected_movement::run_speed).collect();
        let u = crate::collision::UNIT;
        assert!(speeds.iter().all(|s| (225. * u..=275. * u).contains(s)));
        assert!(speeds.iter().any(|s| *s < 230. * u) && speeds.iter().any(|s| *s > 270. * u));
        assert_eq!(crate::infected_movement::run_speed(7), crate::infected_movement::run_speed(7));
        // Playback: 1 unless faster than the blend's ground speed on the ground.
        assert_eq!(common_playback_rate(6., 7., true), 1.);
        assert!((common_playback_rate(6., 3., true) - 2.).abs() < 1e-6);
        assert_eq!(common_playback_rate(6., 3., false), 1.);
        assert_eq!(common_playback_rate(100., 1., true), 12.);
        // Lean: turning right at speed leans negative, then eases back.
        let t = Transform::default();
        let run = 250. * u;
        let mut lean = 0.;
        let v0 = Vec3::X * run;
        let v1 = Vec3::new(run, 0., 0.5 * run);
        for _ in 0..30 {
            lean = common_lean(lean, &t, v1, v0, run, 1. / 30.);
        }
        assert!(lean < -5., "{lean}");
        for _ in 0..90 {
            lean = common_lean(lean, &t, v1, v1, run, 1. / 30.);
        }
        assert!(lean.abs() < 0.5, "{lean}");
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male_tankTop_jeans.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        // move_x is velocity / run speed (half speed -> 0.5); move_y is rightward.
        let values = common_parameters(&sk, &t, Vec3::X * run * 0.5, Vec3::X, run, 0.);
        let get = |n: &str| values.iter().find(|(k, _)| k == n).unwrap().1;
        assert!((get("move_x") - 0.5).abs() < 1e-4);
        let right = common_parameters(&sk, &t, Vec3::Z * run, Vec3::X, run, 0.);
        assert!(right.iter().find(|(k, _)| k == "move_y").unwrap().1 > 0.99);
        // Run_04 ground speed at full forward is ~275 units/s: a 250 run
        // keeps playback 1 (the rewrite used to slow it to 0.91).
        let seq = sk.find_sequence("Run_04").unwrap();
        let gs = sk.sequence_ground_speed_params(seq, &[("move_x", 1.), ("move_y", 0.)]);
        assert!((gs - 274.8).abs() < 2., "{gs}");
        let slow = sk.sequence_ground_speed_params(seq, &[("move_x", 0.3), ("move_y", 0.)]);
        assert!(slow < gs * 0.5, "{slow}");
    }
    #[test]
    fn survivor_aim_override_turns_body_and_head_but_not_the_feet() {
        let mut s = SurvivorAnimState::default();
        let mut input = SurvivorInputs {
            now: 0.,
            dt: 1. / 30.,
            velocity: Vec3::ZERO,
            aim: Vec3::X,
            airborne: false,
            crouched: false,
            walking: false,
            limping: false,
            max_run_speed: SURVIVOR_RUN_SPEED,
            aim_yaw: Some(60.),
            face_front: false,
            pinned: false,
        };
        for i in 0..90 {
            input.now = i as f32 / 30.;
            s.update(&input);
        }
        // Eyes still face 0: the feet stay, body + head aim at the target.
        assert!(s.feet_yaw.abs() < 1e-3, "{}", s.feet_yaw);
        assert!((s.body_yaw + s.head_yaw - 60.).abs() < 0.5, "{} {}", s.body_yaw, s.head_yaw);
        input.aim_yaw = None;
        for i in 90..180 {
            input.now = i as f32 / 30.;
            s.update(&input);
        }
        assert!((s.body_yaw + s.head_yaw).abs() < 0.5);
    }
    #[test]
    fn survivors_aim_at_who_they_revive_heal_and_are_revived_by() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let pistol = crate::game_state::Weapon::load(&mut assets, "weapon_pistol").unwrap();
        let bot = |x: f32| crate::game_state::Companion {
            state: Match::new(&pistol),
            position: Vec3::new(x, 0., 0.),
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::X,
        };
        let mut squad = Squad(vec![bot(0.), bot(1.), bot(5.)]);
        let mut player = Match::new(&pistol);
        let feet = Some(Vec3::new(-1., 0., 0.));
        assert_eq!(survivor_aim_target(0, &squad, &player, feet), None);
        // Bot 1 is down and bot 0 (in reach) revives it.
        squad.0[1].state.incap = true;
        squad.0[1].revive_time = 1.;
        assert_eq!(survivor_reviver(1, &squad, &player, feet), Some(squad.0[0].position));
        assert_eq!(survivor_aim_target(0, &squad, &player, feet), Some(squad.0[1].position));
        assert_eq!(survivor_aim_target(2, &squad, &player, feet), None, "out of reach");
        // The player holding use takes over as the reviver.
        player.reviving = Some(1);
        assert_eq!(survivor_reviver(1, &squad, &player, feet), feet);
        assert_eq!(survivor_aim_target(0, &squad, &player, feet), None);
        // Healing a teammate or the player aims at them.
        player.reviving = None;
        squad.0[1].revive_time = 0.;
        squad.0[2].state.action = 3.;
        squad.0[2].state.medical_kind = 2;
        squad.0[2].state.medical_target = Some(1);
        assert_eq!(survivor_aim_target(2, &squad, &player, feet), Some(squad.0[1].position));
        squad.0[2].state.medical_kind = 5;
        assert_eq!(survivor_aim_target(2, &squad, &player, feet), feet);
        // A bot reviving the downed player aims at the player.
        squad.0[2].state.action = 0.;
        player.incap = true;
        squad.0[0].revive_time = 1.;
        assert_eq!(survivor_aim_target(0, &squad, &player, feet), feet);
    }
    #[test]
    fn survivor_translated_activities_resolve_on_the_owned_models() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let pistol =
            crate::game_state::Weapon::load(&mut assets.borrow_mut(), "weapon_pistol").unwrap();
        let mut state = Match::new(&pistol);
        for model in ["survivor_gambler", "survivor_producer", "survivor_coach", "survivor_mechanic"] {
            let sk = Skeleton::load_with(&format!("models/survivors/{model}.mdl"), |p| {
                assets.borrow_mut().read(p).ok()
            })
            .unwrap();
            for class in ["weapon_pistol", "weapon_pistol_dual", "weapon_rifle", "weapon_smg", "weapon_pumpshotgun", "weapon_autoshotgun", "weapon_hunting_rifle", "weapon_sniper_military"] {
                state.pistol.class = class.into();
                let suffix = survivor_activity_suffix(&state);
                for base in ["IDLE", "RUN", "JUMP"] {
                    for (limping, calm) in [(false, false), (true, false), (false, true)] {
                        let names = survivor_activity_names(base, class, &suffix, limping, calm);
                        assert!(
                            names.iter().any(|n| weighted_activity(&sk, n, 1).is_some()),
                            "{model} {class}: {names:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn survivor_locomotion_selects_owned_weapon_specific_native_clips() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/survivors/survivor_gambler.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let pistol =
            crate::game_state::Weapon::load(&mut assets.borrow_mut(), "weapon_pistol").unwrap();
        let mut state = Match::new(&pistol);
        for class in [
            "weapon_pistol",
            "weapon_pistol_dual",
            "weapon_rifle",
            "weapon_smg",
            "weapon_pumpshotgun",
            "weapon_shotgun_chrome",
            "weapon_autoshotgun",
            "weapon_hunting_rifle",
            "weapon_sniper_military",
        ] {
            state.pistol.class = class.into();
            for moving in [false, true] {
                let label = survivor_locomotion(&state, moving);
                assert!(
                    sk.find_sequence(&label).is_some(),
                    "Missing {class}: {label}"
                );
            }
        }
        state.slot = 4;
        assert_eq!(
            survivor_locomotion(&state, false),
            "Idle_Standing_firstaidkit"
        );
        assert!(
            sk.find_sequence(&survivor_locomotion(&state, true))
                .is_some()
        );
        state.slot = 3;
        state.throwable = Some("weapon_pipe_bomb".into());
        assert_eq!(survivor_locomotion(&state, true), "Run_grenade");
    }

    #[test]
    fn native_autolayers_remap_included_sequences_and_respond_to_named_aim_poses() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let seq = sk.find_sequence("Run_01").unwrap();
        let layers = &sk.sequences()[seq].autolayers;
        assert_eq!(layers.len(), 2);
        assert_eq!(
            sk.sequences()[layers[0].sequence].label,
            "common_male_aim_matrix_run"
        );
        assert_eq!(
            sk.sequences()[layers[1].sequence].label,
            "common_male_body_leans"
        );
        assert!(
            layers
                .iter()
                .all(|l| sk.sequences()[l.sequence].source == sk.sequences()[seq].source)
        );
        let sample = |yaw| {
            sk.pose_sequence_params(
                seq,
                0.3,
                true,
                &[("body_yaw", yaw), ("body_pitch", 0.), ("lean_yaw", 0.)],
            )
        };
        let left = sample(-60.);
        let right = sample(60.);
        assert!(left.iter().chain(&right).all(|m| m.is_finite()));
        let head = sk.bone_index("ValveBiped.Bip01_Head1").unwrap();
        assert!(
            left[head]
                .to_cols_array()
                .iter()
                .zip(right[head].to_cols_array())
                .any(|(a, b)| (*a - b).abs() > 0.01),
            "the native aim layer was ignored"
        );
        let foot = sk.bone_index("ValveBiped.Bip01_L_Foot").unwrap();
        assert_eq!(sk.sequences()[layers[0].sequence].weight_list[foot], 0.);
        let parent = sk.bones[foot].parent as usize;
        let foot_left = left[parent].inverse() * left[foot];
        let foot_right = right[parent].inverse() * right[foot];
        let foot_locked = layers.iter().any(|l| {
            sk.sequences()[l.sequence]
                .ik_locks
                .iter()
                .any(|lock| sk.ik_chains[lock.chain].bones[2] == foot)
        });
        if foot_locked {
            assert!(
                left[foot].w_axis.distance(right[foot].w_axis) < 0.001,
                "native foot lock failed across aim poses: {:?} / {:?}",
                left[foot].w_axis,
                right[foot].w_axis
            );
        } else {
            assert!(
                foot_left
                    .to_cols_array()
                    .iter()
                    .zip(foot_right.to_cols_array())
                    .all(|(a, b)| (*a - b).abs() < 0.001),
                "upper-body aim changed the local pose of a zero-weight foot"
            );
        }
        // Authored moving attacks carry nested delta and absolute arm tracks.
        for (i, s) in sk
            .sequences()
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.autolayers.is_empty())
        {
            for time in [0., 0.3, 1.7] {
                assert!(
                    sk.pose_sequence_params(i, time, true, &[("body_pitch", 0.), ("body_yaw", 0.)])
                        .iter()
                        .all(|m| m.is_finite()),
                    "{}",
                    s.label
                );
            }
        }
    }
    #[test]
    fn gpu_palette_matches_native_cpu_skin_including_zero_weight_vertices() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let path = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let meshes = crate::studio::load(&mut assets.borrow_mut(), path, 0, 0).unwrap();
        let seq = weighted_activity(&sk, "ACT_TERROR_RUN_INTENSE", 17).unwrap();
        let pose = sk.pose_sequence_params(seq, 0.35, true, &[("move_x", 1.), ("move_y", 0.)]);
        let globals = sk
            .bones
            .iter()
            .zip(pose)
            .map(|(b, m)| (b.name.to_ascii_lowercase(), m))
            .collect();
        let actor =
            Mat4::from_rotation_translation(Quat::from_rotation_y(0.8), Vec3::new(3., 2., -5.));
        for mesh in meshes {
            let mut draw = draw_bind(mesh, &sk);
            if let Some(w) = draw.mesh.weights.first_mut() {
                *w = ([0; 3], [0.; 3]);
            }
            let (cpu, _) = skin(&draw, &globals);
            let bones = merged_bones(&draw, &globals);
            let mut palette = bones
                .iter()
                .zip(&draw.bones)
                .map(|(p, b)| actor * render_bone(*p) * render_bone(b.inverse))
                .collect::<Vec<_>>();
            palette.push(actor);
            let (ids, weights) = gpu_weights(&draw);
            for i in (0..cpu.len()).step_by(37) {
                let point = Vec3::from_array(draw.mesh.positions[i]);
                let gpu = (0..4)
                    .map(|k| palette[ids[i][k] as usize].transform_point3(point) * weights[i][k])
                    .sum::<Vec3>();
                assert!(gpu.distance(actor.transform_point3(Vec3::from_array(cpu[i]))) < 0.0001);
            }
        }
    }
    #[test]
    fn native_partial_flinches_preserve_an_upright_locomotion_base() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let path = "models/infected/common_male01.mdl";
        let sk = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
        let base = weighted_activity(&sk, "ACT_TERROR_IDLE_ALERT", 1).unwrap();
        let mesh = crate::studio::load(&mut assets.borrow_mut(), path, 0, 0)
            .unwrap()
            .remove(0);
        let draw = draw_bind(mesh, &sk);
        let mut checked = 0;
        for seed in 0..32 {
            let hit = weighted_activity(&sk, "ACT_TERROR_FLINCH", seed).unwrap();
            let seq = &sk.sequences()[hit];
            if !seq.is_delta() && !seq.weight_list.iter().any(|w| *w < 0.99) {
                continue;
            }
            let pose = sk.pose_gesture(base, 0.2, hit, 0.15);
            let globals = sk
                .bones
                .iter()
                .zip(pose)
                .map(|(b, m)| (b.name.to_lowercase(), m))
                .collect();
            let (positions, _) = skin(&draw, &globals);
            assert!(positions.iter().all(|p| Vec3::from_array(*p).is_finite()));
            assert!(
                positions
                    .iter()
                    .map(|p| p[1])
                    .fold(f32::NEG_INFINITY, f32::max)
                    > 1.,
                "{} flattened the living zombie",
                seq.label
            );
            checked += 1;
        }
        assert!(checked > 0);
        assert!((0..sk.sequences().len()).any(|s| sk.sequence_ground_speed(s) > 10.));
    }
    #[test]
    fn survivor_medical_clips_keep_the_first_person_camera_and_geometry_in_range() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let weapons = HashMap::from_iter(
            ["weapon_first_aid_kit", "weapon_defibrillator"]
                .into_iter()
                .map(|c| {
                    (
                        c.to_string(),
                        crate::game_state::Weapon::load(&mut assets, c).unwrap(),
                    )
                }),
        );
        let library = AnimationLibrary::load(
            root,
            &[],
            &weapons,
            "models/weapons/arms/v_arms_mechanic_new.mdl",
        )
        .unwrap();
        for weapon in weapons.values() {
            let model = &library.models[&weapon.view_model];
            let sk = &model.skeleton;
            let camera = sk.bone_index("ValveBiped.Camera").unwrap();
            let idle = sk.pose_view_sequence(sk.find_sequence("ACT_VM_IDLE").unwrap(), 0., true);
            let sequence = sk
                .find_sequence(medical_activity(&weapon.class, false))
                .unwrap();
            for time in [0., 0.4, 1.2, 2.7] {
                let pose = sk.pose_retargeted_view_sequence(sequence, time, true);
                assert!(pose.iter().all(|m| m.is_finite()));
                assert!(pose[camera].w_axis.distance(idle[camera].w_axis) < 0.001);
                let globals = sk
                    .bones
                    .iter()
                    .zip(pose)
                    .map(|(b, m)| (b.name.to_lowercase(), m))
                    .collect::<HashMap<_, _>>();
                for draw in &model.draws {
                    let (positions, _) = skin(draw, &globals);
                    assert!(!positions.is_empty());
                    assert!(positions.iter().all(|p| Vec3::from_array(*p).length() < 5.));
                }
            }
        }
    }
    #[test]
    fn native_continuing_run_keeps_changing_after_multiple_sequence_durations() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let seq = weighted_activity(&sk, "ACT_TERROR_RUN_INTENSE", 1).unwrap();
        let duration = sk.anims()[sk.sequence_anim(seq).unwrap()].duration();
        let a = sk.pose_sequence_params(
            seq,
            duration * 3. + duration * 0.15,
            true,
            &[("move_x", 1.), ("move_y", 0.)],
        );
        let b = sk.pose_sequence_params(
            seq,
            duration * 3. + duration * 0.65,
            true,
            &[("move_x", 1.), ("move_y", 0.)],
        );
        assert!(a.iter().zip(&b).any(|(a, b)| {
            a.to_cols_array()
                .iter()
                .zip(b.to_cols_array())
                .any(|(a, b)| (*a - b).abs() > 0.001)
        }));
    }
    #[test]
    fn authored_viewmodel_layers_and_last_frames_are_finite() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        for path in [
            "models/v_models/v_pistola.mdl",
            "models/v_models/v_sniper_military.mdl",
            "models/v_models/v_pipebomb.mdl",
        ] {
            let sk = Skeleton::load_with(path, |p| assets.borrow_mut().read(p).ok()).unwrap();
            for seq in 0..sk.sequences().len() {
                let pose = sk.pose_view_sequence(seq, 0.4, false);
                assert!(
                    pose.iter().all(|m| m.is_finite()),
                    "{path} {}",
                    sk.sequences()[seq].label
                );
            }
        }
    }
    #[test]
    fn owned_viewmodel_muzzle_mounts_match_skinned_weapon_geometry() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let weapons = ["weapon_pistol", "weapon_pistol_dual", "weapon_rifle"]
            .into_iter()
            .map(|class| {
                (
                    class.to_string(),
                    crate::game_state::Weapon::load(&mut assets, class).unwrap(),
                )
            })
            .collect::<HashMap<_, _>>();
        let library = AnimationLibrary::load(
            root,
            &[],
            &weapons,
            "models/weapons/arms/v_arms_mechanic_new.mdl",
        )
        .unwrap();
        let mut report = String::new();
        for weapon in weapons.values() {
            let model = &library.models[&weapon.view_model];
            let sequence = choose(&model.skeleton, &["ACT_VM_IDLE", "idle"]).unwrap();
            let pose = model
                .skeleton
                .pose_retargeted_view_sequence(sequence, 0.4, true);
            let globals = model
                .skeleton
                .bones
                .iter()
                .zip(pose.iter())
                .map(|(b, p)| (b.name.to_ascii_lowercase(), *p))
                .collect();
            let (vertices, _) = skin(&model.draws[0], &globals);
            for a in model
                .skeleton
                .attachments
                .iter()
                .filter(|a| a.name.to_lowercase().contains("muzzle"))
            {
                let matrix = pose[a.bone] * a.local;
                let position = Vec3::from_array(crate::binary::source_position(
                    matrix.w_axis.truncate().to_array(),
                ));
                let distance = vertices
                    .iter()
                    .map(|v| position.distance(Vec3::from_array(*v)))
                    .fold(f32::INFINITY, f32::min);
                report.push_str(&format!("{} {} bone={} source_position={:?} nearest_vertex_metres={distance:.5} forward={:?}\n",weapon.class,a.name,model.skeleton.bones[a.bone].name,matrix.w_axis.truncate(),matrix.x_axis.truncate()));
                assert!(distance.is_finite());
            }
        }
        std::fs::write("local-research/parity/muzzle-mount-geometry.txt", report).unwrap();
    }
    #[test]
    fn survivor_pistol_bone_merge_keeps_the_barrel_forward_and_grip_near_the_hand() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let read = |p: &str| assets.borrow_mut().read(p).ok();
        let gun_path = "models/w_models/weapons/w_pistol_a.mdl";
        let gun = Skeleton::load_with(gun_path, read).unwrap();
        let meshes = crate::studio::load(&mut assets.borrow_mut(), gun_path, 0, 0).unwrap();
        let muzzle = gun
            .attachments
            .iter()
            .find(|a| a.name.to_ascii_lowercase().contains("muzzle"))
            .unwrap();
        for character in ["gambler", "coach", "mechanic", "producer"] {
            let sk =
                Skeleton::load_with(&format!("models/survivors/survivor_{character}.mdl"), read)
                    .unwrap();
            let seq = sk.find_sequence("Idle_Standing_pistol").unwrap();
            for time in [0., 0.35, 0.8] {
                let pose = sk.pose_sequence(seq, time, true);
                let globals: HashMap<_, _> = sk
                    .bones
                    .iter()
                    .zip(pose)
                    .map(|(b, m)| (b.name.to_ascii_lowercase(), m))
                    .collect();
                let hand = globals["valvebiped.bip01_r_hand"].w_axis.truncate();
                let weapon = globals["valvebiped.weapon_bone"];
                assert!(
                    weapon.w_axis.truncate().distance(hand) < 12.,
                    "{character}: grip outside hand"
                );
                let axis = (weapon * muzzle.local)
                    .transform_vector3(XVec::X)
                    .normalize();
                assert!(
                    axis.dot(XVec::X) > 0.7,
                    "{character}: sideways muzzle {axis:?}"
                );
                for mesh in &meshes {
                    let draw = draw_bind(mesh.clone(), &gun);
                    let (positions, _) = skin(&draw, &globals);
                    assert!(!positions.is_empty());
                    assert!(positions.iter().all(|p| Vec3::from_array(*p).is_finite()));
                }
            }
        }
    }
    #[test]
    fn installed_infected_activities_have_finite_changing_native_poses() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        for activity in [
            "ACT_TERROR_RUN_INTENSE",
            "ACT_TERROR_IDLE_ALERT",
            "ACT_TERROR_ATTACK_CONTINUOUSLY",
            "ACT_TERROR_SHOVED_BACKWARD",
            "ACT_TERROR_FLINCH",
            "ACT_TERROR_DIE_FROM_STAND",
            "ACT_TERROR_ATTACK_DOOR_CONTINUOUSLY",
            "ACT_TERROR_RUN_ON_FIRE",
        ] {
            let seq = weighted_activity(&sk, activity, 17).expect(activity);
            let a = sk.pose_sequence(seq, 0., false);
            let b = sk.pose_sequence(seq, 0.15, false);
            assert!(a.iter().chain(&b).all(|m| m.is_finite()), "{activity}");
            assert!(
                a.iter()
                    .zip(&b)
                    .any(|(a, b)| a.to_cols_array() != b.to_cols_array()),
                "{activity}"
            );
        }
        let variants = (0..64)
            .filter_map(|seed| weighted_activity(&sk, "ACT_TERROR_RUN_INTENSE", seed))
            .collect::<std::collections::HashSet<_>>();
        assert!(variants.len() > 1);
    }
    #[test]
    fn item_use_gestures_pick_crouched_variants_and_only_cut_backpack_actions() {
        assert_eq!(use_gesture(1, false), Some(("ACT_TERROR_HEAL_SELF", true)));
        assert_eq!(use_gesture(5, true), Some(("ACT_TERROR_CROUCH_HEAL_FRIEND", true)));
        assert_eq!(use_gesture(6, true), Some(("ACT_TERROR_CROUCH_USE_DEFIBRILLATOR", true)));
        assert_eq!(use_gesture(7, true), Some(("ACT_TERROR_USE_ADRENALINE_CROUCHING", false)));
        assert_eq!(use_gesture(8, false), Some(("ACT_TERROR_USE_PILLS", false)));
        assert_eq!(use_gesture(0, false), None);
        assert_eq!(
            use_gesture_activity(USE_REVIVE, true),
            ("ACT_TERROR_CROUCH_HEAL_INCAPACITATED", true)
        );
    }
    #[test]
    fn survivors_fidget_every_ten_to_twenty_seconds() {
        let intervals: Vec<f32> = (0..200).map(fidget_interval).collect();
        assert!(intervals.iter().all(|i| (10. ..=20.).contains(i)));
        assert!(intervals.contains(&10.) && intervals.contains(&20.));
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        for model in ["gambler", "producer", "coach", "mechanic", "namvet", "teenangst", "biker", "manager"] {
            let sk = Skeleton::load_with(&format!("models/survivors/survivor_{model}.mdl"), |p| {
                assets.borrow_mut().read(p).ok()
            })
            .unwrap();
            let variants = (0..64)
                .filter_map(|seed| weighted_activity(&sk, "ACT_TERROR_FIDGET", seed))
                .collect::<std::collections::HashSet<_>>();
            assert!(!variants.is_empty(), "{model} has no ACT_TERROR_FIDGET");
        }
    }
    #[test]
    fn unmatched_arm_helper_follows_animated_parent() {
        let draw = Draw {
            mesh: ModelMesh {
                lod: 0,
                checksum: 0,
                lighting_indices: vec![],
                material: String::new(),
                positions: vec![binary::source_position([2., 0., 0.])],
                normals: vec![[0., 1., 0.]],
                uv: vec![[0.; 2]],
                weights: vec![([1, 0, 0], [1., 0., 0.])],
            },
            bones: vec![
                BindBone {
                    name: "hand".into(),
                    parent: -1,
                    local: XMat::IDENTITY,
                    inverse: XMat::IDENTITY,
                },
                BindBone {
                    name: "sleeve_helper".into(),
                    parent: 0,
                    local: XMat::from_translation(XVec::X * 2.),
                    inverse: XMat::from_translation(-XVec::X * 2.),
                },
            ],
        };
        let mut pose = HashMap::from([("hand".into(), XMat::from_translation(XVec::Y * 5.))]);
        let (p, _) = skin(&draw, &pose);
        assert!(
            Vec3::from_array(p[0])
                .distance(Vec3::from_array(binary::source_position([2., 5., 0.])))
                < 1e-6
        );
        // A helper explicitly animated by the source pose still overrides inheritance.
        pose.insert(
            "sleeve_helper".into(),
            XMat::from_translation(XVec::X * 2. + XVec::Z * 3.),
        );
        let (p, _) = skin(&draw, &pose);
        assert!(
            Vec3::from_array(p[0])
                .distance(Vec3::from_array(binary::source_position([2., 0., 3.])))
                < 1e-6
        );
    }
}
#[derive(Default)]
pub(crate) struct RagdollWorkers {
    /// Round-robin position for the shared ragdoll step budget.
    next: usize,
    source: Option<crate::collision::Collision>,
    snapshots: Vec<(Option<(usize, u64)>, crate::collision::Collision)>,
}
impl RagdollWorkers {
    /// Drop copies of another map's collision world. They were only
    /// replaced when a ragdoll next ran, so after a map change the old map's
    /// collision and one full copy per worker stayed alive (~800 MB on c1m1,
    /// the iPhone campaign-change memory kill).
    pub(crate) fn forget_other_maps(&mut self, collision: &crate::collision::Collision) {
        let tag = collision.snapshot_tag().map(|t| t.0);
        if self.source.as_ref().and_then(|c| c.snapshot_tag()).map(|t| t.0) != tag {
            self.source = None;
            self.snapshots.clear();
        }
    }
}
fn tick_ragdolls(
    jobs: &mut [(Entity, crate::ragdoll::Body)],
    collision: &crate::collision::Collision,
    dt: f32,
    cache: &mut RagdollWorkers,
) {
    // Share the frame's solver budget round-robin, starting after the body
    // served last, so every corpse keeps moving while the total stays bounded.
    // Deferred bodies keep their time and catch up on a later frame.
    let mut run = vec![false; jobs.len()];
    let start = cache.next % jobs.len().max(1);
    let mut deferred = None;
    for k in 0..jobs.len() {
        let i = (start + k) % jobs.len();
        let body = &mut jobs[i].1;
        if crate::ragdoll::take_budget(body.steps_due(dt)) {
            run[i] = true;
        } else {
            body.defer(dt);
            deferred.get_or_insert(i);
        }
    }
    cache.next = deferred.unwrap_or(0);
    let mut ticking: Vec<&mut crate::ragdoll::Body> = jobs
        .iter_mut()
        .zip(&run)
        .filter(|(_, run)| **run)
        .map(|((_, body), _)| body)
        .collect();
    let jobs = &mut ticking[..];
    if jobs.len() < 4 || std::env::args().any(|a| a == "--serial-ragdolls") {
        for body in jobs {
            body.tick(dt, collision);
        }
        return;
    }
    let pool = bevy::tasks::ComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(4).build()
    });
    let workers = pool.thread_num().max(1).min(jobs.len());
    let tag = collision.snapshot_tag();
    // Retain the source allocation so an old map's pointer cannot be reused
    // while its worker snapshots are cached.
    if cache
        .source
        .as_ref()
        .and_then(|c| c.snapshot_tag())
        .map(|t| t.0)
        != tag.map(|t| t.0)
    {
        cache.source = Some(collision.clone());
    }
    for i in 0..workers {
        if i >= cache.snapshots.len() {
            cache.snapshots.push((tag, collision.worker_snapshot()));
        } else if tag.is_none() || cache.snapshots[i].0 != tag {
            cache.snapshots[i] = (tag, collision.worker_snapshot());
        }
    }
    let chunk = jobs.len().div_ceil(workers);
    pool.scope(|scope| {
        for (batch, (_, snapshot)) in jobs.chunks_mut(chunk).zip(cache.snapshots.iter()) {
            scope.spawn(async move {
                for body in batch {
                    body.tick(dt, snapshot);
                }
                crate::performance::flush_thread_metrics();
            });
        }
    });
}
pub fn update(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<Match>,
    rules: Res<Rules>,
    squad: Res<Squad>,
    library: Res<AnimationLibrary>,
    models: Res<RenderModels>,
    mut playback: ResMut<PlaybackState>,
    mut meshes: ResMut<Assets<Mesh>>,
    roots: Query<
        (
            Entity,
            &Transform,
            Option<&CompanionActor>,
            Option<&Enemy>,
            Option<&HeldWeapon>,
            &Children,
        ),
        (Without<crate::game::CompanionGun>, Without<GpuBone>),
    >,
    mut fresh: Query<(Entity, &mut Mesh3d, &ChildOf), Without<AnimatedDraw>>,
    draws: Query<(
        Entity,
        &AnimatedDraw,
        &Mesh3d,
        &ViewVisibility,
        Option<&crate::game::CompanionGun>,
        Option<&GpuSkin>,
    )>,
    mut sounds: Option<ResMut<crate::game_audio::Queue>>,
    targets: Query<&crate::game::Body>,
    physical: (
        ResMut<Assets<SkinnedMeshInverseBindposes>>,
        Query<&mut Transform, With<GpuBone>>,
        Res<crate::collision::Collision>,
        Option<Res<crate::render_warmup::Warmup>>,
        Option<Res<Time<Fixed>>>,
        Local<RagdollWorkers>,
        Query<&crate::special_infected::witch::WitchActivity>,
        Query<(&crate::special_infected::ai::AiActivity, Option<&crate::special_infected::ai::AiFlinch>)>,
        Query<Entity, With<crate::test_lab::ImpactTarget>>,
        Option<Res<crate::source_director::Director>>,
        Query<&crate::game::mind::Mind>,
        Option<Res<crate::l4d2r_navigation::Navigation>>,
        Option<Res<crate::l4d2r_cinematic::MapLogic>>,
        Option<ResMut<BotFlashlights>>,
        Option<ResMut<crate::game::MeleeHits>>,
    ),
) {
    let (
        mut inverse_bindposes,
        mut joints,
        collision,
        warmup,
        fixed,
        mut workers,
        witches,
        ai,
        impact_targets,
        director,
        minds,
        navigation,
        map_logic,
        mut bot_flashlights,
        mut melee_hits,
    ) = physical;
    workers.forget_other_maps(&collision);
    // survivor_calm_intensity: the Director's (single, approximate) intensity.
    let intensity = director.as_ref().map_or(0., |d| d.intensity);
    let _timing = crate::performance::scope("l4d2r_animation::update");
    for (root, _, companion, enemy, weapon, children) in &roots {
        for child in children {
            let Ok((entity, mut mesh, _)) = fresh.get_mut(*child) else {
                continue;
            };
            let path = if let Some(c) = companion {
                squad.0[c.0].model.clone()
            } else if let Some(enemy) = enemy {
                enemy.model.clone()
            } else if let Some(w) = weapon {
                rules
                    .weapons
                    .get(&w.0)
                    .unwrap_or(&rules.pistol)
                    .view_model
                    .clone()
            } else {
                continue;
            };
            let Some(model) = library.models.get(&path) else {
                continue;
            };
            let Some(handles) = models.0.get(&(path.clone(), 0, 0)) else {
                continue;
            };
            let Some(index) = handles.iter().position(|(h, _)| *h == mesh.0) else {
                continue;
            };
            if index >= model.draws.len() {
                continue;
            }
            let shareable = enemy.is_some()
                && path.starts_with("models/infected/common_")
                && !std::env::args().any(|a| a == "--profile-cpu-skin");
            // (registered, ready): share only once the shared copy is skinned.
            let (registered, ready) = SHARED_SKINS.lock().map_or((false, false), |s| {
                let h = s.by_source.get(&mesh.0.id());
                (h.is_some(), h.is_some_and(|h| s.ready.contains(&h.id())))
            });
            let shared = (shareable && ready).then_some(());
            if shared.is_some() {
                // Keep the unskinned template until the joints exist: the
                // shared mesh is swapped in together with its SkinnedMesh
                // (a skinned mesh drawn without joints is a GPU validation
                // error).
                commands.entity(entity).insert((
                    SharedSkin,
                    AnimatedDraw {
                        root,
                        model: path,
                        draw: index,
                    },
                    NoFrustumCulling,
                ));
            } else if let Some(original) = meshes.get(&mesh.0).cloned() {
                let source = mesh.0.id();
                mesh.0 = meshes.add(original);
                if shareable && !registered {
                    if let Ok(mut skins) = SHARED_SKINS.lock() {
                        // Drop entries whose template is gone (map changes).
                        skins.by_source.retain(|s, _| meshes.contains(*s));
                        let live: std::collections::HashSet<_> = skins.by_source.values().map(|h| h.id()).collect();
                        skins.ready.retain(|id| live.contains(id));
                        skins.by_source.insert(source, mesh.0.clone());
                    }
                    commands.entity(entity).insert(SharedSkin);
                }
                commands
                    .entity(entity)
                    .insert(AnimatedDraw {
                        root,
                        model: path,
                        draw: index,
                    })
                    .insert(NoFrustumCulling);
            }
        }
    }
    if state.paused && warmup.is_none() {
        return;
    }
    let dt = if state.paused { 0. } else { time.delta_secs() };
    let mut poses: HashMap<Entity, HashMap<String, XMat>> = HashMap::new();
    // Living infected, for the survivor calm "saw an enemy" delay.
    // Each with whether it is a Witch, for the bot flashlight.
    let enemy_positions: Vec<(Vec3, bool)> = roots
        .iter()
        .filter_map(|(entity, t, _, e, _, _)| {
            e.filter(|e| e.health > 0.).map(|_| (t.translation, witches.get(entity).is_ok()))
        })
        .collect();
    let map_generation = map_logic.as_ref().map_or(0, |l| l.generation);
    for (entity, transform, companion, enemy, weapon, _) in &roots {
        let path = if let Some(c) = companion {
            &squad.0[c.0].model
        } else if let Some(enemy) = enemy {
            &enemy.model
        } else if let Some(w) = weapon {
            &rules.weapons.get(&w.0).unwrap_or(&rules.pistol).view_model
        } else {
            continue;
        };
        let Some(model) = library.models.get(path) else {
            continue;
        };
        let sk = &model.skeleton;
        let p = playback.0.entry(entity).or_default();
        // Opt-in capture oracle: identical undeformed MDL pose in retail and
        // rewrite, so wound seams are compared independently of animation/IK.
        if impact_targets.contains(entity)
            && std::env::args().any(|a| a == "--capture-gore-bind-pose")
        {
            p.ragdoll = None;
            // L4D2 stores common meshes on a different model-space axis.
            // The native default reference sequence evaluates the root fixup;
            // inverse bind matrices alone leave the actor lying sideways.
            p.last_pose = sk.pose_sequence(0, 0., false);
            poses.insert(entity, sk.bones.iter().zip(&p.last_pose)
                .map(|(b, m)| (b.name.to_ascii_lowercase(), *m)).collect());
            continue;
        }
        let survivor_dead = companion.is_some_and(|c| squad.0[c.0].state.dead);
        let dead = survivor_dead || enemy.is_some_and(|e| e.health <= 0.);
        if dead && p.ragdoll.is_some() {
            // Articulated physics owns this pose now. Decoding the completed
            // death clip again would be discarded after the worker batch.
            p.clock += dt;
            if dt <= 0. {
                poses.insert(
                    entity,
                    sk.bones
                        .iter()
                        .zip(p.last_pose.iter())
                        .map(|(b, m)| (b.name.to_ascii_lowercase(), *m))
                        .collect(),
                );
            }
            continue;
        }
        let displacement = (transform.translation - p.position) * Vec3::new(1., 0., 1.);
        // Physics advances at 30Hz; rendering can advance much faster. A
        // render frame without a physics step must not flip running to idle.
        if p.initialized && ai.contains(entity) && dt > 0. {
            // AI specials are render-interpolated (special_infected::ai::
            // SmoothPose): they move every frame, not once per tick.
            let blend = 1. - (-dt / 0.1f32).exp();
            p.motion_velocity += (displacement / dt - p.motion_velocity) * blend;
            p.motion_speed = p.motion_velocity.length();
        } else if p.initialized && state.elapsed > p.simulation_clock + 1e-6 {
            p.motion_speed = displacement.length() / (state.elapsed - p.simulation_clock);
            p.motion_velocity = displacement / (state.elapsed - p.simulation_clock);
        }
        if state.elapsed > p.simulation_clock {
            p.simulation_clock = state.elapsed;
        }
        let speed = enemy.map_or(p.motion_speed, |e| e.movement_speed);
        let moving = p.initialized && speed > 0.08;
        let on_ladder = companion.is_some_and(|c| squad.0[c.0].state.ladder_pose.is_some())
            || enemy.is_some_and(|e| matches!(e.reaction,
                crate::infected_movement::LADDER_UP | crate::infected_movement::LADDER_DOWN))
            || ai.get(entity).is_ok_and(|a| matches!(a.0.0, "ACT_CLIMB_UP" | "ACT_CLIMB_DOWN"));
        let player_feet = targets.iter().next().map(|b| b.feet).filter(|_| !state.infected);
        let look = if on_ladder {
            transform.rotation * Vec3::X
        } else if let Some(c) = companion {
            let survivor = &squad.0[c.0].state;
            if let Some(reviver) = survivor
                .incap
                .then(|| survivor_reviver(c.0, &squad, &state, player_feet))
                .flatten()
            {
                reviver + Vec3::Y * 1.2 - (squad.0[c.0].position + Vec3::Y * 0.4)
            } else if survivor.pinned || survivor.dead || survivor.ladder_pose.is_some() {
                // Bound victim poses must not inherit the bot's last combat
                // aim, which twists the head/upper body back at its attacker.
                transform.rotation * Vec3::X
            } else {
                squad.0[c.0].aim
            }
        } else if enemy.is_some() {
            let mut closest = targets
                .iter()
                .next()
                .filter(|_| !state.dead)
                .map(|b| b.feet);
            for member in squad.0.iter().filter(|m| !m.state.dead) {
                if closest.is_none_or(|q| {
                    member.position.distance_squared(transform.translation)
                        < q.distance_squared(transform.translation)
                }) {
                    closest = Some(member.position);
                }
            }
            closest.map_or(transform.rotation * Vec3::X, |q| {
                q + Vec3::Y * 1.55 - (transform.translation + Vec3::Y * 1.55)
            })
        } else {
            transform.rotation * Vec3::X
        };
        if let Some(c) = companion {
            let member = &squad.0[c.0];
            let bot = &mut p.survivor;
            if member.state.dead || bot.map_generation != map_generation {
                // Retail clears the flag on spawn (103269d0).
                bot.left_checkpoint = false;
                bot.map_generation = map_generation;
            }
            if !member.state.dead && !bot.left_checkpoint {
                bot.left_checkpoint = navigation.as_ref().and_then(|n| n.checkpoint_at(member.position)) == Some(false);
            }
            bot.flashlight = !member.state.dead && bot.left_checkpoint && time.elapsed_secs() >= bot.witch_until;
            if let Some(lights) = bot_flashlights.as_mut() {
                if lights.0.len() <= c.0 {
                    lights.0.resize(c.0 + 1, false);
                }
                lights.0[c.0] = bot.flashlight;
            }
        }
        // Living, unbound survivor bots use the retail survivor animstate
        // (SurvivorAnimState); other states keep their authored poses.
        let survivor_motion = companion
            .filter(|c| {
                let s = &squad.0[c.0].state;
                !s.dead && !s.incap && !s.pinned && s.ladder_pose.is_none() && s.stagger <= 0.
            })
            .map(|c| {
                let member = &squad.0[c.0];
                let s = &member.state;
                let now = time.elapsed_secs();
                let mut saw = false;
                if now >= p.survivor.next_sight_check {
                    p.survivor.next_sight_check = now + 0.2;
                    let eye = member.position + Vec3::Y * 1.6;
                    let mut near: Vec<_> = enemy_positions
                        .iter()
                        .filter(|(e, _)| e.distance_squared(member.position) < 30. * 30.)
                        .collect();
                    near.sort_by(|a, b| {
                        a.0.distance_squared(member.position).total_cmp(&b.0.distance_squared(member.position))
                    });
                    // Nearest visible enemy (the bot's primary threat).
                    if let Some((_, witch)) = near
                        .into_iter()
                        .find(|(e, _)| collision.trace(eye, *e + Vec3::Y * 1.2, Vec3::ZERO).fraction >= 1.)
                    {
                        saw = true;
                        if *witch {
                            p.survivor.witch_until = now + FLASHLIGHT_WITCH_DELAY;
                        }
                    }
                }
                p.survivor.intense = intensity > CALM_INTENSITY;
                p.survivor.update_calm(s, now, saw);
                let limping = s.health + s.temporary < LIMP_HEALTH;
                let input = SurvivorInputs {
                    now,
                    dt,
                    velocity: p.motion_velocity,
                    aim: member.aim,
                    airborne: member.vertical.abs() > 1.,
                    crouched: false,
                    // Retail bots press walk to reposition (85 units/s) and run
                    // to catch up; the rewrite bot only has its speed.
                    walking: (p.motion_velocity * Vec3::new(1., 0., 1.)).length() / 0.0254
                        <= SURVIVOR_WALK_SPEED + 5.,
                    limping,
                    max_run_speed: if limping { SURVIVOR_LIMP_RUN_SPEED } else { SURVIVOR_RUN_SPEED },
                    aim_yaw: survivor_aim_target(c.0, &squad, &state, player_feet)
                        .map(|target| source_yaw(target - member.position)),
                    face_front: s.pinned
                        || s.action > 0.
                        || member.revive_time > 0.
                        || s.ladder_pose.is_some(),
                    pinned: s.pinned,
                };
                let (base, move_x, move_y) = p.survivor.update(&input);
                let names = survivor_activity_names(
                    base,
                    s.held_class(),
                    &survivor_activity_suffix(s),
                    limping,
                    p.survivor.calm(now),
                );
                (move_x, move_y, names)
            });
        if survivor_motion.is_none() && companion.is_some() {
            p.survivor.initialized = false;
        }
        let common = enemy.is_some_and(|e| e.health > 0.)
            && !ai.contains(entity)
            && witches.get(entity).is_err();
        let mut values = if let Some((move_x, move_y, _)) = &survivor_motion {
            p.survivor.parameters(sk, *move_x, *move_y)
        } else if common {
            let run = crate::infected_movement::run_speed(entity.to_bits());
            p.lean_yaw = common_lean(p.lean_yaw, transform, p.motion_velocity, p.lean_velocity, run, dt);
            p.lean_velocity = p.motion_velocity;
            common_parameters(sk, transform, p.motion_velocity, look, run, p.lean_yaw)
        } else {
            actor_parameters(sk, transform, p.motion_velocity, look)
        };
        if on_ladder { ladder_parameters(&mut values); }
        let parameters = values
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect::<Vec<_>>();
        let locomotion = companion.map(|c| survivor_locomotion(&squad.0[c.0].state, moving));
        let locomotion_activity = locomotion.as_ref().map(|name| {
            format!(
                "ACT_{}",
                name.to_ascii_uppercase().replace("IDLE_STANDING", "IDLE")
            )
        });
        let run_candidates = [
            locomotion.as_deref().unwrap_or("Run_Pistol"),
            locomotion_activity.as_deref().unwrap_or("ACT_RUN_PISTOL"),
            "ACT_RUN_PISTOL",
            "Run_Pistol",
            "ACT_WALK_PISTOL",
            "Walk_Pistol",
            "ACT_RUN",
            "ACT_WALK",
        ];
        let idle_candidates = [
            locomotion.as_deref().unwrap_or("Idle_Standing_pistol"),
            locomotion_activity.as_deref().unwrap_or("ACT_IDLE_PISTOL"),
            "Idle_Standing_pistol",
            "ACT_IDLE",
        ];
        let old_sequence = p.sequence;
        // Set by the branches that layer gesture slots (living survivors, AI
        // specials).
        let mut layer_gestures = false;
        // The survivor's main locomotion sequence: weighted once per activity.
        let survivor_sequence = survivor_motion.as_ref().and_then(|(_, _, names)| {
            if p.survivor.activity != names[0] || p.survivor.sequence.is_none() {
                let seed = entity.to_bits().wrapping_add((time.elapsed_secs() * 1000.) as u64);
                p.survivor.sequence = names.iter().find_map(|n| weighted_activity(sk, n, seed));
                p.survivor.activity = names[0].clone();
            }
            p.survivor.sequence
        });
        static VIEW_SAMPLE: std::sync::OnceLock<Option<(String, f32)>> = std::sync::OnceLock::new();
        let sample = VIEW_SAMPLE
            .get_or_init(|| {
                let args = std::env::args().collect::<Vec<_>>();
                let label = args
                    .windows(2)
                    .find(|a| a[0] == "--capture-view-sequence")?[1]
                    .clone();
                let time = args.windows(2).find(|a| a[0] == "--capture-view-time")?[1]
                    .parse::<f32>()
                    .ok()?;
                (time.is_finite() && time >= 0.).then_some((label, time))
            })
            .as_ref()
            .filter(|_| weapon.is_some());
        let mut sequence = if weapon.is_some() && state.infected {
            let duration = sk
                .sequence_anim(p.sequence)
                .map_or(0., |i| sk.anims()[i].duration());
            if !p.initialized {
                p.clock = 0.;
                p.activity = "hunter_deploy".into();
                choose(sk, &["ACT_VM_DEPLOY", "draw"]).unwrap_or(2)
            } else if p.activity == "hunter_deploy"
                && p.clock < duration
                && matches!(state.special_phase, 0 | 5)
            {
                p.sequence
            } else {
                let activity = state
                    .special_class
                    .vm_activity(state.special_phase)
                    .unwrap_or(match state.special_phase {
                        2 => "ACT_VM_LUNGE",
                        3 => "ACT_VM_PUMMEL_ATTACK",
                        4 => "ACT_VM_PRIMARYATTACK",
                        _ => "ACT_VM_IDLE",
                    });
                if p.activity != activity || p.shot_serial != state.shot_serial {
                    p.clock = 0.;
                }
                p.activity = activity.into();
                p.shot_serial = state.shot_serial;
                choose(sk, &[activity]).unwrap_or(2)
            }
        } else if weapon.is_some() {
            p.seed_viewmodel_events(&state);
            let gun = state.gun();
            let reload = state.reload > 0.;
            let duration = sk
                .sequence_anim(p.sequence)
                .and_then(|i| sk.anims().get(i))
                .map_or(0., |a| a.duration());
            let shove_sequence = choose(sk, &["ACT_VM_MELEE", "melee"]);
            let active_shove =
                p.initialized && shove_sequence == Some(p.sequence) && p.clock < duration;
            let resume_reload =
                reload && !is_reload_sequence(sk, p.sequence) && p.clock >= duration;
            let triggered = if state.shove_serial != p.shove_serial {
                shove_sequence
            } else if state.throw_stage != p.throw_stage
                && state.slot == 3
                && matches!(state.throw_stage, 1 | 3)
            {
                choose(
                    sk,
                    if state.throw_stage == 1 {
                        &["ACT_VM_PULLPIN", "pullpin"]
                    } else if state.throw_stage == 3 {
                        &["ACT_VM_THROW", "throw"]
                    } else {
                        &["ACT_VM_PULLPIN", "pullpin"]
                    },
                )
            } else if state.item_use_serial != p.item_use_serial && state.medical_kind > 0 {
                choose(
                    sk,
                    &[
                        medical_activity(state.held_class(), state.medical_kind == 2),
                        "primaryattack",
                        "use",
                    ],
                )
            } else if reload
                && !active_shove
                && (!p.reload
                    || state.reload_phase != p.reload_phase
                    || state.reload > p.reload_remaining + 0.001
                    || resume_reload)
            {
                reload_sequence(sk, state.reload_phase, state.reload_empty)
            } else if !p.initialized {
                choose(
                    sk,
                    &[
                        "ACT_VM_DEPLOY",
                        "ACT_VM_DRAW",
                        "ACT_VM_DEPLOY",
                        "ACT_VM_DRAW",
                        "deploy",
                        "a_deploy",
                    ],
                )
            } else if !active_shove
                && (gun.ammo < p.ammo
                    || (rules.weapons.get(&gun.class).is_some_and(|w| w.melee())
                        && state.cooldown > p.cooldown + 0.01))
            {
                // Melee: the script attack's own activity (slash1
                // ACT_VM_PRIMARYATTACK, slash2 ACT_VM_HITLEFT, ...).
                rules
                    .weapons
                    .get(&gun.class)
                    .filter(|w| w.melee())
                    .and_then(|w| w.melee_attacks.get(state.melee_attack as usize))
                    .and_then(|a| choose(sk, &[a.activity.as_str(), "ACT_VM_PRIMARYATTACK"]))
                    .or_else(|| fire_sequence(sk, &gun.class, gun.ammo))
            } else {
                None
            };
            p.ammo = gun.ammo;
            p.cooldown = state.cooldown;
            p.reload = reload;
            p.reload_remaining = state.reload;
            p.reload_phase = state.reload_phase;
            p.shove_serial = state.shove_serial;
            p.throw_stage = state.throw_stage;
            p.item_use_serial = state.item_use_serial;
            if let Some(seq) = triggered {
                p.clock = 0.;
                seq
            } else {
                let duration = sk
                    .sequence_anim(p.sequence)
                    .and_then(|i| sk.anims().get(i))
                    .map_or(0., |a| a.duration());
                if state.slot == 4 && state.action > 0. {
                    choose(
                        sk,
                        &[medical_activity(
                            state.held_class(),
                            state.medical_kind == 2,
                        )],
                    )
                    .unwrap_or(p.sequence)
                } else if reload
                    || (state.slot == 3 && state.throw_stage == 2)
                    || p.clock < duration
                        && !sk.sequences().get(p.sequence).is_some_and(|s| s.looping())
                {
                    p.sequence
                } else {
                    // Retail's free hand reaches toward a focused pickup
                    // (ACT_VM_ITEMPICKUP_EXTEND/LOOP/RETRACT on every weapon
                    // view model; live retail c1m2/c1m3/c3m1, 19 Nov).
                    let label = sk
                        .sequences()
                        .get(p.sequence)
                        .map(|s| s.activity_name.as_str())
                        .unwrap_or("");
                    hand_reach(label, state.hint.contains("Pick up"), state.reviving.is_some())
                        .and_then(|names| choose(sk, names))
                        .or_else(|| choose(sk, &["ACT_VM_IDLE", "a_idle_1", "idle", "a_idle", "idle1"]))
                        .unwrap_or(0)
                }
            }
        // Hanging from a ledge (online players): the grab, the stage's hang
        // loop, and the climb following the revive.
        } else if let Some((c, act)) = companion.and_then(|c| {
            let m = &squad.0[c.0];
            (!m.state.pinned).then_some(())?;
            crate::ledge_hang::activity(&m.state, m.revive_time).map(|act| (c, act))
        }) {
            let key = format!("ledge/{act}");
            if p.activity != key {
                p.activity = key;
                p.clock = 0.;
            }
            let seq = choose(sk, &[act]).unwrap_or(p.sequence);
            if act == "ACT_TERROR_LEDGE_CLIMB" {
                // Follow the revive: the climb ends with the 5 s save.
                let duration = sk.sequence_anim(seq).map_or(0., |i| sk.anims()[i].duration());
                p.clock = (squad.0[c.0].revive_time / 5.).clamp(0., 1.) * duration - dt;
            }
            seq
        // Last stand: collapse to the incapacitated pistol idle, the native
        // get-up after a revive, and Die_Incap if they bleed out. A pin's
        // own victim pose (pounce, tongue, pummel) takes precedence.
        } else if let Some((key, names, looping)) = companion.and_then(|c| {
            let s = &squad.0[c.0].state;
            let duration = sk
                .sequence_anim(p.sequence)
                .map_or(0., |i| sk.anims()[i].duration());
            incap_activity(
                s.incap && !s.pinned,
                s.incap && !s.pinned && squad.0[c.0].revive_time > 0.,
                s.dead,
                s.pistol.class == "weapon_pistol_dual",
                moving,
                &p.activity,
                p.clock,
                duration,
            )
        }) {
            if p.activity != key {
                p.activity = key.into();
                p.clock = 0.;
                if key == "incap/getup" {
                    // Three weighted retail get-ups (ACT_TERROR_INCAP_TO_STAND).
                    let seed = companion.map_or(0, |c| c.0 as u64).wrapping_add(
                        squad.0[companion.unwrap().0]
                            .state
                            .damage_serial
                            .wrapping_mul(7919),
                    );
                    p.locomotion_clock = 0.;
                    p.sequence = weighted_activity(sk, names[0], seed)
                        .or_else(|| choose(sk, names))
                        .unwrap_or(p.sequence);
                    p.sequence
                } else {
                    choose(sk, names).unwrap_or(p.sequence)
                }
            } else if key == "incap/getup" {
                let s = &squad.0[companion.unwrap().0];
                if s.state.incap {
                    // Follow the revive: the clip ends with the 5 s revive.
                    let duration = sk
                        .sequence_anim(p.sequence)
                        .map_or(0., |i| sk.anims()[i].duration());
                    p.clock = (s.revive_time / 5.).clamp(0., 1.) * duration - dt;
                }
                p.sequence
            } else {
                let _ = looping;
                choose(sk, names).unwrap_or(p.sequence)
            }
        } else if let Some((c, pose)) = companion.and_then(|c| {
            squad.0[c.0].state.ladder_pose.map(|pose| (c, pose))
        }) {
            let s = &squad.0[c.0].state;
            let activity = pose.activity();
            if p.activity != activity { p.clock = 0.; }
            p.activity = activity.into();
            // Actions that occurred before mounting must not replay on dismount.
            p.shot_serial = s.shot_serial;
            p.shove_serial = s.shove_serial;
            p.damage_serial = s.damage_serial;
            p.reload = s.reload > 0.;
            p.reload_phase = s.reload_phase;
            p.reload_remaining = s.reload;
            choose(sk, &[activity]).unwrap_or(p.sequence)
        // Jockey ride victim: native ACT_TERROR_JOCKEY_RIDDEN
        // (survivor_ridden_lower_walk with its survivor_ridden_upper layer).
        } else if companion.is_some_and(|c| {
            squad.0[c.0].state.pinned_by == Some(crate::special_infected::SpecialClass::Jockey)
        }) {
            if p.activity != "jockey_victim" {
                p.clock = 0.;
                p.activity = "jockey_victim".into();
            }
            choose(
                sk,
                &["ACT_TERROR_JOCKEY_RIDDEN", "survivor_ridden_lower_walk"],
            )
            .unwrap_or(p.sequence)
        // Charger carry/slam/pummel victims and impact-flung survivors.
        } else if let Some(seq) = companion.and_then(|c| {
            crate::special_infected::charger::victim_sequence(
                sk,
                c.0,
                &mut p.activity,
                &mut p.clock,
            )
        }) {
            seq
        // Survivor victim poses. Each pinning class keys its own branch on
        // `pinned_by`; this one is the Hunter pounce (and unattributed pins).
        } else if companion.is_some_and(|c| {
            let s = &squad.0[c.0].state;
            s.pinned
                && s.pinned_by
                    .is_none_or(|by| by == crate::special_infected::SpecialClass::Hunter)
        }) || companion.is_some() && p.activity.starts_with("hunter_")
        {
            let pinned = companion.is_some_and(|c| squad.0[c.0].state.pinned);
            let duration = sk
                .sequence_anim(p.sequence)
                .map_or(0., |i| sk.anims()[i].duration());
            if pinned {
                if !p.activity.starts_with("hunter_victim") {
                    p.clock = 0.;
                    p.activity = "hunter_victim_fall".into();
                } else if p.activity == "hunter_victim_fall" && p.clock >= duration {
                    p.clock = 0.;
                    p.activity = "hunter_victim_idle".into();
                }
                choose(
                    sk,
                    if p.activity == "hunter_victim_idle" {
                        &["ACT_IDLE_POUNCED", "Idle_Incap_Pounced"]
                    } else {
                        &["ACT_TERROR_INCAP_FROM_POUNCE", "Collapse_to_Incap_Pounced"]
                    },
                )
                .unwrap_or(0)
            } else if p.activity.starts_with("hunter_victim") {
                p.clock = 0.;
                p.activity = "hunter_getup".into();
                choose(sk, &["ACT_TERROR_POUNCED_TO_STAND", "GetUpFrom_Pounced"]).unwrap_or(0)
            } else if p.clock < duration {
                p.sequence
            } else {
                p.activity.clear();
                choose(sk, &["ACT_IDLE", "Idle_Standing_pistol"]).unwrap_or(0)
            }
        // Smoker tongue victim: native drag/fall/choke/hang activities.
        } else if companion.is_some_and(|c| {
            let s = &squad.0[c.0].state;
            s.pinned && s.pinned_by == Some(crate::special_infected::SpecialClass::Smoker)
        }) || companion.is_some() && p.activity.starts_with("smoker_victim")
        {
            if companion.is_some_and(|c| squad.0[c.0].state.pinned) {
                let act = crate::special_infected::smoker::victim_pose().activity();
                if p.activity.strip_prefix("smoker_victim/") != Some(act) {
                    p.clock = 0.;
                    p.activity = format!("smoker_victim/{act}");
                }
                choose(
                    sk,
                    &[
                        act,
                        "ACT_TERROR_CHOKING_TONGUE_GROUND",
                        "ACT_TERROR_DRAGGING_FROM_TONGUE",
                    ],
                )
                .unwrap_or(p.sequence)
            } else {
                p.activity.clear();
                p.clock = 0.;
                choose(sk, &["ACT_IDLE", "Idle_Standing_pistol"]).unwrap_or(0)
            }
        // Tank punch fling / rock knockdown: native fly, land and getup.
        } else if let Some((key, names)) = companion.and_then(|c| {
            crate::special_infected::tank::victim_activity(
                &squad.0[c.0],
                &p.activity,
                p.clock,
                sk.sequence_anim(p.sequence)
                    .map_or(0., |i| sk.anims()[i].duration()),
            )
        }) {
            if p.activity != key {
                p.clock = 0.;
                p.activity = key.into();
            }
            choose(sk, names).unwrap_or(p.sequence)
        // Survivor stagger (e.g. Boomer blast): native ACT_TERROR_SHOVED_<dir>.
        } else if let Some(act) = companion.and_then(|c| {
            let m = &squad.0[c.0];
            (m.state.stagger > 0. && !m.state.dead && !m.state.incap).then(|| {
                crate::special_infected::boomer::stagger_activity(m.aim, m.state.stagger_dir)
            })
        }) {
            if p.activity != act {
                p.clock = 0.;
                p.activity = act.into();
            }
            choose(sk, &[act, "ACT_TERROR_SHOVED_BACKWARD"]).unwrap_or(p.sequence)
        } else if let Some(c) = companion {
            let player_state = &*state;
            let state = &squad.0[c.0].state;
            let base = survivor_sequence
                .or_else(|| choose(sk, if moving { &run_candidates } else { &idle_candidates }))
                .unwrap_or(0);
            p.locomotion_sequence = base;
            p.locomotion_clock += dt;
            layer_gestures = true;
            // Gesture slots (server 1033cb40), each on its own clock over the
            // uninterrupted locomotion: shove/reload/fire restart slot 0.
            let reload_event = state.reload > 0.
                && (!p.reload
                    || p.reload_phase != state.reload_phase
                    || state.reload > p.reload_remaining + 0.001);
            let attack = if state.shove_serial != p.shove_serial {
                Some("shove")
            } else if reload_event {
                Some("reload")
            } else if state.shot_serial != p.shot_serial {
                Some("shoot")
            } else {
                None
            };
            if let Some(seq) = attack.and_then(|a| companion_weapon_action(sk, state, a, moving)) {
                p.gestures[GESTURE_ATTACK].restart(seq);
            }
            // Item use and reviving also go to slot 0 over the locomotion.
            let me = &squad.0[c.0];
            let player = targets.iter().next().map(|b| b.feet);
            let reviving = !me.state.incap
                && !me.state.dead
                && ((player_state.incap
                    && !player_state.infected
                    && me.revive_time > 0.
                    && player.is_some_and(|f| f.distance(me.position) < 2.))
                    // Only the one reviver kneels (retail m_reviveOwner).
                    || squad.0.iter().enumerate().any(|(i, o)| {
                        i != c.0
                            && o.state.incap
                            && o.revive_time > 0.
                            && survivor_reviver(i, &squad, player_state, player) == Some(me.position)
                    }));
            let crouched = survivor_motion
                .as_ref()
                .is_some_and(|(_, _, names)| names[0].contains("CROUCH"));
            let wanted = if state.action > 0. {
                use_gesture(state.medical_kind, crouched)
            } else if reviving {
                Some(use_gesture_activity(USE_REVIVE, crouched))
            } else {
                None
            };
            let remaining = if state.action > 0. { state.action } else { f32::INFINITY };
            match (wanted, p.medical) {
                (Some((act, cut)), current)
                    if current.is_none_or(|(a, _, left)| a != act || remaining > left + 0.001) =>
                {
                    let seed = entity.to_bits().wrapping_add((time.elapsed_secs() * 1000.) as u64);
                    let standing = use_gesture_activity(act, false).0;
                    if let Some(seq) = weighted_activity(sk, act, seed)
                        .or_else(|| weighted_activity(sk, standing, seed))
                    {
                        p.gestures[GESTURE_ATTACK].restart(seq);
                    }
                    p.medical = Some((act, cut, remaining));
                }
                (Some(_), Some((a, cut, _))) => p.medical = Some((a, cut, remaining)),
                (None, Some((_, cut, _))) => {
                    // Events 0x2e/0x36/0x38 reset slot 0 (no fade out).
                    if cut {
                        p.gestures[GESTURE_ATTACK] = Gesture::default();
                    }
                    p.medical = None;
                }
                _ => {}
            }
            // ACT_TERROR_FLINCH (Flinch_01-03) in slot 4, independent of slot 0.
            let flinch = p.gestures[GESTURE_FLINCH];
            if state.damage_serial != p.damage_serial
                && p.initialized
                && !state.incap
                && !state.pinned
                && !state.dead
                && (!flinch.active || flinch.cycle >= FLINCH_RESTART_CYCLE)
            {
                let seed = entity
                    .to_bits()
                    .wrapping_add(state.damage_serial.wrapping_mul(7919));
                if let Some(seq) = weighted_activity(sk, "ACT_TERROR_FLINCH", seed) {
                    p.gestures[GESTURE_FLINCH].restart(seq);
                }
            }
            // Calm fidget in slot 4 (ACT_TERROR_FIDGET).
            let busy = state.shove_serial != p.shove_serial
                || state.shot_serial != p.shot_serial
                || state.damage_serial != p.damage_serial
                || state.reload > 0.;
            p.calm = if busy { 0. } else { p.calm + dt };
            p.fidget_clock += dt;
            let fidget_seed = entity.to_bits().wrapping_add(p.fidgets.wrapping_mul(104_729));
            if p.fidget_at <= 0. {
                p.fidget_at = p.fidget_clock + fidget_interval(fidget_seed);
            }
            if p.fidget_clock >= p.fidget_at {
                p.fidgets += 1;
                p.fidget_at = p.fidget_clock + fidget_interval(fidget_seed.wrapping_add(1));
                if p.calm >= FIDGET_CALM
                    && !moving
                    && !state.incap
                    && !state.pinned
                    && !state.dead
                    && state.action <= 0.
                    && !p.gestures[GESTURE_ATTACK].active
                    && !p.gestures[GESTURE_FLINCH].active
                {
                    if let Some(seq) = weighted_activity(sk, "ACT_TERROR_FIDGET", fidget_seed) {
                        p.gestures[GESTURE_FLINCH].restart(seq);
                    }
                }
            }
            // HandleJumping's ACT_TERROR_JUMP_LANDING goes to slot 2.
            if std::mem::take(&mut p.survivor.landed) {
                if let Some(seq) = weighted_activity(sk, "ACT_TERROR_JUMP_LANDING", entity.to_bits()) {
                    p.gestures[GESTURE_JUMP].restart(seq);
                }
            }
            p.shove_serial = state.shove_serial;
            p.shot_serial = state.shot_serial;
            p.damage_serial = state.damage_serial;
            p.reload = state.reload > 0.;
            p.reload_phase = state.reload_phase;
            p.reload_remaining = state.reload;
            p.activity.clear();
            base
        } else {
            let e = enemy.unwrap();
            // Common hit flinch: AddGesture(ACT_TERROR_FLINCH) on damage
            // (server 10416750 / 1041fed0) over whatever the common is doing.
            // Shoves, door bashes and climbs bump the same serial but are
            // full-body reactions of their own.
            if path.contains("/common_") && e.health > 0. && witches.get(entity).is_err() {
                if e.reaction_serial != p.damage_serial
                    && p.initialized
                    && !matches!(e.reaction, 1 | 3 | 4..=10)
                {
                    let slot = p.gestures[GESTURE_FLINCH];
                    if !slot.active || slot.cycle >= FLINCH_RESTART_CYCLE {
                        let seed = entity.to_bits().wrapping_add(e.reaction_serial.wrapping_mul(7919));
                        if let Some(seq) = weighted_activity(sk, "ACT_TERROR_FLINCH", seed) {
                            p.gestures[GESTURE_FLINCH].restart(seq);
                        }
                    }
                }
                p.damage_serial = e.reaction_serial;
                layer_gestures = true;
            }
            let climb_activity = crate::infected_movement::climb_activity(e.reaction);
            let activity = if e.health <= 0. {
                crate::infected_death::activity(e.death)
            } else if let Ok(witch) = witches.get(entity) {
                // Witch behaviour activity (special_infected/witch.rs).
                witch.0
            } else if let Ok((special, flinch)) = ai.get(entity) {
                // Hit flinch in gesture slot 4 (server SetGesture flinch
                // slot), over whatever the special is doing.
                if let Some(flinch) = flinch {
                    let slot = p.gestures[GESTURE_FLINCH];
                    if flinch.1 != p.damage_serial
                        && p.initialized
                        && (!slot.active || slot.cycle >= FLINCH_RESTART_CYCLE)
                    {
                        let seed = entity.to_bits().wrapping_add(flinch.1.wrapping_mul(7919));
                        if let Some(seq) = weighted_activity(sk, flinch.0, seed) {
                            p.gestures[GESTURE_FLINCH].restart(seq);
                        }
                    }
                    p.damage_serial = flinch.1;
                }
                // AI special infected brain (special_infected/ai.rs); a punch
                // restarts gesture slot 0 once per swing.
                layer_gestures = true;
                if let Some(gesture) = special.1 {
                    if e.attack_serial != p.gesture_serial {
                        let seed = entity.to_bits().wrapping_add(e.attack_serial.wrapping_mul(7919));
                        if let Some(seq) = weighted_activity(sk, gesture, seed) {
                            p.gestures[GESTURE_ATTACK].restart(seq);
                        }
                    }
                }
                p.gesture_serial = e.attack_serial;
                special.0
            } else if e.reaction == crate::infected_movement::LADDER_UP && e.attack > 0. {
                "ACT_CLIMB_UP"
            } else if e.reaction == crate::infected_movement::LADDER_DOWN && e.attack > 0. {
                "ACT_CLIMB_DOWN"
            } else if (4..=10).contains(&e.reaction) && e.attack > 0. {
                climb_activity.as_str()
            } else if e.reaction == 1 && e.attack > 0. {
                "ACT_TERROR_SHOVED_BACKWARD"
            } else if e.reaction == 3 && e.attack > 0. {
                "ACT_TERROR_ATTACK_DOOR_CONTINUOUSLY"
            } else if e.reaction == 2 && e.attack > 0. {
                "ACT_TERROR_FLINCH"
            } else if e.burning > 0. {
                "ACT_TERROR_RUN_ON_FIRE"
            } else if let Some(mind) = minds.get(entity).ok().filter(|m| !m.attacking()) {
                // Unalerted wanderer (game::mind): posture idles, the
                // shamble, getting up, alert and acquire.
                mind.activity
            } else if e.attack > 0. {
                "ACT_TERROR_ATTACK_CONTINUOUSLY"
            } else if moving {
                if crate::infected_population::Kind::model(&e.model)
                    == crate::infected_population::Kind::Mud
                {
                    "ACT_TERROR_CRAWL_RUN"
                } else {
                    "ACT_TERROR_RUN_INTENSE"
                }
            } else {
                "ACT_TERROR_IDLE_ALERT"
            };
            let event = if matches!(e.reaction, 1 | 2 | 3) {
                e.reaction_serial != p.reaction_serial
            } else if activity == "ACT_TERROR_ATTACK_CONTINUOUSLY" {
                // Retail's looping melee clip plays on through every blow.
                false
            } else {
                false
            };
            p.reaction_serial = e.reaction_serial;
            p.attack_serial = e.attack_serial;
            if p.initialized && p.activity == activity && !event {
                p.sequence
            } else {
                p.activity = activity.into();
                p.clock = 0.;
                let seed = entity
                    .to_bits()
                    .wrapping_add(e.reaction_serial.wrapping_mul(7919));
                if e.health <= 0. {
                    crate::infected_death::select(sk, e.death, seed)
                } else {
                    // AI brains may name an exact sequence (the Tank's throw).
                    weighted_activity(sk, activity, seed).or_else(|| sk.find_sequence(activity))
                }
                .unwrap_or_else(|| choose(sk, &["ACT_IDLE", "Idle"]).unwrap_or(0))
            }
        };
        if let Some((label, _)) = sample {
            sequence = choose(sk, &[label]).unwrap_or(sequence);
        }
        if sk.sequences().is_empty() {
            continue;
        }
        sequence = sequence.min(sk.sequences().len() - 1);
        if sequence != p.sequence {
            p.clock = 0.;
        }
        if sequence != old_sequence || p.clock == 0. && p.initialized {
            p.transition_pose = p.last_pose.clone();
            p.transition_time = 0.;
            p.transition_duration = sk.sequences()[sequence]
                .fade_in
                .max(sk.sequences()[old_sequence].fade_out)
                .max(0.);
        }
        p.sequence = sequence;
        let before = p.clock;
        let authored_speed = sk.sequence_ground_speed(sequence) * 0.0254;
        let wandering = minds.get(entity).is_ok_and(|m| !m.attacking());
        let playback_rate = if common && wandering {
            // A wanderer's clip moves the body (enemy_ai's Intent::Clip), so
            // it plays at rate 1 as in retail.
            1.
        } else if common {
            common_playback_rate(
                speed,
                sk.sequence_ground_speed_params(sequence, &parameters) * crate::collision::UNIT,
                enemy.is_some_and(|e| e.vertical == 0.),
            )
        } else if enemy.is_some_and(|e| e.health > 0.)
            && moving
            && p.activity.contains("RUN")
            && authored_speed > 0.1
        {
            (speed / authored_speed).clamp(0.25, 2.5)
        } else if companion.is_some_and(|c| squad.0[c.0].state.ladder_pose.is_some()) {
            // Retail survivor cycle rate on a ladder (client 102822d0):
            // |vertical speed| * 0.01, so a stopped climber holds its pose.
            p.motion_velocity.y.abs() / 0.0254 * 0.01
        } else {
            1.
        };
        p.clock += dt * playback_rate;
        // `L4D2R_COMMON_LOG=1`: the retail parity probe's line
        // (l4d2r_common_anim_1112.nut) for every common, ten times a second.
        let dying_common = enemy.is_some_and(|e| e.health <= 0.) && path.contains("/common_");
        if (common || dying_common) && std::env::var_os("L4D2R_COMMON_LOG").is_some() {
            let tick = (state.elapsed * 10.).floor();
            if tick != ((state.elapsed - dt) * 10.).floor() {
                let s = &sk.sequences()[sequence];
                let duration = sk.sequence_anim(sequence).map_or(1., |i| sk.anims()[i].duration()).max(1e-3);
                let unit = crate::collision::UNIT;
                let pos = transform.translation / unit;
                let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
                println!(
                    "L4D2RCOMMON t={:.2} ent={} name={} act={} cyc={:.3} rate={:.3} pos={:.0},{:.0},{:.0} yaw={:.0} spd={:.0} hp={:.0}",
                    state.elapsed, entity.index(), s.label, s.activity_name, (p.clock / duration).fract(), playback_rate,
                    pos.x, pos.y, pos.z, yaw.to_degrees(), speed / unit, enemy.map_or(0., |e| e.health)
                );
                if dying_common {
                    println!("L4D2RDEATH t={:.2} ent={} ragdoll={} low={:?} death={:?}",
                        state.elapsed, entity.index(), p.ragdoll.is_some(), p.death_low, enemy.map(|e| e.death));
                }
            }
        }
        if weapon.is_some() && state.reload > 0. && is_reload_sequence(sk, sequence) {
            if let Some(i) = sk.sequence_anim(sequence) {
                let authored = sk.anims()[i].duration();
                let gun = rules
                    .weapons
                    .get(&state.gun().class)
                    .unwrap_or(&rules.pistol);
                let phase = state.reload_phase.saturating_sub(1).min(2) as usize;
                let total = if state.reload_total > 0. {
                    state.reload_total
                } else {
                    gun.reload_times[phase]
                }
                .max(0.001);
                let overstep = fixed
                    .as_ref()
                    .map_or(0., |t| t.timestep().as_secs_f32() * t.overstep_fraction());
                p.clock = authored * (1. - (state.reload - overstep) / total).clamp(0., 1.);
            }
        }
        if let Some(e) = enemy.filter(|e| e.health > 0. && (4..=10).contains(&e.reaction)) {
            let authored = sk
                .sequence_anim(sequence)
                .map_or(0., |i| sk.anims()[i].duration());
            p.clock = authored
                * (1. - e.attack / crate::infected_movement::climb_duration(e.reaction))
                    .clamp(0., 1.);
        }
        if let Some((_, time)) = sample {
            p.clock = *time;
            p.transition_pose.clear();
        }
        if enemy.is_some() {
            p.locomotion_clock += dt * playback_rate;
            if !p.activity.contains("FLINCH") {
                p.locomotion_sequence = sequence;
            }
        }
        if p.clock <= dt * playback_rate + 0.0001 {
            p.audit_clock = 0.;
        }
        if weapon.is_some() || enemy.is_some() {
            if let Some(queue) = sounds.as_mut() {
                let s = &sk.sequences()[sequence];
                let duration = sk
                    .sequence_anim(sequence)
                    .map_or(1., |i| sk.anims()[i].duration().max(0.01));
                let mut sequences = vec![sequence];
                if let Some(layer) = sk.find_sequence(&format!("{}_layer", s.label)) {
                    sequences.push(layer);
                }
                for i in sequences {
                    for event in &sk.sequences()[i].events {
                        let at = event.cycle * duration;
                        let first = if s.looping() {
                            before.rem_euclid(duration)
                        } else {
                            before
                        };
                        let last = if s.looping() {
                            p.clock.rem_euclid(duration)
                        } else {
                            p.clock
                        };
                        let crossed = if last < first {
                            at >= first || at < last
                        } else {
                            first <= at && last > at
                        };
                        if event.event == 5004 && crossed {
                            queue.play(
                                event.options.clone(),
                                if enemy.is_some() {
                                    Some(transform.translation)
                                } else {
                                    None
                                },
                            );
                        }
                    }
                }
            }
        }
        // A common's blows land on its melee clip's AE_ATTACK_HIT events.
        if let (Some(hits), true) = (melee_hits.as_mut(), common && enemy.is_some_and(|e| e.health > 0.)) {
            if p.activity == "ACT_TERROR_ATTACK_CONTINUOUSLY" && p.clock > before {
                let s = &sk.sequences()[sequence];
                let duration = sk.sequence_anim(sequence).map_or(1., |i| sk.anims()[i].duration().max(0.01));
                let (first, last) = if s.looping() {
                    (before.rem_euclid(duration), p.clock.rem_euclid(duration))
                } else {
                    (before, p.clock)
                };
                let crossed = s
                    .events
                    .iter()
                    .filter(|e| e.options.eq_ignore_ascii_case("AE_ATTACK_HIT"))
                    .filter(|e| {
                        let at = e.cycle * duration;
                        if last < first { at >= first || at < last } else { first <= at && last > at }
                    })
                    .count() as u32;
                if crossed > 0 {
                    *hits.hits.entry(entity).or_default() += crossed;
                }
            }
        }
        p.position = transform.translation;
        p.initialized = true;
        let continuous = enemy.is_some_and(|e| e.health > 0. && matches!(e.reaction, 0 | 3))
            || companion.is_some()
                && !survivor_dead
                && !matches!(
                    p.activity.as_str(),
                    "hunter_victim_fall"
                        | "hunter_getup"
                        | "tank_hit"
                        | "tank_land"
                        | "tank_rock"
                        // One-shot last-stand clips: wrapping showed frame 0
                        // (standing) for a moment as the collapse ended.
                        | "incap/collapse"
                        | "incap/getup"
                        | "incap/die"
                )
            || weapon.is_some() && state.slot == 4 && state.action > 0.;
        let looping = !dead && (sk.sequences()[sequence].looping() || continuous);
        let gestures: Vec<(usize, f32, bool, f32)> = if layer_gestures {
            p.gestures
                .iter_mut()
                .filter_map(|g| g.advance(sk, dt))
                .map(|(seq, time, weight)| (seq, time, false, weight))
                .collect()
        } else {
            p.gestures = Default::default();
            vec![]
        };
        let mut pose = if !gestures.is_empty() {
            sk.pose_weighted_layers_params(sequence, p.clock, looping, &gestures, &parameters)
        } else if weapon.is_some() {
            let activity = &sk.sequences()[sequence].activity_name;
            if activity.starts_with("ACT_TERROR_HEAL") || activity == "ACT_TERROR_USE_DEFIBRILLATOR" {
                sk.pose_retargeted_view_sequence(sequence, p.clock, looping)
            } else {
                let body = targets.iter().next();
                // The player's current run speed (client 102880d0 reads
                // GetRunMaxSpeed): adrenaline 260, limping (under 40 health)
                // the walk speed, otherwise 220.
                let run = if state.adrenaline_time > 0. {
                    260.
                } else if state.health + state.temporary < 40. {
                    WALK_SPEED
                } else {
                    RUN_SPEED
                };
                let (move_x, ver_aims) = body.map_or((0., 0.), |b| {
                    view_pose_targets(b.velocity, b.yaw, b.pitch, b.crouched, run)
                });
                let values = [("move_x", p.view_move_x), ("ver_aims", p.view_ver_aims)];
                // Retail eases toward the targets each frame, then the idle
                // advances at its blended cycle rate.
                p.view_move_x = approach_move_x(p.view_move_x, move_x, dt);
                p.view_ver_aims = approach_linear(p.view_ver_aims, ver_aims, 10. * dt);
                let idle = choose(sk, &["ACT_VM_IDLE"]);
                if let Some(idle) = idle {
                    p.view_idle_cycle = (p.view_idle_cycle + dt * sk.sequence_cycle_rate_params(idle, &values)).rem_euclid(1.);
                }
                sk.pose_view_sequence_params(sequence, p.clock, looping, p.view_idle_cycle, &values)
            }
        } else if enemy.is_some_and(|e| e.health > 0. && e.reaction == 2)
            && (sk.sequences()[sequence].is_delta()
                || sk.sequences()[sequence]
                    .weight_list
                    .iter()
                    .any(|w| *w < 0.99))
        {
            let base = if p.locomotion_sequence == sequence {
                weighted_activity(sk, "ACT_TERROR_IDLE_ALERT", entity.to_bits()).unwrap_or(0)
            } else {
                p.locomotion_sequence
            };
            sk.pose_gesture_params(base, p.locomotion_clock, sequence, p.clock, &parameters)
        } else if companion.is_some() && p.activity == "hunter_victim_idle" {
            let base = choose(sk, &["Collapse_to_Incap_Pounced"]).unwrap_or(sequence);
            let duration = sk
                .sequence_anim(base)
                .map_or(0., |i| sk.anims()[i].duration());
            sk.pose_layers_params(
                base,
                duration,
                false,
                &[(sequence, p.clock, true)],
                &[("body_pitch", 0.), ("body_yaw", 0.)],
            )
        } else {
            sk.pose_sequence_params(sequence, p.clock, looping, &parameters)
        };
        // Retail renders a survivor at its feet yaw; the actor root faces the
        // aim (eye) yaw, so turn the model-space pose (Source z up) by the
        // difference before transitions and IK.
        let mut feet_turn = None;
        if survivor_motion.is_some() {
            let delta = angle_normalize(p.survivor.feet_yaw - source_yaw(transform.rotation * Vec3::X));
            if delta.abs() > 1e-3 {
                let turn = XMat::from_rotation_z(delta.to_radians());
                for bone in pose.iter_mut() {
                    *bone = turn * *bone;
                }
                feet_turn = Some(turn);
            }
        }
        let fade = p.transition_duration;
        if p.transition_pose.len() == pose.len() && p.transition_time < fade {
            p.transition_time += dt;
            blend_local_pose(
                sk,
                &p.transition_pose,
                &mut pose,
                (p.transition_time / fade).clamp(0., 1.),
            );
        }
        // Commons and the Witch are C_NextBotCombatCharacter, whose
        // constructor (client 102d5560) sets ENTCLIENTFLAG_DONTUSEIK: retail
        // runs no IK on them (C_BaseAnimating::SetupBones 1003c380). AI
        // specials are players and keep it.
        let nextbot = enemy.is_some() && !ai.contains(entity);
        if !dead && !nextbot && !p.activity.starts_with("hunter_victim") {
            let mut ik_rules = if weapon.is_some() {
                sk.ik_view_rules(sequence, p.clock, looping)
            } else {
                sk.ik_rules(sequence, p.clock, looping, &parameters)
            };
            for &(seq, time, _, _) in &gestures {
                ik_rules.append(&mut sk.ik_rules(seq, time, false, &parameters));
            }
            // Model-space IK targets (IK_SELF rules without a reference bone,
            // e.g. the survivor foot locks) are authored in the frame retail
            // renders at the feet yaw: turn them with the pose, or they pull
            // the feet back toward the unturned stance and bend the legs.
            if let Some(turn) = feet_turn {
                for rule in ik_rules.iter_mut().filter(|r| r.bone.is_none() && r.kind == 1) {
                    rule.target = turn * rule.target;
                }
            }
            p.ik.apply(
                sk,
                &mut pose,
                &ik_rules,
                transform,
                &collision,
                weapon.is_none(),
            );
        }
        if dead {
            let duration = sk
                .sequence_anim(sequence)
                .map_or(0., |i| sk.anims()[i].duration());
            // Retail commons with no death animation (blasts, melee, cut
            // limbs) go limp from the pose they were in, not a ragdoll clip.
            if p.death_low.is_none() {
                let low = enemy.is_some() && path.contains("/common_")
                    && sk.bone_index("ValveBiped.Bip01_Pelvis").is_some_and(|i| {
                        let bind = sk.bones[i].pose_to_bone.inverse().transform_point3(XVec::ZERO);
                        p.last_pose.get(i).is_some_and(|m| m.transform_point3(bind).z < 24.)
                    });
                p.death_low = Some(low);
            }
            let low = p.death_low == Some(true);
            let limp = low || enemy.is_some_and(|e| crate::infected_death::immediate_ragdoll(e.death));
            if limp && p.ragdoll.is_none() && p.last_pose.len() == pose.len() {
                pose.clone_from(&p.last_pose);
            }
            if survivor_dead && p.activity != "incap/die" || limp || p.clock >= duration.max(0.1) {
                let velocity = if let Some(e) = enemy {
                    let push = match e.death.cause {
                        crate::infected_death::Cause::Explosion => 6.,
                        crate::infected_death::Cause::Shotgun => 2.5,
                        crate::infected_death::Cause::Blunt
                        | crate::infected_death::Cause::Slash => 1.,
                        _ => 0.35,
                    };
                    transform.rotation * e.death.incoming * push + Vec3::Y * e.vertical.min(0.)
                } else {
                    // No invented launch impulse for a dead survivor. Preserve
                    // their rendered pin/standing pose and let gravity settle it.
                    Vec3::ZERO
                };
                if survivor_dead && p.activity != "incap/die" && p.last_pose.len() == pose.len() {
                    pose.clone_from(&p.last_pose);
                }
                let body = p.ragdoll.get_or_insert_with(|| {
                    let body = crate::ragdoll::Body::new(sk, &pose, transform, velocity);
                    if let Some(profile) = &model.physics {
                        body.with_profile(sk, profile, &library.surfaces)
                    } else {
                        body
                    }
                });
                pose = body.pose(transform);
            }
        } else {
            p.ragdoll = None;
            p.death_low = None;
        }
        p.last_pose = pose.clone();
        poses.insert(
            entity,
            sk.bones
                .iter()
                .zip(pose)
                .map(|(b, mat)| (b.name.to_ascii_lowercase(), mat))
                .collect(),
        );
    }
    // Include freshly initialized bodies in this frame's parallel batch.
    // Final poses are installed before any GPU joints/meshes are updated.
    let mut jobs = Vec::new();
    if dt > 0. {
        for (entity, p) in playback.0.iter_mut() {
            if roots.get(*entity).is_ok_and(|(_, _, c, e, _, _)| {
                c.is_some_and(|c| squad.0[c.0].state.dead)
                    || e.is_some_and(|e| e.health <= 0. && library.models.contains_key(&e.model))
            }) {
                if let Some(body) = p.ragdoll.take() {
                    jobs.push((*entity, body));
                }
            }
        }
    }
    tick_ragdolls(&mut jobs, &collision, dt, &mut workers);
    for (entity, body) in jobs {
        let (_, transform, companion, enemy, _, _) = roots.get(entity).unwrap();
        let path = companion.map_or_else(|| &enemy.unwrap().model, |c| &squad.0[c.0].model);
        let model = &library.models[path];
        let pose = body.pose(transform);
        let p = playback.0.get_mut(&entity).unwrap();
        p.last_pose = pose.clone();
        p.ragdoll = Some(body);
        poses.insert(
            entity,
            model
                .skeleton
                .bones
                .iter()
                .zip(pose)
                .map(|(b, m)| (b.name.to_ascii_lowercase(), m))
                .collect(),
        );
    }
    for (entity, transform, _, enemy, _, _) in &roots {
        if enemy.is_some_and(|e| e.model.contains("/common_")) {
            if let Some(p) = playback.0.get_mut(&entity) {
                p.record_gore_pose(transform, dt);
            }
        }
    }
    static AUDIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *AUDIT.get_or_init(|| std::env::args().any(|a| a == "--audit-animation-playback")) {
        use std::io::Write;
        for (entity, transform, companion, enemy, weapon, _) in &roots {
            let path = if let Some(c) = companion {
                &squad.0[c.0].model
            } else if let Some(e) = enemy {
                &e.model
            } else if let Some(w) = weapon {
                &rules.weapons.get(&w.0).unwrap_or(&rules.pistol).view_model
            } else {
                continue;
            };
            let Some(p) = playback.0.get_mut(&entity) else {
                continue;
            };
            if p.clock - p.audit_clock < 0.1 {
                continue;
            }
            p.audit_clock = p.clock;
            let Some(model) = library.models.get(path) else {
                continue;
            };
            let sk = &model.skeleton;
            let hash = p
                .last_pose
                .iter()
                .flat_map(|m| m.to_cols_array())
                .fold(0u64, |h, f| h.wrapping_mul(16777619) ^ f.to_bits() as u64);
            let survivor_dead = companion.is_some_and(|c| squad.0[c.0].state.dead);
            let looping = !survivor_dead
                && !enemy.is_some_and(|e| e.health <= 0.)
                && (sk.sequences()[p.sequence].looping()
                    || companion.is_some()
                    || enemy.is_some_and(|e| e.health > 0. && matches!(e.reaction, 0 | 3))
                    || weapon.is_some() && state.slot == 4 && state.action > 0.);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("local-research/animation-playback.tsv")
            {
                let _ = writeln!(
                    f,
                    "{entity:?}\t{path}\t{}\t{}\t{:.3}\t{}\t{hash}",
                    p.activity,
                    sk.sequences()[p.sequence].label,
                    p.clock,
                    looping
                );
            }
            if survivor_dead {
                if let Some(head) = sk
                    .bones
                    .iter()
                    .position(|b| b.name.eq_ignore_ascii_case("ValveBiped.Bip01_Head1"))
                {
                    let head = (transform.to_matrix() * render_bone(p.last_pose[head]))
                        .w_axis
                        .truncate();
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("local-research/parity/survivor-corpse-poses.tsv")
                    {
                        let _ = writeln!(
                            f,
                            "{entity:?}\t{}\t{:.3}\t{:?}\t{head:?}\t{}",
                            path,
                            p.clock,
                            transform.translation,
                            p.ragdoll.is_some()
                        );
                    }
                }
            }
        }
    }
    static FREEZE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let freeze = *FREEZE.get_or_init(|| std::env::args().any(|a| a == "--profile-freeze-skin"));
    static CPU_SKIN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let cpu_skin = *CPU_SKIN.get_or_init(|| std::env::args().any(|a| a == "--profile-cpu-skin"));
    for (entity, draw, mesh, visible, gun, gpu) in &draws {
        if freeze {
            continue;
        }
        // Keep clocks advancing, but avoid skinning and GPU mesh uploads for
        // actors outside every camera. Returning actors use the current pose.
        let dying = roots.get(draw.root).is_ok_and(|r| {
            r.2.is_some_and(|c| squad.0[c.0].state.dead) || r.3.is_some_and(|e| e.health <= 0.)
        });
        // GPU skinning too: writing ~100 joint transforms per hidden actor
        // each frame (c4m3's start: ~3,400) dominated transform propagation.
        if !visible.get() && !dying {
            continue;
        }
        if let (Some(model), Some(pose)) = (library.models.get(&draw.model), poses.get(&draw.root))
        {
            let source = &model.draws[draw.draw];
            if !cpu_skin {
                let merged = merged_bones(source, pose);
                if let Some(gpu) = gpu {
                    for (&joint, bone) in gpu.joints.iter().zip(merged) {
                        if let Ok(mut transform) = joints.get_mut(joint) {
                            transform.set_if_neq(Transform::from_matrix(render_bone(bone)));
                        }
                    }
                } else if meshes.contains(&mesh.0) {
                    // A shared common body is skinned once; later infected
                    // only add their own joints.
                    // Template of an already skinned shared body: use that.
                    let swap = SHARED_SKINS.lock().ok().and_then(|s| {
                        s.by_source.get(&mesh.0.id()).filter(|h| s.ready.contains(&h.id())).cloned()
                    });
                    let ready = swap.is_some()
                        || SHARED_SKINS.lock().is_ok_and(|s| s.ready.contains(&mesh.0.id()));
                    if let Some(handle) = swap {
                        commands.entity(entity).insert(Mesh3d(handle));
                    }
                    if !ready {
                    let mut mesh_asset = meshes.get_mut(&mesh.0).unwrap();
                    let mesh = &mut *mesh_asset;
                    let (indices, weights) = gpu_weights(source);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, source.mesh.positions.clone());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, source.mesh.normals.clone());
                    mesh.insert_attribute(
                        Mesh::ATTRIBUTE_JOINT_INDEX,
                        VertexAttributeValues::Uint16x4(indices),
                    );
                    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
                    let _ = mesh.generate_skinned_mesh_bounds();
                    }
                    if let Ok(mut skins) = SHARED_SKINS.lock() {
                        if skins.by_source.values().any(|h| *h == mesh.0) {
                            skins.ready.insert(mesh.0.id());
                        }
                    }
                    let mut nodes = Vec::with_capacity(merged.len() + 1);
                    for bone in merged.into_iter().chain(std::iter::once(XMat::IDENTITY)) {
                        nodes.push(
                            commands
                                .spawn((
                                    GpuBone,
                                    Transform::from_matrix(render_bone(bone)),
                                    ChildOf(entity),
                                ))
                                .id(),
                        );
                    }
                    let mut bind = source
                        .bones
                        .iter()
                        .map(|b| render_bone(b.inverse))
                        .collect::<Vec<_>>();
                    bind.push(Mat4::IDENTITY);
                    commands.entity(entity).insert((
                        SkinnedMesh {
                            joints: nodes.clone(),
                            inverse_bindposes: inverse_bindposes
                                .add(SkinnedMeshInverseBindposes::from(bind)),
                        },
                        GpuSkin { joints: nodes },
                        DynamicSkinnedMeshBounds,
                    ));
                }
                if !roots.get(draw.root).is_ok_and(|r| r.4.is_some()) {
                    commands.entity(entity).remove::<NoFrustumCulling>();
                }
                continue;
            }
            let Some(mut mesh) = meshes.get_mut(&mesh.0) else {
                continue;
            };
            let (positions, normals) = skin(&model.draws[draw.draw], pose);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            if let Some(bounds) = mesh.compute_aabb() {
                commands.entity(entity).insert(bounds);
            }
            let view = roots.get(draw.root).is_ok_and(|r| r.4.is_some());
            let dying = roots.get(draw.root).is_ok_and(|r| {
                r.2.is_some_and(|c| squad.0[c.0].state.dead) || r.3.is_some_and(|e| e.health <= 0.)
            });
            if dying {
                commands.entity(entity).insert(NoFrustumCulling);
            } else if !view && gun.is_none() {
                commands.entity(entity).remove::<NoFrustumCulling>();
            }
        }
    }
    playback.0.retain(|entity, _| poses.contains_key(entity));
}
#[cfg(test)]
mod viewmodel_motion_tests {
    use super::*;
    #[test]
    fn retail_viewmodel_walk_blend_targets() {
        let unit = crate::collision::UNIT;
        let forward = Vec3::NEG_Z;
        let at = |speed: f32, dir: Vec3| view_pose_targets(dir * speed * unit, 0., 0., false, RUN_SPEED).0;
        assert_eq!(at(0., forward), 0.);
        assert_eq!(at(60., forward), 0.);
        assert!((at(70., forward) - 70. / 85. * 0.5).abs() < 1e-4);
        assert!((at(85., forward) - 0.5).abs() < 1e-4);
        assert!((at(220., forward) - 1.).abs() < 1e-4);
        assert_eq!(at(260., forward), 1.);
        assert!((at(220., Vec3::Z) + 1.).abs() < 1e-4, "backwards is negative");
        assert!((at(220., Vec3::X) - 1.).abs() < 1e-4, "strafing is forward");
        // Standing: Source pitch / 90 (looking down is positive).
        let aims = |pitch: f32, crouched| view_pose_targets(Vec3::ZERO, 0., pitch.to_radians(), crouched, RUN_SPEED).1;
        assert!((aims(-45., false) - 0.5).abs() < 1e-4);
        assert!((aims(45., false) + 0.5).abs() < 1e-4);
        assert!((aims(45., true) + 0.5).abs() < 1e-4);
        assert!((aims(-45., true) + 0.5).abs() < 1e-4);
        // Eases at 1/s out, 2/s back.
        assert!((approach_move_x(0., 1., 0.1) - 0.1).abs() < 1e-6);
        assert!((approach_move_x(1., 0., 0.1) - 0.8).abs() < 1e-6);
    }
    #[test]
    fn running_rifle_idle_bobs_at_the_run_cycle() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/v_models/v_rifle.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let idle = sk.find_sequence("ACT_VM_IDLE").unwrap();
        let still = [("move_x", 0.), ("ver_aims", 0.)];
        let run = [("move_x", 1.), ("ver_aims", 0.)];
        // a_idle_1: 61 frames at 30 fps; a_run: 31 frames at 30 fps.
        assert!((sk.sequence_cycle_rate_params(idle, &still) - 0.5).abs() < 1e-3);
        assert!((sk.sequence_cycle_rate_params(idle, &run) - 1.).abs() < 1e-3);
        let moved = |a: &[XMat], b: &[XMat]| a.iter().zip(b).map(|(a, b)| a.w_axis.distance(b.w_axis)).fold(0., f32::max);
        let standing = sk.pose_view_sequence_params(idle, 0., true, 0.25, &still);
        let running = sk.pose_view_sequence_params(idle, 0., true, 0.25, &run);
        let run_offset = moved(&standing, &running);
        // Inches: a gentle bob of the hands and gun together.
        assert!(run_offset > 0.05 && run_offset < 3., "run blend moves the arms: {run_offset}");
        let up = sk.pose_view_sequence_params(idle, 0., true, 0.25, &[("move_x", 0.), ("ver_aims", 1.)]);
        assert!(moved(&standing, &up) > 0.01, "ver_aims layer moves the gun");
        // The standing pose matches the old (parameterless) viewmodel path.
        let old = sk.pose_view_sequence(idle, 0.5, true);
        assert!(moved(&old, &sk.pose_view_sequence_params(idle, 0.5, true, 0.25, &still)) < 1e-3);
        // Fire keeps the running idle beneath its layer.
        let fire = sk.find_sequence("ACT_VM_PRIMARYATTACK").unwrap();
        let fire_still = sk.pose_view_sequence_params(fire, 0.05, false, 0.25, &still);
        let fire_run = sk.pose_view_sequence_params(fire, 0.05, false, 0.25, &run);
        assert!(moved(&fire_still, &fire_run) > 0.05);
    }
}
