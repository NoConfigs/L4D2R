//! Development lab over the real campaign runtime, not a separate game simulation.
use crate::{
    binary,
    collision::{Collision, HEIGHT, UNIT},
    game::{Body, Enemy},
    game_menu::{Frontend, Page},
    game_state::{Match, Rules},
    l4d2r_animation::AnimationLibrary,
    viewer::RenderModels,
};
use bevy::{
    camera::visibility::{NoFrustumCulling, RenderLayers},
    prelude::*,
};
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn path() -> PathBuf {
    // Android: unpacked next to the game data, the working directory.
    if cfg!(target_os = "android") {
        return PathBuf::from("maps/rewrite_lab.bsp");
    }
    // iOS: shipped inside the app bundle and unpacked to the app's cache.
    if cfg!(target_os = "ios") {
        if let Some(path) = ios_unpacked() {
            return path;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/rewrite_lab.bsp")
}
/// The bundle stores the navmesh as `rewrite_lab.navpack`: "L4DR" + the .nav.
/// A raw .nav starts with 0xFEEDFACE, the 32-bit Mach-O magic, and
/// sideloading signers (AltStore's ldid) reject the IPA when they see it.
/// The lab files are unpacked once to Library/Caches/maps.
fn ios_unpacked() -> Option<PathBuf> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let bundle = std::env::current_exe().ok()?.parent()?.join("maps");
        let cache = PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/maps");
        std::fs::create_dir_all(&cache).ok()?;
        for name in ["rewrite_lab.bsp", "rewrite_lab.json"] {
            let source = bundle.join(name);
            if source.is_file() {
                std::fs::copy(&source, cache.join(name)).ok()?;
            }
        }
        let pack = std::fs::read(bundle.join("rewrite_lab.navpack")).ok()?;
        let nav = pack.strip_prefix(b"L4DR")?;
        std::fs::write(cache.join("rewrite_lab.nav"), nav).ok()?;
        Some(cache.join("rewrite_lab.bsp"))
    })
    .clone()
}
pub fn active(path: &Path) -> bool {
    path.file_stem().is_some_and(|p| p == "rewrite_lab")
}
const STATIONS: [(&str, [f32; 3]); 6] = [
    ("Weapons / medical items", [128., 128., 4.]),
    ("Native animation stage / combat", [1850., 2300., 4.]),
    ("Doors / glass / physics", [3050., 950., 4.]),
    ("Triggers / fire / sound", [2150., 1260., 4.]),
    ("Stairs / crouching / jumping", [2550., 2320., 4.]),
    ("Return / reset", [3600., 2750., 4.]),
];
const MODELS: [&str; 6] = [
    "models/infected/common_male01.mdl",
    "models/survivors/survivor_mechanic.mdl",
    "models/survivors/survivor_producer.mdl",
    "models/survivors/survivor_coach.mdl",
    "models/survivors/survivor_gambler.mdl",
    crate::special_infected::HUNTER_MODEL,
];
pub fn models() -> Vec<String> {
    let mut models: Vec<_> = MODELS.iter().map(|s| s.to_string()).collect();
    let target = capture_gore_model();
    if !models.contains(&target) { models.push(target); }
    models
}
fn capture_gore_model() -> String {
    std::env::args().collect::<Vec<_>>().windows(2)
        .find(|v| v[0] == "--capture-gore-model").map(|v| v[1].clone())
        .unwrap_or_else(|| MODELS[0].into())
}
#[derive(Resource, Default)]
pub struct Lab {
    station: usize,
    model: usize,
    sequence: usize,
    clock: f32,
    names: Vec<String>,
    preview: Option<Entity>,
    note: String,
    director_enabled: bool,
    capture_relay: bool,
    capture_combat: bool,
    capture_route: bool,
    route_audit_at: f32,
    capture_panic: bool,
    director_case: usize,
    director_audit_at: f32,
    spawned: usize,
    medical_case: usize,
    capture_medical: usize,
    medical_audit: bool,
    blood_shots: u8,
    blood_shot_at: f32,
}
#[derive(Component)]
pub(crate) struct Panel;
#[derive(Component)]
pub(crate) struct ImpactTarget;
#[derive(Component)]
pub(crate) struct RouteTarget;
#[derive(Component)]
struct StageDraw {
    model: String,
    index: usize,
}

pub fn install(app: &mut App) {
    let args = std::env::args().collect::<Vec<_>>();
    let number = |flag: &str| {
        args.windows(2)
            .find(|a| a[0] == flag)
            .and_then(|a| a[1].parse::<usize>().ok())
            .unwrap_or(0)
    };
    app.insert_resource(Lab {
        model: number("--lab-model") % MODELS.len(),
        sequence: if args.iter().any(|a| a == "--lab-sequence") {
            number("--lab-sequence")
        } else {
            usize::MAX
        },
        capture_relay: args.iter().any(|a| a == "--lab-capture-relay"),
        capture_combat: args.iter().any(|a| {
            a == "--lab-capture-combat" || a == "--lab-capture-route" || a == "--lab-bot-combat"
        }),
        capture_route: args.iter().any(|a| a == "--lab-capture-route"),
        capture_panic: args.iter().any(|a| a == "--lab-director-panic")
            || number("--lab-director-script-case") > 0,
        director_case: number("--lab-director-script-case") % 6,
        capture_medical: if args.iter().any(|a| a == "--hotel-live-kill-stress") {
            11
        } else if args.iter().any(|a| {
            a == "--lab-ragdoll-air" || a == "--lab-corpse-stress" || a == "--hotel-corpse-stress"
        }) {
            15
        } else if args.iter().any(|a| a == "--lab-bot-items") {
            14
        } else if number("--lab-hitbox-case") > 0 {
            9 + number("--lab-hitbox-case").clamp(1, 4)
        } else {
            number("--lab-medical-case")
        },
        ..default()
    })
    .add_systems(
        Startup,
        setup
            .after(crate::viewer::RenderSetup::Scene)
            .run_if(on_lab),
    )
    .add_systems(
        Update,
        update.after(crate::l4d2r_animation::update).run_if(on_lab),
    )
    .add_systems(PostUpdate, hide_comparison_viewmodel.run_if(on_lab));
}
fn hide_comparison_viewmodel(mut cameras: Query<&mut Camera, With<crate::game::ViewCamera>>) {
    if std::env::args().any(|a| a == "--capture-gore-bind-pose") {
        for mut camera in &mut cameras { camera.is_active = false; }
    }
}
fn on_lab(session: Res<crate::campaign_settings::SessionSettings>) -> bool {
    active(&session.map)
        || std::env::args().any(|a| a == "--hotel-corpse-stress" || a == "--hotel-live-kill-stress")
}
pub(crate) fn rebuild(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world.insert_resource(Lab {
        sequence: usize::MAX,
        ..default()
    });
    world
        .run_system_once(setup)
        .expect("Rebuild laboratory overlay");
}
fn setup(mut commands: Commands) {
    commands.spawn((
        Text::new("REWRITE LAB: click to capture the mouse and begin.\nF3 stations | F1 stage model | F2 sequence | F9 reset"),
        TextFont {
            font_size: FontSize::Px(16.),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: px(12.),
            top: px(35.),
            width: px(650.),
            ..default()
        },
        TextColor(Color::srgb(1., 0.95, 0.7)),
        TextShadow::default(),
        Panel,
    ));
}
fn validated_corpse_positions(collision: &Collision, origin: Vec3, count: usize) -> Vec<Vec3> {
    let mut points = Vec::new();
    for ring in 1..=32 {
        for sample in 0..32 {
            let angle = sample as f32 * std::f32::consts::TAU / 32.;
            let candidate = origin + Vec3::new(angle.cos(), 0., angle.sin()) * (ring as f32 * 0.25);
            let ground = collision.trace(
                candidate + Vec3::Y * 0.4,
                candidate - Vec3::Y * 1.5,
                Vec3::ZERO,
            );
            if ground.start_solid || ground.fraction >= 1. || ground.normal.y < 0.7 {
                continue;
            }
            let point =
                candidate + Vec3::Y * 0.4 - Vec3::Y * 1.9 * ground.fraction + Vec3::Y * 0.004;
            if (point.y - origin.y).abs() > 0.5
                || !collision.clear_hull(point, HEIGHT)
                || points
                    .iter()
                    .any(|p: &Vec3| p.distance_squared(point) < 0.55f32.powi(2))
            {
                continue;
            }
            points.push(point);
            if points.len() == count {
                return points;
            }
        }
    }
    points
}
fn update(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut lab: ResMut<Lab>,
    mut state: ResMut<Match>,
    mut frontend: ResMut<Frontend>,
    mut body: Single<&mut Body>,
    medical_runtime: (
        Res<Rules>,
        ResMut<crate::game_state::Squad>,
        ResMut<crate::game::Input>,
        Option<Res<crate::bullet_impacts::Impacts>>,
        Query<(Entity, &Transform, &Enemy), With<ImpactTarget>>,
        Option<Res<crate::l4d2r_animation::PlaybackState>>,
        Option<ResMut<crate::studio_hitboxes::Audit>>,
        Query<&crate::game::CompanionGun>,
        Query<(&Transform, &Enemy), With<RouteTarget>>,
    ),
    models: Res<RenderModels>,
    library: Res<AnimationLibrary>,
    collision: Res<Collision>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    mut director: ResMut<crate::source_director::Director>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut panel: Single<(&mut Text, &mut Visibility), With<Panel>>,
    draws: Query<(&StageDraw, &Mesh3d)>,
) {
    let (
        rules,
        mut squad,
        mut input,
        impacts,
        targets,
        playback,
        mut hit_audit,
        bot_items,
        route_targets,
    ) = medical_runtime;
    // Opt-in close inspection of floor-level corpse/wound fixtures.
    static INSPECT_PITCH: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    if let Some(pitch) = INSPECT_PITCH.get_or_init(|| {
        let args = std::env::args().collect::<Vec<_>>();
        args.windows(2).find(|a| a[0] == "--lab-inspect-pitch")
            .and_then(|a| a[1].parse::<f32>().ok()).filter(|p| p.is_finite())
    }) {
        if lab.clock > 2.4 { body.pitch = pitch.clamp(-89., 89.).to_radians(); }
    }
    let playing = frontend.page == Page::Game;
    *panel.1 = if playing && !std::env::args().any(|a| a == "--capture-gore-bind-pose") {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if !playing || state.paused {
        return;
    }
    if !lab.director_enabled {
        director.command("disable", "");
    }
    if keys.just_pressed(KeyCode::F7) || lab.capture_panic && lab.clock > 0.8 {
        let scripted = lab.capture_panic && lab.director_case > 0
            || keys.pressed(KeyCode::ShiftLeft)
            || keys.pressed(KeyCode::ShiftRight);
        if keys.just_pressed(KeyCode::F7) && scripted {
            lab.director_case = (lab.director_case + 1) % 6;
        }
        if lab.capture_panic {
            // Stage the party beside the lab's occluding test wall. At the
            // weapon-table start there are no hidden areas inside mob range.
            body.feet = Vec3::from_array(binary::source_position([3050., 950., 4.]));
            body.velocity = Vec3::ZERO;
            body.yaw = 0.;
            body.pitch = 0.;
            for (i, member) in squad.0.iter_mut().enumerate() {
                member.position = body.feet - Vec3::X * 2.5 + Vec3::Z * (i as f32 - 1.) * 1.25;
                member.vertical = 0.;
            }
        }
        lab.capture_panic = false;
        if lab.director_case == 4 {
            lab.director_audit_at = lab.clock + 1.;
        }
        lab.director_enabled = scripted || !lab.director_enabled;
        let script = match lab.director_case {
            1 => "director_quiet",
            2 => "c14_junkyard_crane",
            3 => "c7m1_docks",
            4 => "director_gauntlet",
            _ => "",
        };
        if scripted {
            director.command("EndScript", "");
            if !script.is_empty() {
                director.command("BeginScript", script);
            }
        }
        director.command(
            if lab.director_enabled {
                "enable"
            } else {
                "disable"
            },
            "",
        );
        if lab.director_enabled {
            if std::env::args().any(|a| a == "--lab-map-panic") {
                if let Some(&button) = logic.world.find("lab_panic_button").first() {
                    logic.world.use_ent(button, None);
                }
            } else {
                director.command("panicevent", "");
            }
        }
        println!(
            "Lab Director script: case={} script={} limit={} pending={}",
            lab.director_case,
            script,
            director.limit(),
            director.pending
        );
        if lab.director_case == 5 {
            lab.director_enabled = false;
            director.command("Disable", "");
            if let Some(id) = logic.world.find("lab_door").first().copied() {
                director.entity_script(id,"RunScriptCode",r#"
                    calls <- 0;
                    function OnOpenOutput(){Assert(caller.id==self.id); EntFire("lab_hint","ShowHudHint");}
                    function ValveThink(){
                        calls++;
                        local door=Entities.FindByClassnameNearest("prop_door_rotating",self.GetOrigin(),10);
                        Assert(door!=null && door.id==self.id);
                        EntFire("!self",calls==1?"Open":"Close");
                        if(calls>=2) AddThinkToEnt(self,null);
                        return 0.6;
                    }
                    ConnectOutputs(this);AddThinkToEnt(self,"ValveThink");
                "#);
            }
            lab.note="Valve bindings: timed think opens/closes the lab door, spatial lookup checks self, and OnOpenOutput triggers the hint with the native caller context.".into();
        }
        if lab.director_case != 5 {
            lab.note = format!(
                "Director {}: {:?}; script={}; common limit={}; queued={}. Shift+F7 cycles quiet/crane/docks/gauntlet/Valve bindings/default.",
                if lab.director_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                director.pace,
                if script.is_empty() { "default" } else { script },
                director.limit(),
                director.pending
            );
        }
    }
    if lab.director_enabled && lab.director_case == 4 {
        lab.note = format!(
            "Live Squirrel gauntlet: common limit={} pending={} NAV flow={:.0} average speed={:.0}. Shift+F7 changes script.",
            director.limit(),
            director.pending,
            director.flow,
            director.survivor_speed
        );
        if lab.director_audit_at > 0. && lab.clock >= lab.director_audit_at {
            println!(
                "Lab live VM: limit={} pending={} flow={:.1} speed={:.1}",
                director.limit(),
                director.pending,
                director.flow,
                director.survivor_speed
            );
            lab.director_audit_at = lab.clock + 2.;
        }
    }
    if keys.just_pressed(KeyCode::F3) {
        lab.station = (lab.station + 1) % STATIONS.len();
        let position = Vec3::from_array(binary::source_position(STATIONS[lab.station].1));
        match collision.supported_spawn_near(position, HEIGHT) {
            Ok(p) => {
                body.feet = p;
                body.velocity = Vec3::ZERO;
                body.pitch = 0.;
                body.yaw = -std::f32::consts::FRAC_PI_2;
            }
            Err(e) => lab.note = format!("Station spawn failed: {e}"),
        }
    }
    if keys.just_pressed(KeyCode::F1) {
        lab.model = (lab.model + 1) % MODELS.len();
        lab.sequence = usize::MAX;
        lab.clock = 0.;
        lab.names.clear();
    }
    if keys.just_pressed(KeyCode::F2) && !lab.names.is_empty() {
        let backwards = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        lab.sequence =
            (lab.sequence + if backwards { lab.names.len() - 1 } else { 1 }) % lab.names.len();
        lab.clock = 0.;
    }
    if keys.just_pressed(KeyCode::F4) || lab.capture_combat && lab.clock > 0.8 {
        let route = lab.capture_route
            || keys.pressed(KeyCode::ShiftLeft)
            || keys.pressed(KeyCode::ShiftRight);
        lab.capture_route = false;
        lab.capture_combat = false;
        if route {
            body.feet = Vec3::from_array(binary::source_position([1920., 600., 4.]));
            body.velocity = Vec3::ZERO;
            body.yaw = -std::f32::consts::FRAC_PI_2;
            body.pitch = 0.;
            for (i, member) in squad.0.iter_mut().enumerate() {
                member.position =
                    Vec3::from_array(binary::source_position([128., 128. + i as f32 * 80., 4.]));
                member.vertical = 0.;
                member.state.dead = true;
            }
            lab.route_audit_at = lab.clock + 16.;
        }
        let model = "models/infected/common_male01.mdl";
        if let Some(handles) = models.0.get(&(model.into(), 0, 0)) {
            for i in 0..4 {
                if lab.spawned >= 32 {
                    break;
                }
                let p = Vec3::from_array(binary::source_position(if route {
                    [1700. + i as f32 * 8., 600. + i as f32 * 8., 4.]
                } else {
                    [2050. + i as f32 * 90., 900., 4.]
                }));
                if !collision.clear_hull(p, HEIGHT) {
                    continue;
                }
                let root = commands
                    .spawn((
                        Transform::from_translation(p),
                        Visibility::Inherited,
                        Enemy {
                            model: "models/infected/common_male01.mdl".into(),
                            death_loot: None,
                            loot_dropped: false,
                            reaction_serial: 0,
                            attack_serial: 0,
                            health: if route { 2000. } else { 50. },
                            attack: 0.,
                            reaction: 0,
                            corpse_time: 0.,
                            death: Default::default(),
                            movement_speed: 0.,
                            burning: 0.,
                            vertical: 0.,
                        },
                    ))
                    .id();
                if route {
                    commands.entity(root).insert(RouteTarget);
                }
                lab.spawned += 1;
                for (mesh, material) in handles {
                    commands.spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        crate::viewer::DynamicModelLighting::default(),
                        Transform::default(),
                        RenderLayers::layer(1),
                        ChildOf(root),
                    ));
                }
            }
            lab.note = if route {
                "Wall route/crowd test: four durable infected start across the solid wall. Shift+F4 repeats; F9 resets.".into()
            } else {
                format!(
                    "Manual combat spawn budget: {}/32; shoot / shove; F9 resets it.",
                    lab.spawned
                )
            };
        }
    }
    if lab.route_audit_at > 0. && lab.clock >= lab.route_audit_at {
        let actors = route_targets
            .iter()
            .filter(|(_, e)| e.health > 0.)
            .map(|(t, _)| t.translation)
            .collect::<Vec<_>>();
        let mut nearest = f32::INFINITY;
        for i in 0..actors.len() {
            for j in i + 1..actors.len() {
                nearest = nearest.min(actors[i].distance(actors[j]));
            }
        }
        let reached = actors.iter().filter(|p| p.distance(body.feet) < 2.).count();
        println!(
            "Lab infected route: alive={} reached={} nearest_pair={nearest:.3} positions={actors:?}",
            actors.len(),
            reached
        );
        lab.note = format!(
            "Wall route result: {reached}/{} within 2m; nearest pair {nearest:.2}m. Full infected behavior remains open.",
            actors.len()
        );
        lab.route_audit_at = 0.;
    }
    if keys.just_pressed(KeyCode::F5) {
        state.health = 25.;
        state.medkit = true;
        state.defibrillator = false;
        state.pills = true;
        state.adrenaline = false;
        state.dead = false;
        state.incap = false;
        lab.note =
            "Health 25; medkit and pills supplied. Use 4/5 and mouse to test healing.".into();
    }
    if keys.just_pressed(KeyCode::F11) || lab.capture_medical > 0 && lab.clock > 2.1 {
        let capture = lab.capture_medical > 0;
        lab.medical_case = if capture {
            std::mem::take(&mut lab.capture_medical)
        } else if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            if lab.medical_case >= 10 {
                10 + (lab.medical_case - 9) % 6
            } else {
                10
            }
        } else {
            (lab.medical_case % 3) + 1
        };
        let direction = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
        match lab.medical_case {
            1 => {
                state.health = 50.;
                state.adrenaline = true;
                state.pills = false;
                state.slot = 5;
                if capture {
                    input.fire = true;
                }
                lab.note="Adrenaline supplied: left click; native use animation then 25 temporary HP and a timed speed boost.".into();
            }
            2 | 3 => {
                let p = body.feet + direction * 1.2;
                if let Some(m) = squad.0.first_mut() {
                    m.position = p;
                    m.vertical = 0.;
                    m.state.health = 25.;
                    m.state.dead = lab.medical_case == 2;
                    m.state.incap = false;
                }
                state.slot = 4;
                state.defibrillator = lab.medical_case == 2;
                state.medkit = lab.medical_case == 3;
                if capture {
                    if lab.medical_case == 2 {
                        input.fire_held = true;
                    } else {
                        input.shove = true;
                        input.shove_held = true;
                    }
                }
                lab.note="Teammate positioned in front: hold left mouse with defibrillator, or right mouse with medkit.".into();
            }
            4 => {
                state.revives = 2;
                state.black_white = true;
                state.health = 30.;
                lab.note = "Third strike: black-and-white vision; a medkit resets it.".into();
            }
            5 => {
                state.medkit = true;
                state.defibrillator = false;
                crate::loot::drop_item(
                    &mut commands,
                    Some(&models),
                    Some(&meshes),
                    &rules,
                    &collision,
                    "weapon_defibrillator",
                    body.feet
                        + (direction * Vec3::new(1., 0., 1.)).normalize_or_zero() * 1.2
                        + Vec3::Y * 0.08,
                    None,
                );
                input.use_item = true;
                lab.note =
                    "Inventory swap: the old medkit should drop visibly and remain collectible."
                        .into();
            }
            6 => {
                if let Some(w) = rules.weapons.get("weapon_rifle_desert") {
                    state.primary = Some(crate::game_state::Gun {
                        laser: false,
                        ammo_upgrade: None,
                        upgraded_rounds: 0,
                        class: w.class.clone(),
                        ammo: w.clip,
                        reserve: w.reserve_capacity(),
                    });
                    state.slot = 1;
                    input.fire = true;
                }
            }
            7 => {
                body.feet = Vec3::from_array(binary::source_position([256., 1900., 4.]));
                body.yaw = std::f32::consts::FRAC_PI_2;
                body.pitch = 0.;
                state.slot = 2;
                input.fire = true;
            }
            14 => {
                body.feet = Vec3::from_array(binary::source_position([1500., 800., 4.]));
                body.yaw = -std::f32::consts::FRAC_PI_2;
                body.pitch = 0.;
                if let Some(bot) = squad.0.first_mut() {
                    bot.position = body.feet + Vec3::X * 2.;
                    bot.vertical = 0.;
                    bot.state.health = 25.;
                    bot.state.dead = false;
                    bot.state.incap = false;
                    bot.state.medkit = true;
                    bot.state.pills = false;
                    bot.state.adrenaline = false;
                    bot.state.defibrillator = false;
                    bot.state.medical_kind = 1;
                    bot.state.action = 5.;
                    bot.state.action_total = 5.;
                }
                lab.note="Bot medical prop: native medkit should replace the pistol during healing, then pistol returns.".into();
            }
            8 | 9 | 10..=13 | 15 => {
                let live = std::env::args().any(|a| a == "--hotel-live-kill-stress");
                let hotel = live || std::env::args().any(|a| a == "--hotel-corpse-stress");
                if let Some(audit) = hit_audit.as_mut() {
                    audit.groups.clear();
                }
                if !hotel {
                    body.feet = Vec3::from_array(binary::source_position([1500., 800., 4.]));
                    body.yaw = -std::f32::consts::FRAC_PI_2;
                    body.pitch = 0.;
                }
                state.slot = if state.primary.is_some() { 1 } else { 2 };
                let melee = rules
                    .weapons
                    .get(&state.gun().class)
                    .is_some_and(|w| w.melee());
                let capture_args = std::env::args().collect::<Vec<_>>();
                let number = |key: &str, default: f32| capture_args.windows(2)
                    .find(|v| v[0] == key).and_then(|v| v[1].parse::<f32>().ok())
                    .filter(|v| v.is_finite()).unwrap_or(default);
                let point = Vec3::from_array(binary::source_position([
                    1500. + number("--capture-gore-distance", if melee { 60. } else { 240. }),
                    800.,
                    if lab.medical_case == 15 { 110. } else { 4. },
                ]));
                let target_model = capture_gore_model();
                if let Some(handles) =
                    models
                        .0
                        .get(&(target_model.clone(), 0, 0))
                {
                    let stress = hotel || std::env::args().any(|a| a == "--lab-corpse-stress");
                    let count = if stress {
                        std::env::args()
                            .collect::<Vec<_>>()
                            .windows(2)
                            .find(|a| a[0] == "--lab-corpse-count")
                            .and_then(|a| a[1].parse::<usize>().ok())
                            .unwrap_or(24)
                            .clamp(1, 48)
                    } else {
                        1
                    };
                    let validated = (hotel
                        && (live || std::env::args().any(|a| a == "--hotel-valid-corpse-spawns")))
                    .then(|| {
                        validated_corpse_positions(&collision, body.feet, count + usize::from(live))
                    });
                    if let Some(points) = &validated {
                        println!(
                            "Hotel valid corpse fixture: requested={count} placed={}",
                            points.len()
                        );
                    }
                    if live {
                        if std::env::args().any(|a| a == "--lab-empty-gun") {
                            state.gun_mut().ammo = 0;
                        }
                        if let Some(points) = &validated {
                            if let Some(point) = points.first() {
                                body.feet = *point;
                                body.velocity = Vec3::ZERO;
                            }
                        }
                    }
                    for i in 0..count {
                        let point = if hotel {
                            if let Some(points) = &validated {
                                let Some(point) = points.get(i + usize::from(live)) else {
                                    continue;
                                };
                                *point
                            } else {
                                body.feet
                                    + Vec3::X * ((i % 4) as f32 - 1.5) * 0.35
                                    + Vec3::Z * (1.2 + (i / 4) as f32 * 0.35)
                            }
                        } else if stress {
                            Vec3::from_array(binary::source_position([
                                1740. + (i % 4) as f32 * 65.,
                                800. + (i / 4) as f32 * 65.,
                                4.,
                            ]))
                        } else {
                            point
                        };
                        let root = commands
                            .spawn((
                                Transform::from_translation(point).with_rotation(Quat::from_rotation_y(
                                    number("--capture-gore-yaw", 0.).to_radians())),
                                Visibility::Inherited,
                                ImpactTarget,
                                Enemy {
                                    model: target_model.clone(),
                                    death_loot: None,
                                    loot_dropped: false,
                                    reaction_serial: 0,
                                    attack_serial: 0,
                                    health: if live {
                                        1.
                                    } else if lab.medical_case == 15 {
                                        0.
                                    } else if lab.medical_case == 9 {
                                        1.
                                    } else {
                                        300.
                                    },
                                    attack: 0.,
                                    reaction: 0,
                                    corpse_time: if lab.medical_case == 15 { 12. } else { 0. },
                                    death: crate::infected_death::Context {
                                        cause: if lab.medical_case == 15 && !stress {
                                            crate::infected_death::Cause::Explosion
                                        } else {
                                            crate::infected_death::Cause::Bullet
                                        },
                                        head: stress && i % 3 == 0,
                                        running: stress && i % 2 == 0,
                                        incoming: Vec3::X,
                                        ..Default::default()
                                    },
                                    movement_speed: 0.,
                                    burning: 0.,
                                    vertical: 0.,
                                },
                            ))
                            .id();
                        for (mesh, material) in handles {
                            commands.spawn((
                                Mesh3d(mesh.clone()),
                                MeshMaterial3d(material.clone()),
                                Transform::default(),
                                crate::viewer::DynamicModelLighting::default(),
                                ChildOf(root),
                            ));
                        }
                    }
                }
                lab.blood_shots = if capture_args.iter().any(|a| a == "--capture-gore-alive") {
                    0
                } else if live {
                    96
                } else if lab.medical_case == 15 {
                    0
                } else if lab.medical_case == 8 {
                    3
                } else {
                    1
                };
                lab.blood_shot_at = 2.7;
                lab.note =
                    "Native infected blood burst and wall splatter; repeated hit reactions.".into();
            }
            _ => {}
        }
        lab.medical_audit = capture;
    }
    if lab.blood_shots > 0
        && lab.clock > 2.7
        && std::env::args().any(|a| a == "--hotel-live-kill-stress")
        && !targets.iter().any(|(_, _, e)| e.health > 0.)
    {
        println!(
            "Hotel live kill fixture complete: kills={} elapsed={} ammo={} shots={}",
            state.kills,
            lab.clock,
            state.gun().ammo,
            state.shot_serial
        );
        lab.blood_shots = 0;
    }
    if lab.blood_shots > 0 && lab.clock >= lab.blood_shot_at {
        let live = std::env::args().any(|a| a == "--hotel-live-kill-stress");
        if live && state.gun().ammo == 0 {
            // Exercise normal empty-gun automatic reload, without injecting R.
            lab.blood_shot_at += 0.1;
        } else {
            if (10..=13).contains(&lab.medical_case) {
                if let Some((entity, transform, _)) = targets.iter().find(|(_, _, e)| e.health > 0.)
                {
                    let point = if lab.medical_case == 13 {
                        let eye = body.feet + Vec3::Y * crate::collision::EYE;
                        let boxes = library.hitboxes(
                            "models/infected/common_male01.mdl",
                            entity,
                            transform,
                            playback.as_deref(),
                        );
                        [
                            Vec3::new(0., 1.78, 0.34),
                            Vec3::new(0., 1.78, -0.34),
                            Vec3::Y * 0.3,
                        ]
                        .into_iter()
                        .map(|p| transform.translation + p)
                        .find(|point| {
                            boxes.as_ref().is_some_and(|b| {
                                b.trace(eye, (*point - eye).normalize(), 10.).is_none()
                            })
                        })
                    } else {
                        library.hitbox_target(
                            "models/infected/common_male01.mdl",
                            entity,
                            transform,
                            playback.as_deref(),
                            match lab.medical_case {
                                10 => 1,
                                11 => 2,
                                _ => 6,
                            },
                        )
                    };
                    if let Some(point) = point {
                        if live {
                            let eye = body.feet + Vec3::Y * crate::collision::EYE;
                            if collision.sight_trace(eye, point).fraction < 0.99 {
                                if let Some((_, position, _)) = targets.iter().find(|(_, t, _)| {
                                    t.translation.distance_squared(transform.translation)
                                        > 0.5f32.powi(2)
                                        && collision.clear_hull(t.translation, HEIGHT)
                                        && collision
                                            .sight_trace(
                                                t.translation + Vec3::Y * crate::collision::EYE,
                                                point,
                                            )
                                            .fraction
                                            >= 0.99
                                }) {
                                    body.feet = position.translation;
                                    body.velocity = Vec3::ZERO;
                                }
                            }
                        }
                        let direction = (point - body.feet - Vec3::Y * crate::collision::EYE)
                            .normalize_or_zero();
                        body.yaw = (-direction.x).atan2(-direction.z);
                        body.pitch = direction.y.asin();
                    }
                }
            }
            input.fire = true;
            lab.blood_shots -= 1;
            lab.blood_shot_at += 0.3;
        }
    }
    if lab.medical_audit && lab.clock > 5.7 {
        println!(
            "Lab medical result: case={} health={} temporary={} adrenaline={} defib={} held={} ammo={} use_serial={} shots={} bots={:?}",
            lab.medical_case,
            state.health,
            state.temporary,
            state.adrenaline_time,
            state.defibrillator,
            state.held_class(),
            state.gun().ammo,
            state.item_use_serial,
            state.shot_serial,
            squad
                .0
                .iter()
                .map(|m| (m.state.dead, m.state.health))
                .collect::<Vec<_>>()
        );
        if let Some(impacts) = &impacts {
            println!(
                "Impact counters: bullets={} blood_bursts={} blood_marks={}",
                impacts.bullet_marks, impacts.blood_bursts, impacts.blood_marks
            );
        }
        if (10..=13).contains(&lab.medical_case) {
            println!(
                "Lab native hitboxes: case={} groups={:?} health={:?}",
                lab.medical_case,
                hit_audit.as_ref().map(|a| &a.groups),
                targets.iter().map(|(_, _, e)| e.health).collect::<Vec<_>>()
            );
        }
        if lab.medical_case == 14 {
            println!(
                "Lab bot held items: {:?}",
                bot_items.iter().map(|i| &i.0).collect::<Vec<_>>()
            );
        }
        lab.medical_audit = false;
    }
    if keys.just_pressed(KeyCode::F12) {
        let p = Vec3::from_array(binary::source_position([3820., 2750., 4.]));
        body.feet = p;
        body.velocity = Vec3::ZERO;
        for (i, m) in squad.0.iter_mut().enumerate() {
            m.position = p + Vec3::X * ((i as f32 - 1.) * 0.4);
            m.vertical = 0.;
            if m.state.incap {
                m.state.revive();
            }
        }
        lab.note="Checkpoint test: everyone gathered; close lab_checkpoint to load Hotel in this window.".into();
    }
    if keys.just_pressed(KeyCode::F10) {
        let kinds = ["weapon_molotov", "weapon_pipe_bomb", "weapon_vomitjar"];
        let next = state
            .throwable
            .as_ref()
            .and_then(|c| kinds.iter().position(|k| *k == c))
            .map_or(0, |i| (i + 1) % 3);
        state.throwable = Some(kinds[next].into());
        state.throw_stage = 0;
        state.throw_class = None;
        state.slot = 3;
        lab.note="Throwable supplied: hold left mouse to prepare, release to throw; 3 selects owned throwable.".into();
    }
    if keys.just_pressed(KeyCode::F6) || lab.capture_relay && lab.clock > 0.8 {
        lab.capture_relay = false;
        if let Some(id) = logic.world.find("lab_relay").first().copied() {
            logic.world.input(id, "Trigger", "", None, None);
        }
        lab.note="Relay requested through native entity I/O. Observe door, sound, fire and delayed stop.".into();
    }
    if keys.just_pressed(KeyCode::F9) {
        lab.spawned = 0;
        lab.director_enabled = false;
        frontend.restart = true;
        lab.clock = 0.;
        lab.route_audit_at = 0.;
        lab.note = "Map reset requested; restores pickups, doors, breakables and health.".into();
    }
    if keys.just_pressed(KeyCode::F8) {
        let name = lab.names.get(lab.sequence).cloned().unwrap_or_default();
        let line = format!(
            "{}\t{}\t{}\t{:.2}\t{}\n",
            MODELS[lab.model],
            lab.sequence,
            name,
            state.health,
            lab.note.replace(['\t', '\n'], " ")
        );
        let result = std::fs::create_dir_all("local-research").and_then(|_| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("local-research/lab-observations.tsv")?
                .write_all(line.as_bytes())
        });
        lab.note = match result {
            Ok(()) => "Observation snapshot saved; this does not mark the feature passed.".into(),
            Err(e) => format!("Report: {e}"),
        };
    }
    if lab.names.is_empty() {
        if let Some(root) = lab.preview.take() {
            commands.entity(root).despawn();
        }
        let path = MODELS[lab.model];
        lab.names = library.sequence_names(path);
        lab.sequence = if lab.sequence == usize::MAX {
            library.default_preview_sequence(path)
        } else {
            lab.sequence.min(lab.names.len().saturating_sub(1))
        };
        let root = commands
            .spawn((
                Transform::from_translation(Vec3::from_array(binary::source_position([
                    2200., 2400., 0.,
                ])))
                .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
                Visibility::Inherited,
            ))
            .id();
        lab.preview = Some(root);
        let mut paths = vec![path];
        if lab.model > 0 && lab.model < 5 {
            paths.push(&rules.pistol.player_model);
        }
        for path in paths {
            if let Some(handles) = models.0.get(&(path.into(), 0, 0)) {
                for (index, (mesh, material)) in handles.iter().enumerate() {
                    if let Some(original) = meshes.get(mesh).cloned() {
                        let mesh = meshes.add(original);
                        commands.spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(material.clone()),
                            Transform::default(),
                            crate::viewer::DynamicModelLighting::default(),
                            NoFrustumCulling,
                            RenderLayers::layer(1),
                            StageDraw {
                                model: path.into(),
                                index,
                            },
                            ChildOf(root),
                        ));
                    }
                }
            }
        }
    }
    lab.clock += time.delta_secs().min(0.1);
    for (draw, mesh) in &draws {
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            library.preview(
                MODELS[lab.model],
                lab.sequence,
                lab.clock,
                &draw.model,
                draw.index,
                &mut mesh,
            );
        }
    }
    lab.station = STATIONS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let distance = |p: [f32; 3]| {
                Vec3::from_array(binary::source_position(p)).distance_squared(body.feet)
            };
            distance(a.1).total_cmp(&distance(b.1))
        })
        .map_or(0, |(i, _)| i);
    let name = lab
        .names
        .get(lab.sequence)
        .map_or("unavailable", String::as_str);
    let text = format!(
        "REWRITE LAB | {}\nF3 station | F1 stage model | F2 / Shift+F2 sequence\n{} | sequence {} / {}: {}\nU uncommons / Alt+U fallen / Shift+U Jimmy | M mud screen | F4 combat | F5 medical | F6 relay | F7 Director | F8 snapshot | F9 reset | F10 throwable | F11 medical | F12 checkpoint\n{}\nKnown gaps: AI special spawning, full animation layers/IK, exact VPhysics, Director engine bindings/EMS, multiplayer and finales.",
        STATIONS[lab.station].0,
        MODELS[lab.model].trim_start_matches("models/"),
        lab.sequence + 1,
        lab.names.len(),
        name,
        lab.note
    );
    if panel.0.0 != text {
        panel.0.0 = text;
    }
}

/// Check authored fixture identities and BSP ownership, rather than an old
/// total model count that changes whenever a new station is added.
fn verify_fixture_models(bsp:&l4d2_formats::bsp::Bsp)->io::Result<usize>{
    if bsp.models.is_empty()||bsp.models[0].num_faces<=0{return Err(binary::invalid("Lab world geometry missing"));}
    let mut referenced=std::collections::BTreeSet::from([0usize]);
    for entity in &bsp.entities {
        if let Some(model)=entity.get("model").and_then(|m|m.strip_prefix('*')){
            let id=model.parse::<usize>().map_err(|_|binary::invalid("Lab brush model reference is not an index"))?;
            if id==0||id>=bsp.models.len(){return Err(binary::invalid(format!("Lab brush model *{id} is outside the entity model range")));}
            referenced.insert(id);
        }
    }
    if referenced.len()!=bsp.models.len(){return Err(binary::invalid("Lab contains an unowned brush model"));}
    for (id,model) in bsp.models.iter().enumerate(){
        if !model.mins.is_finite()||!model.maxs.is_finite()||!model.origin.is_finite()||(model.maxs-model.mins).min_element()<=0.{return Err(binary::invalid(format!("Lab model *{id} has invalid bounds")));}
        let valid_head=if model.head_node>=0{(model.head_node as usize)<bsp.nodes.len()}else{(-1i64-model.head_node as i64)<bsp.leafs.len() as i64};
        if !valid_head{return Err(binary::invalid(format!("Lab model *{id} has no collision-tree root")));}
        if model.first_face<0||model.num_faces<0||model.first_face as usize+model.num_faces as usize>bsp.faces.len(){return Err(binary::invalid(format!("Lab model *{id} has an invalid face range")));}
    }
    let required=[("lab_ladder_team_0","func_simpleladder",Some(0)),("lab_ladder_team_1","func_simpleladder",Some(1)),("lab_ladder_team_2","func_simpleladder",Some(2)),
        ("lab_panic_button","func_button",None),("lab_glass","func_breakable",None),("lab_wood","func_breakable",None),
        ("lab_trigger","trigger_multiple",None),("lab_hurt","trigger_hurt",None),("lab_once","trigger_once",None),("lab_exit","info_changelevel",None)];
    let mut fixtures=std::collections::BTreeSet::new();
    for (name,class,team) in required {
        let entries=bsp.entities.iter().filter(|e|e.get("targetname")==Some(name)).collect::<Vec<_>>();
        if entries.len()!=1{return Err(binary::invalid(format!("Lab fixture {name} must have exactly one entity")));}
        let e=entries[0];let model=e.get("model").and_then(|m|m.strip_prefix('*')).and_then(|m|m.parse::<usize>().ok());
        if e.classname()!=class||model.is_none()||!fixtures.insert(model.unwrap()){return Err(binary::invalid(format!("Lab fixture {name} has a missing, shared or incorrect brush binding")));}
        if let Some(team)=team{if e.get("team").and_then(|v|v.parse::<i32>().ok())!=Some(team){return Err(binary::invalid(format!("Lab ladder {name} has the wrong team")));}}
    }
    Ok(bsp.models.len())
}

pub fn verify(root: &Path) -> io::Result<()> {
    let path = path();
    let bytes = crate::bsp::load_bytes(&path, root)?;
    let bsp = l4d2_formats::bsp::Bsp::parse(bytes).map_err(|e| binary::invalid(e.to_string()))?;
    let model_count=verify_fixture_models(&bsp)?;
    let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true)?;
    let mut collision = Collision::load(&path)?;
    collision.enable_l4d2r(root, &logic)?;
    let mut rows = format!("check\tresult\tdetail\nbrush fixtures\tpassed\t{model_count} owned models; required ladder/button/breakable/trigger fixtures present\n");
    for (name, p) in STATIONS {
        collision.supported_spawn_near(Vec3::from_array(binary::source_position(p)), HEIGHT)?;
        rows += &format!("spawn {name}\tpassed\tsupported native hull\n");
    }
    logic.verify_triggers(&collision)?;
    rows += "trigger geometry\tpassed\tall test brush volumes\n";
    let nav = l4d2_formats::nav::parse_nav(&std::fs::read(path.with_extension("nav"))?)
        .map_err(|e| binary::invalid(e.to_string()))?;
    let graph = crate::l4d2r_navgraph::NavGraph::new(std::sync::Arc::new(nav));
    if graph
        .path(
            l4d2r_math::Vec3::new(2600., 2330., 0.),
            l4d2r_math::Vec3::new(3120., 2330., 144.),
        )
        .is_none()
    {
        return Err(binary::invalid("Lab stairs have no NAV route"));
    }
    rows += "stair NAV route\tpassed\tconnected base to raised landing\n";
    let target = Vec3::from_array(binary::source_position([3120., 2330., 148.]));
    let mut position = collision.supported_spawn_near(
        Vec3::from_array(binary::source_position([2600., 2330., 4.])),
        HEIGHT,
    )?;
    let mut navigation = crate::l4d2r_navigation::Navigation::load(&path);
    let mut vertical = 0.;
    for _ in 0..1500 {
        let grounded = collision.grounded(position, HEIGHT);
        vertical = if grounded {
            0.
        } else {
            vertical - 800. * UNIT / 60.
        };
        let direction = (navigation.toward(0, position, target, 1. / 60.) * Vec3::new(1., 0., 1.))
            .normalize_or_zero();
        position = collision.walk(
            position,
            (direction * 3. + Vec3::Y * vertical) / 60.,
            HEIGHT,
            grounded,
        );
        if position.distance(target) < 0.7 {
            break;
        }
    }
    if position.distance(target) > 0.7 {
        return Err(binary::invalid(format!(
            "Lab stair hull traversal stuck at {position:?}"
        )));
    }
    rows += "stair hull traversal\tpassed\treal Navigation::toward and Collision::walk reached landing\n";
    let relay = *logic
        .world
        .find("lab_relay")
        .first()
        .ok_or_else(|| binary::invalid("Lab relay missing"))?;
    if logic.world.ents[relay].conns.len() != 7 {
        return Err(binary::invalid("Lab relay outputs incomplete"));
    }
    logic.world.input(relay, "Trigger", "", None, None);
    logic.world.tick(0.1);
    let fire = logic.world.find("lab_fire")[0];
    let particle = logic.world.find("lab_particles")[0];
    if !matches!(
        logic.world.ents[fire].kind,
        l4d2r_game_core::entities::Kind::Fire { active: true, .. }
    ) || !matches!(
        logic.world.ents[particle].kind,
        l4d2r_game_core::entities::Kind::Particle { active: true, .. }
    ) {
        return Err(binary::invalid("Relay did not activate fire / particles"));
    }
    for _ in 0..360 {
        logic.world.tick(1. / 60.);
    }
    if !matches!(
        logic.world.ents[fire].kind,
        l4d2r_game_core::entities::Kind::Fire { active: false, .. }
    ) || !matches!(
        logic.world.ents[particle].kind,
        l4d2r_game_core::entities::Kind::Particle { active: false, .. }
    ) {
        return Err(binary::invalid(
            "Delayed relay did not stop fire / particles",
        ));
    }
    rows += "relay outputs\tpassed\tfire and particles started then stopped by delayed I/O\n";
    let door = logic.world.find("lab_door")[0];
    let open = logic.world.world_matrix(door);
    logic.world.input(door, "Close", "", None, None);
    for _ in 0..120 {
        logic.world.tick(1. / 60.);
    }
    if open == logic.world.world_matrix(door) {
        return Err(binary::invalid("Door did not rotate / close"));
    }
    rows += "door rotation\tpassed\topen and closed transforms differ\n";
    for name in ["lab_glass", "lab_wood", "lab_door"] {
        let id = logic.world.find(name)[0];
        if !logic.world.damage_breakable(id, 10000.) || logic.world.ents[id].alive {
            return Err(binary::invalid(format!("Lab destruction failed: {name}")));
        }
        rows += &format!("destruction {name}\tpassed\tnative entity damage / alive state\n");
    }
    rows += "manual / unsupported features\tpending\tsee TEST_MAP.md; no blanket parity claim\n";
    std::fs::create_dir_all("local-research")?;
    std::fs::write("local-research/lab-verification.tsv", rows)?;
    println!(
        "Lab checks passed; manual coverage remains pending. See local-research/lab-verification.tsv"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_lab_has_complete_relay_connections_and_connected_stair_navigation() {
        let bsp = std::sync::Arc::new(
            l4d2_formats::bsp::Bsp::parse(std::fs::read(path()).unwrap()).unwrap(),
        );
        assert_eq!(verify_fixture_models(&bsp).unwrap(),bsp.models.len());
        let world = l4d2r_game_core::entities::EntityWorld::new(
            bsp.clone(),
            l4d2r_game_core::mapents::collect(&bsp),
            1,
        );
        assert_eq!(world.ents[world.find("lab_relay")[0]].conns.len(), 7);
        let nav =
            l4d2_formats::nav::parse_nav(&std::fs::read(path().with_extension("nav")).unwrap())
                .unwrap();
        assert!(nav.areas.len() > 250);
        let graph = crate::l4d2r_navgraph::NavGraph::new(std::sync::Arc::new(nav));
        assert!(
            graph
                .path(
                    l4d2r_math::Vec3::new(2600., 2330., 0.),
                    l4d2r_math::Vec3::new(3120., 2330., 144.)
                )
                .is_some()
        );
    }
    #[test]
    fn lab_fixture_gate_accepts_extensions_and_rejects_missing_or_misbound_fixtures(){
        use l4d2_formats::bsp::{Bsp,Entity};
        let bytes=std::fs::read(path()).unwrap();
        let fresh=||Bsp::parse(bytes.clone()).unwrap();
        let mut extended=fresh();let model=extended.models[5];let id=extended.models.len();extended.models.push(model);
        extended.entities.push(Entity(vec![("classname".into(),"func_breakable".into()),("targetname".into(),"lab_future_brush".into()),("model".into(),format!("*{id}"))]));
        assert_eq!(verify_fixture_models(&extended).unwrap(),extended.models.len());
        let mut missing=fresh();missing.entities.retain(|e|e.get("targetname")!=Some("lab_ladder_team_2"));assert!(verify_fixture_models(&missing).is_err());
        let mut wrong_team=fresh();let e=wrong_team.entities.iter_mut().find(|e|e.get("targetname")==Some("lab_ladder_team_2")).unwrap();e.0.iter_mut().find(|(k,_)|k=="team").unwrap().1="1".into();assert!(verify_fixture_models(&wrong_team).is_err());
        let mut dangling=fresh();let e=dangling.entities.iter_mut().find(|e|e.get("targetname")==Some("lab_panic_button")).unwrap();e.0.iter_mut().find(|(k,_)|k=="model").unwrap().1="*99999".into();assert!(verify_fixture_models(&dangling).is_err());
        let mut orphan=fresh();orphan.models.push(orphan.models[5]);assert!(verify_fixture_models(&orphan).is_err());
        let mut bad_root=fresh();bad_root.models[3].head_node=i32::MAX;assert!(verify_fixture_models(&bad_root).is_err());
        let mut no_volume=fresh();no_volume.models[3].maxs=no_volume.models[3].mins;assert!(verify_fixture_models(&no_volume).is_err());
    }

}
