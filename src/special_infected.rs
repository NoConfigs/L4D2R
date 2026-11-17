//! Playable special infected. This file owns the shared infected-player
//! lifecycle (team switch, ghost spawn, death/respawn, survivor-bot
//! opposition) plus the Hunter ability. Each other class owns its ability in
//! `special_infected/<class>.rs`. Full Versus rounds remain pending.
pub mod ai;
pub mod boomer;
pub mod boomer_gibs;
pub mod charger;
pub mod death_effects;
pub mod jockey;
pub mod smoker;
pub mod spitter;
pub mod tank;
pub mod voice;
pub mod witch;
use crate::{
    collision::{Collision, HEIGHT, UNIT},
    game::{Body, CompanionActor},
    game_state::{Match, Rules, Squad},
};
#[cfg(test)]
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
pub const HUNTER_WEAPON: &str = "weapon_melee_infected_hunter";
pub const HUNTER_MODEL: &str = "models/infected/hunter.mdl";
pub const HUNTER_VIEW: &str = "models/v_models/weapons/v_claw_hunter.mdl";
pub const MARKER_LAYER: usize = 29;
/// Playable special-infected roles. The Witch is not playable; see `witch`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SpecialClass {
    #[default]
    Hunter,
    Smoker,
    Boomer,
    Spitter,
    Jockey,
    Charger,
    Tank,
}
impl SpecialClass {
    pub const MINIONS: [Self; 6] = [
        Self::Hunter,
        Self::Smoker,
        Self::Boomer,
        Self::Spitter,
        Self::Jockey,
        Self::Charger,
    ];
    pub fn from_model(path: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|role| role.model().eq_ignore_ascii_case(path))
    }
    pub const ALL: [Self; 7] = [
        Self::Hunter,
        Self::Smoker,
        Self::Boomer,
        Self::Spitter,
        Self::Jockey,
        Self::Charger,
        Self::Tank,
    ];
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|c| c.name() == name)
            .or((name == "hulk").then_some(Self::Tank))
    }
    /// Lower-case retail name, as used by z_spawn, HUD resources and tips.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hunter => "hunter",
            Self::Smoker => "smoker",
            Self::Boomer => "boomer",
            Self::Spitter => "spitter",
            Self::Jockey => "jockey",
            Self::Charger => "charger",
            Self::Tank => "tank",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Hunter => "HUNTER",
            Self::Smoker => "SMOKER",
            Self::Boomer => "BOOMER",
            Self::Spitter => "SPITTER",
            Self::Jockey => "JOCKEY",
            Self::Charger => "CHARGER",
            Self::Tank => "TANK",
        }
    }
    /// Classes whose ability is implemented; the console rejects the rest.
    pub fn playable(self) -> bool {
        match self {
            Self::Hunter => true,
            Self::Smoker => smoker::PLAYABLE,
            Self::Boomer => boomer::PLAYABLE,
            Self::Spitter => spitter::PLAYABLE,
            Self::Jockey => jockey::PLAYABLE,
            Self::Charger => charger::PLAYABLE,
            Self::Tank => tank::PLAYABLE,
        }
    }
    pub fn weapon(self) -> &'static str {
        match self {
            Self::Hunter => HUNTER_WEAPON,
            Self::Smoker => smoker::WEAPON,
            Self::Boomer => boomer::WEAPON,
            Self::Spitter => spitter::WEAPON,
            Self::Jockey => jockey::WEAPON,
            Self::Charger => charger::WEAPON,
            Self::Tank => tank::WEAPON,
        }
    }
    pub fn from_weapon(class: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.weapon() == class)
    }
    pub fn model(self) -> &'static str {
        match self {
            Self::Hunter => HUNTER_MODEL,
            Self::Smoker => smoker::MODEL,
            Self::Boomer => boomer::MODEL,
            Self::Spitter => spitter::MODEL,
            Self::Jockey => jockey::MODEL,
            Self::Charger => charger::MODEL,
            Self::Tank => tank::MODEL,
        }
    }
    pub fn view(self) -> &'static str {
        match self {
            Self::Hunter => HUNTER_VIEW,
            Self::Smoker => smoker::VIEW,
            Self::Boomer => boomer::VIEW,
            Self::Spitter => spitter::VIEW,
            Self::Jockey => jockey::VIEW,
            Self::Charger => charger::VIEW,
            Self::Tank => tank::VIEW,
        }
    }
    /// Retail z_<class>_health (z_hunter_health=250 measured).
    pub fn health(self) -> f32 {
        match self {
            Self::Hunter => 250.,
            Self::Smoker => smoker::HEALTH,
            Self::Boomer => boomer::HEALTH,
            Self::Spitter => spitter::HEALTH,
            Self::Jockey => jockey::HEALTH,
            Self::Charger => charger::HEALTH,
            Self::Tank => tank::HEALTH,
        }
    }
    /// Ground speed in Source units/sec (z_hunter_speed=300 measured).
    pub fn speed(self) -> f32 {
        match self {
            Self::Hunter => 300.,
            Self::Smoker => smoker::SPEED,
            Self::Boomer => boomer::SPEED,
            Self::Spitter => spitter::SPEED,
            Self::Jockey => jockey::SPEED,
            Self::Charger => charger::SPEED,
            Self::Tank => tank::SPEED,
        }
    }

    /// Footstep soundscript family. client.dll precaches Survivor, Infected,
    /// Boomer, Witch and Tank step sets; the Jockey has its own scripts.
    pub fn step_prefix(self) -> &'static str {
        match self {
            Self::Boomer => "Boomer",
            Self::Jockey => "Jockey",
            Self::Tank => "Tank",
            _ => "Infected",
        }
    }
    /// Ability-timer artwork under materials/vgui/hud/.
    pub fn ability_icon(self) -> &'static str {
        match self {
            Self::Hunter => "pz_charge_lunge",
            Self::Smoker => "pz_charge_smoker",
            Self::Boomer => "pz_charge_boomer",
            Self::Spitter => "pz_charge_spitter",
            Self::Jockey => "pz_charge_jockey",
            Self::Charger => "pz_charge_charger",
            Self::Tank => "pz_charge_tank",
        }
    }
    /// Whether special_phase 3 (an active pin/ride/pummel) uses the
    /// third-person orbit camera and hides the first-person claws.
    pub fn third_person_pin(self) -> bool {
        match self {
            Self::Hunter => true,
            Self::Smoker => smoker::THIRD_PERSON_PIN,
            Self::Boomer => false,
            Self::Spitter => false,
            Self::Jockey => jockey::THIRD_PERSON_PIN,
            Self::Charger => charger::THIRD_PERSON_PIN,
            Self::Tank => false,
        }
    }
    /// Viewmodel activity for a special_phase. `None` keeps the shared
    /// default (2 lunge, 3 pummel, 4 primary attack, otherwise idle).
    pub fn vm_activity(self, phase: u8) -> Option<&'static str> {
        match self {
            Self::Hunter => None,
            Self::Smoker => smoker::vm_activity(phase),
            Self::Boomer => boomer::vm_activity(phase),
            Self::Spitter => spitter::vm_activity(phase),
            Self::Jockey => jockey::vm_activity(phase),
            Self::Charger => charger::vm_activity(phase),
            Self::Tank => tank::vm_activity(phase),
        }
    }
    /// Extra (action, activity) pairs the viewmodel must provide.
    pub fn vm_actions(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Hunter => &[("pounce", "ACT_VM_LUNGE"), ("pin", "ACT_VM_PUMMEL_ATTACK")],
            Self::Smoker => smoker::VM_ACTIONS,
            Self::Boomer => boomer::VM_ACTIONS,
            Self::Spitter => spitter::VM_ACTIONS,
            Self::Jockey => jockey::VM_ACTIONS,
            Self::Charger => charger::VM_ACTIONS,
            Self::Tank => tank::VM_ACTIONS,
        }
    }
    pub fn controls(self) -> &'static str {
        match self {
            Self::Hunter => {
                "Hunter playtest: Ctrl charges pounce; left click pounces; right click claws."
            }
            Self::Smoker => smoker::CONTROLS,
            Self::Boomer => boomer::CONTROLS,
            Self::Spitter => spitter::CONTROLS,
            Self::Jockey => jockey::CONTROLS,
            Self::Charger => charger::CONTROLS,
            Self::Tank => tank::CONTROLS,
        }
    }
}
/// True while the given class is the alive, spawned local infected player.
pub fn active(state: &Match, hunter: &Hunter, class: SpecialClass) -> bool {
    state.infected && state.special_class == class && !state.dead && !hunter.ghost
}
pub(crate) fn pin_camera(
    feet: Vec3,
    yaw: f32,
    pitch: f32,
    collision: Option<&Collision>,
) -> Transform {
    let target = feet + Vec3::Y * 0.85;
    let orbit = Quat::from_euler(
        EulerRot::YXZ,
        yaw + std::f32::consts::PI,
        pitch.clamp(-0.7, 0.7),
        0.,
    );
    let desired = target - (orbit * Vec3::NEG_Z) * 3.5 + Vec3::Y * 1.2;
    let fraction = collision.map_or(1., |c| c.trace(target, desired, Vec3::splat(0.12)).fraction);
    let position = target.lerp(desired, (fraction - 0.015).max(0.));
    Transform::from_translation(position).looking_at(target, Vec3::Y)
}
/// Pinned survivor's view: behind the survivor along the view yaw, looking
/// down at them (and past them at the attacker). The mouse orbits it.
pub(crate) fn victim_camera(
    feet: Vec3,
    yaw: f32,
    pitch: f32,
    collision: Option<&Collision>,
) -> Transform {
    let target = feet + Vec3::Y * 0.6;
    let place = |yaw: f32| {
        let orbit = Quat::from_euler(EulerRot::YXZ, yaw, pitch.clamp(-0.9, 0.6), 0.);
        let desired = target + orbit * Vec3::Z * 3. + Vec3::Y * 1.1;
        let fraction = collision.map_or(1., |c| c.trace(target, desired, Vec3::splat(0.12)).fraction);
        let position = target.lerp(desired, (fraction - 0.015).max(0.));
        (position, position.distance(target))
    };
    // Pins usually end against a wall (a Charger's charge always does), so
    // the spot behind the survivor is often inside it and the view sat in
    // their own body. Swing round to the nearest open side instead.
    const CLEAR: f32 = 1.6;
    let (mut position, mut reach) = place(yaw);
    if reach < CLEAR {
        'search: for step in 1..=6 {
            for sign in [1., -1.] {
                let (p, r) = place(yaw + sign * step as f32 * std::f32::consts::PI / 6.);
                if r > reach {
                    (position, reach) = (p, r);
                }
                if r >= CLEAR {
                    break 'search;
                }
            }
        }
    }
    Transform::from_translation(position).looking_at(target, Vec3::Y)
}
/// Over-the-shoulder view behind the infected player, looking along the view
/// yaw/pitch (retail charger-retail-charge-view.jpg; Smoker while reeling).
pub(crate) fn chase_camera(
    feet: Vec3,
    yaw: f32,
    pitch: f32,
    collision: Option<&Collision>,
) -> Transform {
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch.clamp(-1.2, 1.2), 0.);
    let pivot = feet + Vec3::Y * 1.5;
    let desired = pivot + rotation * Vec3::Z * 1.7 + Vec3::Y * 0.3;
    let fraction = collision.map_or(1., |c| c.trace(pivot, desired, Vec3::splat(0.12)).fraction);
    let position = pivot.lerp(desired, (fraction - 0.015).max(0.));
    Transform::from_translation(position).with_rotation(rotation)
}
/// Third-person view for the current pin: Smoker reeling and Charger
/// charging chase behind the player; other pins orbit the pinned pair.
pub(crate) fn third_person_camera(
    class: SpecialClass,
    chase: bool,
    feet: Vec3,
    yaw: f32,
    pitch: f32,
    collision: Option<&Collision>,
) -> Transform {
    if class == SpecialClass::Smoker || (class == SpecialClass::Charger && chase) {
        chase_camera(feet, yaw, pitch, collision)
    } else {
        pin_camera(feet, yaw, pitch, collision)
    }
}
#[derive(Component)]
struct PinDraw(usize);
pub(crate) fn pin_rotation(yaw: f32, attacker: bool) -> Quat {
    // Native pounced survivor's head is along -X; the Hunter must face it.
    Quat::from_rotation_y(
        yaw + if attacker {
            -std::f32::consts::FRAC_PI_2
        } else {
            std::f32::consts::FRAC_PI_2
        },
    )
}
#[derive(Component)]
struct InfectedFootLight;
fn pin_light_intensity(state: &Match, hunter: &Hunter, playing: bool) -> f32 {
    if state.infected
        && !state.dead
        && !hunter.ghost
        && hunter.pin.is_some()
        && state.special_phase == 3
        && playing
    {
        40.
    } else {
        0.
    }
}
fn infected_light(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    frontend: Res<crate::game_menu::Frontend>,
    body: Single<&Body>,
    mut lights: Query<(&mut PointLight, &mut Transform), With<InfectedFootLight>>,
) {
    let intensity = pin_light_intensity(&state, &hunter, frontend.playing());
    let position = body.feet + Vec3::Y * 0.6;
    if lights.is_empty() {
        commands.spawn((
            InfectedFootLight,
            PointLight {
                color: Color::srgb(1., 0.025, 0.),
                intensity,
                range: 3.,
                radius: 0.1,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(position),
            bevy::camera::visibility::RenderLayers::layer(1),
        ));
    }
    for (mut light, mut transform) in &mut lights {
        if light.intensity != intensity {
            light.intensity = intensity;
        }
        if transform.translation != position {
            transform.translation = position;
        }
    }
}
fn advance_pin_attack(attack: &mut (u64, Option<f32>), serial: u64, dt: f32) {
    if attack.0 != serial {
        attack.0 = serial;
        // Damage ticks do not restart an already-running native pummel cycle.
        if attack.1.is_none() {
            attack.1 = Some(0.);
        }
    }
    if let Some(clock) = &mut attack.1 {
        *clock += dt;
    }
}
fn pin_model(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    body: Single<&Body>,
    models: Res<crate::viewer::RenderModels>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    mut meshes: ResMut<Assets<Mesh>>,
    time: Res<Time>,
    mut draws: Query<(Entity, &PinDraw, &Mesh3d, &mut Transform)>,
    mut clock: Local<f32>,
    mut previous: Local<Option<usize>>,
    mut attack: Local<(u64, Option<f32>)>,
) {
    let pin = hunter.pin.filter(|_| {
        state.infected
            && !state.dead
            && state.special_phase == 3
            && state.special_class == SpecialClass::Hunter
    });
    if pin.is_none() {
        for (entity, _, _, _) in &draws {
            commands.entity(entity).despawn();
        }
        *previous = None;
        *clock = 0.;
        attack.1 = None;
        return;
    }
    if *previous != pin {
        *clock = 0.;
        *previous = pin;
        *attack = (state.shot_serial, None);
    }
    if !state.paused {
        *clock += time.delta_secs();
        advance_pin_attack(&mut attack, state.shot_serial, time.delta_secs());
    }
    let transform =
        Transform::from_translation(body.feet).with_rotation(pin_rotation(hunter.pin_yaw, true));
    if draws.is_empty() {
        if let Some(parts) = models.0.get(&(HUNTER_MODEL.into(), 0, 0)) {
            for (index, (mesh, material)) in parts.iter().enumerate() {
                let Some(mut owned) = meshes.get(mesh).cloned() else {
                    continue;
                };
                library.hunter_pin(index, *clock, attack.1, &mut owned);
                commands.spawn((
                    PinDraw(index),
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
                library.hunter_pin(draw.0, *clock, attack.1, &mut mesh);
            }
        }
    }
}
pub fn infected_only_brush(class: &str) -> bool {
    matches!(
        class,
        "func_simpleladder" | "func_playerinfected_clip" | "func_playerghostinfected_clip"
    )
}
pub fn infected_only_material(material: &str) -> bool {
    matches!(
        material,
        "tools/toolsinvisibleladder" | "dev/dev_measureladder01"
    ) || material.contains("infectedclip")
        || material.contains("zombieclimb")
}
/// Shared local infected-player state (historically Hunter-only, hence the
/// name). `ghost`, `pin`, `cooldown` and `ability_progress` apply to every
/// class; the remaining fields are Hunter ability state.
#[derive(Resource, Default)]
pub struct Hunter {
    saved: Option<Match>,
    map: std::path::PathBuf,
    pub ghost: bool,
    pub charge: f32,
    pub cooldown: f32,
    /// Squad index of the survivor currently pinned/ridden/pummelled.
    pub pin: Option<usize>,
    pub(crate) pin_yaw: f32,
    /// Ability meter shown by the HUD for non-Hunter classes (0..1).
    /// `None` uses the Hunter crouch-charge readiness.
    pub ability_progress: Option<f32>,
    /// Where unpinned survivor bots go to shove/rescue during a pin.
    /// `None` means the attacker's feet (correct for Hunter/Charger/Jockey).
    pub rescue_point: Option<Vec3>,
    airborne: bool,
    rebound_normal: Vec3,
    rebound_time: f32,
    jump_stamina: f32,
    damage_clock: f32,
    previous: Vec3,
    /// Retail player-infected voice state; see `voice`.
    voice: voice::VoiceState,
    class: SpecialClass,
    /// The third-person view follows behind the player looking forward
    /// (Charger charge) instead of orbiting the pin. Set by the class tick.
    pub(crate) chase_camera: bool,
}
#[cfg(test)]
impl Hunter {
    pub(crate) fn test_start_pounce(&mut self, from: Vec3) {
        self.airborne = true;
        self.ghost = false;
        self.previous = from;
    }
}
pub fn install(app: &mut App) {
    app.init_resource::<Hunter>()
        .add_systems(Update, switch_team.before(crate::game::controls));
    app.add_systems(Update, pin_model.after(crate::game::controls));
    app.add_systems(Update, infected_light.after(crate::game::controls));
    crate::infected_hud::install(app);
    // Each class schedules its own FixedUpdate ability after `tick`, which
    // owns the shared ghost/death lifecycle and survivor-bot opposition.
    smoker::install(app);
    boomer::install(app);
    spitter::install(app);
    jockey::install(app);
    charger::install(app);
    tank::install(app);
    witch::install(app);
    ai::install(app);
    death_effects::install(app);
    app.add_systems(FixedUpdate, voice::tick.after(tick));
}
pub(crate) fn switch_team(
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    session: Res<crate::campaign_settings::SessionSettings>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    if hunter.map != session.map {
        *hunter = Hunter {
            map: session.map.clone(),
            ..default()
        };
    }
    if state.infected && hunter.saved.is_none() {
        let mut survivor = state.clone();
        survivor.infected = false;
        if survivor.carried.take().is_some() {
            survivor.slot = 2;
        }
        hunter.saved = Some(survivor);
        hunter.ghost = true;
        hunter.jump_stamina = 0.;
        hunter.class = state.special_class;
        state.health = state.special_class.health();
        state.temporary = 0.;
        state.dead = false;
        state.incap = false;
        state.action = 0.;
        state.reload = 0.;
        state.medical_kind = 0;
        state.slot = 2;
        state.special_phase = 5;
        state.adrenaline_time = 0.;
        state.shove = 0.;
        state.cooldown = 0.;
        state.throw_stage = 0;
        body.velocity = Vec3::ZERO;
        hunter.previous = body.feet;
        if std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--capture-hunter-pounce"
                    | "--capture-hunter-flight"
                    | "--capture-hunter-ready"
                    | "--capture-hunter-live"
            )
        }) {
            hunter.ghost = false;
            state.special_phase = 0;
            if !std::env::args().any(|a| a == "--capture-hunter-live") {
                keys.press(KeyCode::ControlLeft);
            }
            let direction = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
            for (i, member) in squad.0.iter_mut().enumerate() {
                member.position = body.feet
                    + direction
                        * (if std::env::args().any(|a| a == "--capture-hunter-flight") {
                            25.
                        } else {
                            2.5
                        } + i as f32 * 2.);
                member.cooldown = 10.;
            }
        }
    } else if state.infected && hunter.class != state.special_class {
        // Changing role while infected returns to ghost placement.
        release(&mut hunter, &mut squad, &mut state);
        hunter.class = state.special_class;
        hunter.ghost = true;
        hunter.cooldown = 0.;
        hunter.ability_progress = None;
        hunter.rescue_point = None;
        state.health = state.special_class.health();
        state.dead = false;
        state.special_phase = 5;
        body.velocity = Vec3::ZERO;
    } else if !state.infected {
        if let Some(mut survivor) = hunter.saved.take() {
            survivor.paused = state.paused;
            *state = survivor;
            body.velocity = Vec3::ZERO;
        }
        // Release only the pin this player-controlled infected made: AI
        // specials pin bots through the same flags, and clearing every
        // member each tick erased those pins (no pinned glow, animation or
        // rescue for a bot held by an AI Hunter).
        if let Some(i) = hunter.pin.take() {
            if let Some(member) = squad.0.get_mut(i) {
                member.state.pinned = false;
                member.state.pinned_by = None;
            }
        }
        hunter.airborne = false;
        hunter.charge = 0.;
        hunter.ability_progress = None;
        hunter.rescue_point = None;
    }
}
pub(crate) fn can_spawn(c: &Collision, position: Vec3, squad: &Squad) -> bool {
    let c = c.for_actor(true, false);
    c.infected_spawn_allowed(position)
        && c.clear_hull(position, HEIGHT)
        && c.grounded(position, HEIGHT)
        && squad.0.iter().filter(|m| !m.state.dead).all(|m| {
            m.position.distance(position) > 1.5
                && c.trace(
                    m.position + Vec3::Y * 1.55,
                    position + Vec3::Y * 1.55,
                    Vec3::ZERO,
                )
                .fraction
                    < 1.
        })
}
fn segment_distance(point: Vec3, from: Vec3, to: Vec3) -> f32 {
    let delta = to - from;
    let t = ((point - from).dot(delta) / delta.length_squared().max(1e-8)).clamp(0., 1.);
    point.distance(from + delta * t)
}
pub(crate) fn launch_velocity(direction: Vec3) -> Vec3 {
    direction.normalize_or_zero() * (600. * UNIT) + Vec3::Y * (150. * UNIT)
}
fn stamina_launch(direction: Vec3, stamina: &mut f32) -> Vec3 {
    // Owned server FUN_102d6080: vertical scale 1 - stamina_ms * .00019;
    // jump sets stamina to 1315.789ms. FUN_1023a260 recovers 1000ms/sec.
    let mut velocity = launch_velocity(direction);
    velocity.y *= (1. - *stamina * 0.19).max(0.);
    *stamina = 1.3157895;
    velocity
}
fn can_rebound(normal: Vec3, direction: Vec3) -> bool {
    normal.length_squared() > 0.5 && normal.y < 0.2 && launch_velocity(direction).dot(normal) > 0.01
}
pub(crate) fn swept_victim(from: Vec3, to: Vec3, feet: Vec3, height: f32) -> Option<f32> {
    let min = feet
        + Vec3::new(
            -2. * crate::collision::RADIUS,
            -crate::collision::DUCK_HEIGHT,
            -2. * crate::collision::RADIUS,
        );
    let max = feet
        + Vec3::new(
            2. * crate::collision::RADIUS,
            height,
            2. * crate::collision::RADIUS,
        );
    let delta = to - from;
    let mut enter = 0_f32;
    let mut exit = 1_f32;
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
/// Height of a survivor lying under its attacker (Hunter pounce, Smoker
/// tongue, Charger pummel), for teammates' friendly-fire checks; `None`
/// keeps the standing box (a Jockey ride holds the victim upright).
pub(crate) fn pinned_height(state: &Match) -> Option<f32> {
    (state.pinned && state.pinned_by != Some(SpecialClass::Jockey)).then_some(0.45)
}
/// Marks a survivor as pinned by `class` (drives victim animation/AI).
pub(crate) fn pin_member(squad: &mut Squad, index: usize, class: SpecialClass) {
    if let Some(member) = squad.0.get_mut(index) {
        member.state.pinned = true;
        member.state.pinned_by = Some(class);
    }
}
/// Ends any active pin and returns the infected player to phase 0.
pub(crate) fn release(hunter: &mut Hunter, squad: &mut Squad, state: &mut Match) {
    if let Some(i) = hunter.pin.take() {
        if let Some(member) = squad.0.get_mut(i) {
            member.state.pinned = false;
            member.state.pinned_by = None;
        }
    }
    hunter.rescue_point = None;
    hunter.airborne = false;
    hunter.rebound_time = 0.;
    hunter.rebound_normal = Vec3::ZERO;
    hunter.cooldown = 1.;
    hunter.charge = 0.;
    state.special_phase = 0;
}
pub fn tick(
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    collision: Res<Collision>,
    rules: Res<Rules>,
    mut state: ResMut<Match>,
    mut hunter: ResMut<Hunter>,
    mut input: ResMut<crate::game::Input>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut actors: Query<(&CompanionActor, &mut Transform)>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut bot_aim: Local<crate::bot_combat::Aim>,
) {
    if !state.infected || state.paused || logic.as_ref().is_some_and(|l| l.active) {
        if !state.infected { bot_aim.reset(); }
        return;
    }
    let collision = collision.for_actor(true, hunter.ghost);
    let dt = time.delta_secs();
    hunter.cooldown = (hunter.cooldown - dt).max(0.);
    if state.dead {
        bot_aim.reset();
        release(&mut hunter, &mut squad, &mut state);
        if keys.just_pressed(KeyCode::KeyR) {
            if let Ok(point) = collision.supported_spawn_near(body.feet, HEIGHT) {
                body.feet = point;
                body.velocity = Vec3::ZERO;
                state.health = state.special_class.health();
                state.dead = false;
                state.incap = false;
                hunter.ghost = true;
                state.special_phase = 5;
            }
        }
        return;
    }
    if hunter.ghost {
        bot_aim.reset();
        state.special_phase = 5;
        let spawn = input.fire || input.fire_held;
        // Spawn consumes primary attack so the same click cannot fire an ability.
        input.fire = false;
        input.fire_held = false;
        if spawn && can_spawn(&collision, body.feet, &squad) {
            input.consume_spawn_attack();
            // BecomeReal and other lifecycle voices: see `voice`.
            hunter.ghost = false;
            state.special_phase = 0;
        }
        return;
    }
    let direction = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let eye = body.feet + Vec3::Y;
    // Other classes run their ability in special_infected/<class>.rs after
    // this system; only the shared opposition below applies to them.
    if state.special_class == SpecialClass::Hunter {
        hunter.rebound_time = (hunter.rebound_time - dt).max(0.);
        hunter.jump_stamina = (hunter.jump_stamina - dt).max(0.);
        if !body.grounded
            && body.contact_normal.length_squared() > 0.5
            && body.contact_normal.y < 0.2
        {
            hunter.rebound_normal = body.contact_normal;
            hunter.rebound_time = 0.1;
        }
        let test = std::env::args().any(|a| {
            matches!(
                a.as_str(),
                "--capture-hunter-pounce" | "--capture-hunter-flight"
            )
        });
        // Charge belongs to the held crouch, not to each individual jump.
        // Keep it through flight/rebounds so landing does not add another second.
        if hunter.pin.is_none() && keys.pressed(KeyCode::ControlLeft) {
            // Retail has no ready cue; the crouched Warn voice is in `voice`.
            hunter.charge = (hunter.charge + dt).min(1.);
        } else {
            hunter.charge = 0.;
        }
        // Retail retries a held attack once charge/contact and the launch interval
        // permit it; turning away from a wall must not require another mouse edge.
        let fire = std::mem::take(&mut input.fire)
            || input.fire_held
            || test && state.elapsed >= 1.2 && hunter.charge >= 1.;
        let claw = std::mem::take(&mut input.shove) || input.shove_held;
        input.reload = false;
        input.use_item = false;
        // A player can turn before pressing attack. Confirm the surface behind
        // the intended launch instead of relying only on the last collision tick.
        if fire && !body.grounded && keys.pressed(KeyCode::ControlLeft) {
            let height = if body.crouched {
                crate::collision::DUCK_HEIGHT
            } else {
                HEIGHT
            };
            let center = body.feet + Vec3::Y * height * 0.5;
            let hit = collision.trace(
                center,
                center - launch_velocity(direction).normalize_or_zero() * (2. * UNIT),
                Vec3::new(
                    crate::collision::RADIUS,
                    height * 0.5,
                    crate::collision::RADIUS,
                ),
            );
            if !hit.start_solid && hit.fraction < 1. && can_rebound(hit.normal, direction) {
                hunter.rebound_normal = hit.normal;
                hunter.rebound_time = 0.1;
            }
        }
        let rebound = !body.grounded
            && hunter.rebound_time > 0.
            && keys.pressed(KeyCode::ControlLeft)
            && can_rebound(hunter.rebound_normal, direction);
        if fire
            && (hunter.charge >= 0.999 && body.grounded || rebound)
            && hunter.cooldown <= 0.
            && hunter.pin.is_none()
        {
            // The retail wall recordings also lift the feet by twenty units.
            // A ceiling must still prevent this clearance shift.
            let lifted = body.feet + Vec3::Y * (20. * UNIT);
            if collision.clear_hull(lifted, crate::collision::DUCK_HEIGHT) {
                body.feet = lifted;
            }
            body.velocity = stamina_launch(direction, &mut hunter.jump_stamina);
            body.grounded = false;
            hunter.airborne = true;
            hunter.cooldown = 0.1;
            hunter.rebound_time = 0.;
            hunter.previous = body.feet;
            state.special_phase = 2;
            // A rebound remains in phase 2, but is a fresh viewmodel lunge.
            state.shot_serial = state.shot_serial.wrapping_add(1);
            voice::pounce_voice(&mut hunter, &mut sounds);
        }
        if hunter.airborne {
            let victim = squad
                .0
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.state.dead && !m.state.pinned)
                .filter_map(|(i, m)| {
                    swept_victim(
                        hunter.previous,
                        body.feet,
                        m.position,
                        if m.state.incap {
                            crate::collision::DUCK_HEIGHT
                        } else {
                            HEIGHT
                        },
                    )
                    .map(|fraction| (i, fraction))
                })
                .filter(|(i, _)| {
                    collision
                        .trace(eye, squad.0[*i].position + Vec3::Y, Vec3::ZERO)
                        .fraction
                        >= 0.99
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            if let Some(i) = victim {
                hunter.pin = Some(i);
                hunter.pin_yaw = body.yaw;
                hunter.airborne = false;
                hunter.damage_clock = 1.;
                pin_member(&mut squad, i, SpecialClass::Hunter);
                state.special_phase = 3;
                body.velocity = Vec3::ZERO;
                sounds.play("HunterZombie.Pounce.Hit", None);
            } else if body.grounded {
                hunter.rebound_time = 0.;
                hunter.airborne = false;
                state.special_phase = 0;
                sounds.play("HunterZombie.Pounce.Miss", None);
            }
        }
        if let Some(i) = hunter.pin {
            if squad.0.get(i).is_none_or(|m| m.state.dead) {
                release(&mut hunter, &mut squad, &mut state);
            } else {
                body.velocity = Vec3::ZERO;
                body.feet = squad.0[i].position;
                hunter.damage_clock -= dt;
                if hunter.damage_clock <= 0. {
                    squad.0[i].state.damage(5.);
                    state.shot_serial = state.shot_serial.wrapping_add(1);
                    // Shred voice and Melee_Pounce slices: see `voice`.
                    hunter.damage_clock += 0.5;
                }
            }
        } else if claw && hunter.cooldown <= 0. {
            let victim = squad
                .0
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.state.dead)
                .filter(|(_, m)| {
                    let delta = m.position + Vec3::Y - eye;
                    delta.length() < 75. * UNIT
                        && direction.dot(delta.normalize_or_zero()) > 0.6
                        && collision
                            .trace(eye, m.position + Vec3::Y, Vec3::ZERO)
                            .fraction
                            >= 1.
                })
                .map(|(i, _)| i)
                .next();
            if let Some(i) = victim {
                squad.0[i].state.damage(6.);
                sounds.play("Claw.HitFlesh", None);
            } else if logic
                .as_mut()
                .is_some_and(|l| l.pound_obstacle(body.feet, direction, &collision))
            {
                sounds.play("Claw.Hit", None);
            } else {
                sounds.play("Claw.Swing", None);
            }
            hunter.cooldown = 0.6;
            state.special_phase = 4;
            state.shot_serial = state.shot_serial.wrapping_add(1);
            voice::clawed(&mut hunter);
        } else if state.special_phase == 4 && hunter.cooldown <= 0. {
            state.special_phase = 0;
        }
    }
    // Local survivor opposition and pin rescue; full retail combat tactics remain pending.
    let rescue_point = hunter.rescue_point.unwrap_or(body.feet);
    let mut rescue = false;
    for (index, member) in squad.0.iter_mut().enumerate() {
        let weapon = rules.weapons.get(&member.state.gun().class).unwrap_or(&rules.pistol);
        bot_aim.advance(index, dt, weapon);
        if member.state.dead
            || member.state.incap
            || member.state.pinned
            || crate::game_state::bots_stopped()
        {
            bot_aim.forget(index);
            continue;
        }
        member.cooldown = (member.cooldown - dt).max(0.);
        member.state.advance_reload(weapon, dt);
        let delta = body.feet - member.position;
        let to_rescue = rescue_point - member.position;
        if hunter.pin.is_some() && to_rescue.length() > 1.8 {
            let direction = (to_rescue * Vec3::new(1., 0., 1.)).normalize_or_zero();
            member.position = collision.walk(
                member.position,
                direction * (220. * UNIT) * dt,
                HEIGHT,
                collision.grounded(member.position, HEIGHT),
            );
        }
        let origin = member.position + Vec3::Y * crate::collision::EYE;
        let height = if body.crouched { crate::collision::DUCK_HEIGHT } else { HEIGHT };
        let target = body.feet + Vec3::Y * height * 0.55;
        let sight = collision.sight_trace(origin, target);
        let visible = !state.dead && !hunter.ghost && crate::bot_combat::in_view(target - origin, member.aim, member.state.vomited > 0.)
            && !sight.start_solid && sight.fraction >= 1.;
        let ready = if visible {
            bot_aim.observe(index, 0, target - origin, &mut member.aim, dt)
        } else { bot_aim.forget(index); false };
        // Retail z_charger_allow_shove=0: a Charger must be killed, not shoved.
        if hunter.pin.is_some()
            && to_rescue.length() < 2.
            && member.cooldown <= 0.
            && visible
            && state.special_class != SpecialClass::Charger
        {
            rescue = true;
            member.state.shove_serial = member.state.shove_serial.wrapping_add(1);
            member.state.shove = 0.7;
            member.cooldown = 0.7;
        } else if member.cooldown <= 0. && ready {
            if member.state.gun().ammo == 0 {
                if member.state.reload <= 0. {
                    member.state.begin_reload(weapon);
                }
            } else if member.state.reload <= 0. {
                member.state.gun_mut().ammo -= 1;
                let serial = member.state.shot_serial.wrapping_add(1);
                member.state.shot_serial = serial;
                let mut damage = 0.;
                for ray in bot_aim.rays(index, weapon, member.aim, serial, 0., member.state.gun().laser) {
                    let wall = collision.trace(origin, origin + ray * weapon.range, Vec3::ZERO).fraction * weapon.range;
                    if let Some(distance) = crate::ray_box(origin, ray,
                        body.feet + Vec3::new(-crate::collision::RADIUS, 0., -crate::collision::RADIUS),
                        body.feet + Vec3::new(crate::collision::RADIUS, height, crate::collision::RADIUS))
                        .filter(|d| *d < wall && *d <= weapon.range) {
                        damage += weapon.damage_at(distance);
                    }
                }
                bot_aim.fired(index, weapon);
                if damage > 0. {
                    state.damage_short = true;
                    state.damage(damage);
                }
                sounds.play(&weapon.fire_sound, Some(member.position));
                member.cooldown = weapon.shot_cycle(member.state.gun().ammo, false);
            }
        }
    }
    if rescue {
        release(&mut hunter, &mut squad, &mut state);
        if state.special_class == SpecialClass::Hunter {
            sounds.play("HunterZombie.Pounce.Cancel", None);
        }
    }
    for (actor, mut transform) in &mut actors {
        if let Some(member) = squad.0.get(actor.0) {
            if member.state.dead {
                continue;
            }
            transform.translation = member.position;
            transform.rotation = if member.state.pinned {
                pin_rotation(hunter.pin_yaw, false)
            } else {
                Quat::from_rotation_y((-member.aim.z).atan2(member.aim.x))
            };
        }
    }
    hunter.previous = body.feet;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn red_light_requires_a_live_pin_and_switches_off_on_release_or_death() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        let mut hunter = Hunter::default();
        assert_eq!(pin_light_intensity(&state, &hunter, true), 0.);
        state.special_phase = 2;
        assert_eq!(pin_light_intensity(&state, &hunter, true), 0.);
        state.special_phase = 3;
        hunter.pin = Some(0);
        assert_eq!(pin_light_intensity(&state, &hunter, true), 40.);
        assert_eq!(pin_light_intensity(&state, &hunter, false), 0.);
        hunter.ghost = true;
        assert_eq!(pin_light_intensity(&state, &hunter, true), 0.);
        hunter.ghost = false;
        state.dead = true;
        assert_eq!(pin_light_intensity(&state, &hunter, true), 0.);
        state.dead = false;
        hunter.pin = None;
        assert_eq!(pin_light_intensity(&state, &hunter, true), 0.);
    }
    #[test]
    fn pinned_hunter_faces_the_native_victim_head_direction() {
        for yaw in [0., 1., 3.] {
            let toward_head = pin_rotation(yaw, false) * Vec3::NEG_X;
            let hunter_forward = pin_rotation(yaw, true) * Vec3::X;
            assert!(hunter_forward.dot(toward_head) > 0.999);
        }
    }
    #[test]
    fn damage_ticks_do_not_truncate_the_native_pin_attack_cycle() {
        let mut attack = (8, None);
        advance_pin_attack(&mut attack, 8, 0.1);
        assert_eq!(attack.1, None);
        advance_pin_attack(&mut attack, 9, 0.1);
        advance_pin_attack(&mut attack, 10, 0.1);
        assert!((attack.1.unwrap() - 0.2).abs() < 1e-6);
    }
    #[test]
    fn pin_camera_orbits_pair_and_keeps_focus_on_victim() {
        let feet = Vec3::new(2., 0., 4.);
        for yaw in [0., 1., 3.] {
            let camera = pin_camera(feet, yaw, 0., None);
            let target = feet + Vec3::Y * 0.85;
            assert!(camera.translation.distance(target) > 3.);
            assert!(
                (camera.rotation * Vec3::NEG_Z).dot((target - camera.translation).normalize())
                    > 0.999
            );
            assert!(camera.translation.y > target.y);
        }
    }
    #[test]
    fn pin_camera_stops_before_wall_behind_player() {
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(-10., 0., -1.5), Vec3::new(10., 5., -1.));
        let camera = pin_camera(Vec3::ZERO, 0., 0., Some(&collision));
        assert!(camera.translation.z > -1.);
        assert!(camera.translation.z < 0.);
    }
    #[test]
    fn owned_hunter_assets_have_actions_sounds_and_hud_artwork() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for path in [
            HUNTER_MODEL,
            HUNTER_VIEW,
            "materials/vgui/hud/pz_charge_pounce.vtf",
            "materials/vgui/hud/pz_healthbar_250.vtf",
        ] {
            assert!(!assets.read(path).unwrap().is_empty());
        }
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "hunterzombie.alert",
            "hunterzombie.death",
            "hunterzombie.growl",
            "hunterzombie.pain",
            "hunterzombie.lunge",
            "hunterzombie.pounce",
            "hunterzombie.pounce.hit",
            "hunterzombie.pounce.miss",
            "hunterzombie.pounce.shred",
            "hunterzombie.pounce.cancel",
            "claw.hitflesh",
            "claw.swing",
        ] {
            let sound = registry.definitions.get(name).expect(name);
            assert!(!assets.read(&sound.waves[0]).unwrap().is_empty());
        }
        let assets = std::cell::RefCell::new(assets);
        let sk = l4d2_formats::anim::Skeleton::load_with(HUNTER_VIEW, |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        for activity in [
            "ACT_VM_DEPLOY",
            "ACT_VM_IDLE",
            "ACT_VM_LUNGE",
            "ACT_VM_PUMMEL_ATTACK",
            "ACT_VM_PRIMARYATTACK",
        ] {
            assert!(
                sk.sequences().iter().any(|s| s.activity_name == activity),
                "{activity}"
            );
        }
    }
    #[test]
    fn ghost_spawn_rejects_sightlines_and_hunter_is_invulnerable_until_spawn() {
        let mut c = Collision::test_solid_floor();
        let mut squad = Squad(vec![]);
        let mut member = crate::game_state::Companion {
            state: Match::new(&crate::game_state::tests_weapon()),
            position: Vec3::X * 4.,
            spawn: Vec3::X * 4.,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_X,
        };
        member.state.dead = false;
        squad.0.push(member);
        let p = Vec3::Y * 0.004;
        assert!(!can_spawn(&c, p, &squad));
        c.test_add_box(Vec3::new(1.5, 0., -2.), Vec3::new(2., 3., 2.));
        assert!(can_spawn(&c, p, &squad));
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        state.special_phase = 5;
        state.health = 250.;
        state.damage(500.);
        assert_eq!(state.health, 250.);
        assert!(!state.dead);
    }
    #[test]
    fn marker_layers_hide_only_infected_geometry_and_never_real_ladders() {
        for class in [
            "func_simpleladder",
            "func_playerinfected_clip",
            "func_playerghostinfected_clip",
        ] {
            assert!(infected_only_brush(class));
        }
        for class in ["prop_static", "func_ladder", "func_brush"] {
            assert!(!infected_only_brush(class));
        }
        assert!(!RenderLayers::from_layers(&[0, 1]).intersects(&RenderLayers::layer(MARKER_LAYER)));
        assert!(
            RenderLayers::from_layers(&[0, 1, MARKER_LAYER])
                .intersects(&RenderLayers::layer(MARKER_LAYER))
        );
    }
    #[test]
    fn player_launch_and_rebounds_use_outward_velocity() {
        let v = launch_velocity(Vec3::NEG_Z);
        assert!((v.z / UNIT + 600.).abs() < 0.001);
        assert!((v.y / UNIT - 150.).abs() < 0.001);
        assert!(can_rebound(Vec3::X, Vec3::X));
        assert!(!can_rebound(Vec3::X, Vec3::NEG_X));
        assert!(can_rebound(Vec3::NEG_Y, Vec3::NEG_Y));
        assert!(!can_rebound(Vec3::NEG_Y, Vec3::Y));
        assert!(!can_rebound(Vec3::Y, Vec3::Y));
    }
    #[test]
    fn repeated_pounce_stamina_matches_retail_wall_trace_and_recovers() {
        let mut stamina = 0.;
        let first = stamina_launch(Vec3::X, &mut stamina);
        assert!((first.y / UNIT - 150.).abs() < 0.001);
        // Native 1.3667 launch, 1.8000 rebound: outgoing vertical 98.1833
        // after one 30Hz gravity step. Horizontal velocity remains 600.
        stamina = (stamina - 13. / 30.).max(0.);
        let next = stamina_launch(Vec3::NEG_X, &mut stamina);
        assert!((next.y / UNIT - 800. / 30. - 98.183334).abs() < 0.001);
        assert!((next.x / UNIT + 600.).abs() < 0.001);
        stamina = (stamina - 2.).max(0.);
        let recovered = stamina_launch(Vec3::NEG_X, &mut stamina);
        assert!((recovered.y / UNIT - 150.).abs() < 0.001);
        let upward = launch_velocity(Vec3::new(1., 1., 0.));
        assert!(upward.y > first.y);
        assert!(upward.x < first.x);
    }
    #[test]
    fn swept_pounce_hits_torso_and_landing_on_head_but_not_nearby_passes() {
        assert!(swept_victim(Vec3::Z * 4., Vec3::NEG_Z * 4., Vec3::ZERO, HEIGHT).is_some());
        assert!(swept_victim(Vec3::Y * 4., Vec3::Y, Vec3::ZERO, HEIGHT).is_some());
        assert!(
            swept_victim(
                Vec3::new(2., 0., 4.),
                Vec3::new(2., 0., -4.),
                Vec3::ZERO,
                HEIGHT
            )
            .is_none()
        );
    }
    #[test]
    fn swept_pounce_contact_and_infected_death_do_not_use_survivor_incapacitation() {
        assert!(segment_distance(Vec3::X, Vec3::ZERO, Vec3::X * 2.) < 0.01);
        assert!(segment_distance(Vec3::Z * 2., Vec3::ZERO, Vec3::X * 2.) > 1.);
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.infected = true;
        state.health = 250.;
        state.damage(260.);
        assert!(state.dead && !state.incap);
    }
}
