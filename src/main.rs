mod adrenaline;
mod low_violence;
mod bullet_penetration;
mod prop_impact;
mod scavenge_rules;
mod color_correction;
mod engine_post;
mod ui_presentation;
mod articulated;
mod ivp_joint;
mod ivp_joint_callback;
mod ivp_joint_mixed;
mod ivp_joint_tail;
mod ivp_joint_apply;
mod ivp_impulse_wake;
mod ivp_constraint_order;
mod ivp_group_members;
mod ivp_registered_groups;
mod ivp_registered_schedule;
mod ivp_group_effects;
mod ivp_group_complete;
mod ivp_joint_linear;
mod ivp_joint_frame;
mod ivp_hinge_constructor;
mod ivp_ragdoll_constructor;
mod ivp_sphere_constructor;
mod ivp_poly_constructor;
mod ivp_public_velocity;
mod ivp_joint_angular;
mod ivp_joint_group;
mod ivp_hinge;
mod ivp_hinge_rows;
mod ivp_hinge_prepare;
mod ivp_hinge_motor;
mod ivp_hinge_angle;
mod ivp_hinge_callback;
mod ivp_fixed_callback;
mod ivp_impact;
mod ivp_impact_state;
mod ivp_impact_wave;
mod ivp_contact;
mod ivp_energy;
mod ivp_history;
mod ivp_single;
mod ivp_contact_pipeline;
mod ivp_dissipation;
mod ivp_unit;
mod ivp_unit_prepare;
mod ivp_joint_angle;
mod ivp_registered_physical;
mod ivp_registered_contacts;
mod ivp_runtime_scene;
mod ivp_playtest;
mod ivp_unit_registry;
mod ivp_contact_notifications;
#[cfg(test)]
mod ivp_contact_notifications_tests;
mod ivp_contact_system;
#[cfg(test)]
mod ivp_contact_split_pipeline_tests;
#[cfg(test)]
mod ivp_contact_system_split_tests;
#[cfg(test)]
mod ivp_contact_unit_graph_tests;
#[cfg(test)]
mod ivp_contact_graph_pipeline_tests;
#[cfg(test)]
mod ivp_staged_dispatch_tests;
#[cfg(test)]
mod ivp_physical_contact_bank_tests;
mod ivp_managed_contacts;
mod ivp_managed_response;
mod ivp_batch_normal;
#[cfg(test)]
mod ivp_batch_pipeline_tests;
#[cfg(test)]
mod ivp_batch_normal_tests;
#[cfg(test)]
mod ivp_managed_response_tests;
#[cfg(test)]
mod ivp_managed_contacts_tests;
#[cfg(test)]
mod ivp_contact_system_tests;
#[cfg(test)]
mod ivp_contact_registry_tests;
#[cfg(test)]
mod ivp_registered_reset;
mod ivp_movement;
mod ivp_lifetime;
mod ivp_friction;
mod ivp_slip;
mod ivp_redistribute;
mod ivp_create;
mod ivp_refresh;
mod ivp_active;
mod ivp_motion;
mod ivp_bounds;
mod ivp_hull_budget;
mod ivp_hull_policy;
mod ivp_hull_manager;
mod ivp_event_queue;
mod ivp_event_world;
mod ivp_broadphase;
mod ivp_spatial_tree;
mod ivp_trial_bounds;
mod ivp_minimizer;
mod ivp_compact_tree;
mod ivp_surface_admission;
mod ivp_distance_schedule;
mod ivp_sphere_schedule;
mod ivp_sphere_owner;
mod ivp_mindist_graph;
mod ivp_far_distance;
mod ivp_feature_handler;
mod ivp_cache_pool;
mod ivp_world_cache;
#[cfg(test)]
mod ivp_world_cache_tests;
#[cfg(test)]
mod ivp_cache_lifecycle_tests;
mod ivp_material;
mod ivp_mass_bound;
mod ivp_material_backend;
mod ivp_surface_registry;
mod ivp_time;
mod ivp_damping;
mod ivp_base_controller;
mod ballistics;
mod binary;
mod bot_recovery;
mod bot_retreat;
mod bsp;
mod bullet_impacts;
mod campaign_flow;
mod campaign_settings;
mod carryables;
mod collision;
mod game;
mod game_audio;
mod game_console;
mod game_chat;
mod multiplayer;
mod net_play;
mod net_flow;
mod survival;
mod vomit_post;
mod net_infected;
mod steam;
mod net;
mod game_loading;
mod game_menu;
mod game_state;
mod game_ui;
mod glow_menu;
mod glow_settings;
mod gpu_writes;
mod graphics_options;
mod gun_effects;
mod precipitation;
mod dev_console;
mod source_commands;
mod runtime_props;
mod world_editor;
mod hotel_live_test;
mod infected_death;
mod infected_appearance;
mod infected_gore;
mod infected_hud;
mod ai_ladder;
mod light_styles;
mod ropes;
mod env_sprites;
mod detail_props;
mod room_dsp;
mod soundscapes;
mod model_shadows;
mod bile_screen;
mod infected_movement;
mod infected_population;
mod kv;
mod l4d2r_animation;
mod l4d2r_cinematic;
mod l4d2r_navgraph;
mod l4d2r_navigation;
mod survivor_flashlight;
mod stack_sampler;
mod skate_mod;

/// mimalloc instead of the Windows process heap: after an in-window chapter
/// change frees a large map, the fragmented system heap made every later
/// allocation slow (c1m1 -> c1m2 render thread ~38% in RtlAllocateHeap,
/// 21% in RtlFreeHeap, versus 4%/3% on a direct load).
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
/// mimalloc plus a live-byte counter (allocated minus freed). Memory logs
/// print it next to the process footprint, which separates objects the game
/// still holds from memory the allocator or driver keeps after a free.
struct CountingAllocator;
static LIVE_HEAP: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
pub(crate) fn live_heap_bytes() -> isize {
    LIVE_HEAP.load(std::sync::atomic::Ordering::Relaxed)
}
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !p.is_null() {
            LIVE_HEAP.fetch_add(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
            heap_audit::track(p, layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE_HEAP.fetch_add(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
            heap_audit::track(p, layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        heap_audit::untrack(ptr, layout.size());
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) };
        LIVE_HEAP.fetch_sub(layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE_HEAP.fetch_add(new_size as isize - layout.size() as isize, std::sync::atomic::Ordering::Relaxed);
            heap_audit::untrack(ptr, layout.size());
            heap_audit::track(p, new_size);
        }
        p
    }
}
/// `L4D2R_HEAP_AUDIT=1` (PC): remembers where every live allocation of
/// 256 KiB or more was made, so a memory checkpoint can list what a released
/// map left behind (`dump`).
pub(crate) mod heap_audit {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    pub(crate) static ENABLED: AtomicBool = AtomicBool::new(false);
    const MIN: usize = 256 << 10;
    thread_local! { static BUSY: Cell<bool> = const { Cell::new(false) }; }
    type Table = std::collections::HashMap<usize, (usize, std::backtrace::Backtrace)>;
    static LIVE: std::sync::Mutex<Option<Table>> = std::sync::Mutex::new(None);
    fn guarded(f: impl FnOnce()) {
        let entered = BUSY.try_with(|b| !b.replace(true)).unwrap_or(false);
        if !entered {
            return;
        }
        f();
        let _ = BUSY.try_with(|b| b.set(false));
    }
    pub(crate) fn track(p: *mut u8, size: usize) {
        if size < MIN || !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        guarded(|| {
            let trace = std::backtrace::Backtrace::force_capture();
            if let Ok(mut live) = LIVE.lock() {
                live.get_or_insert_with(Default::default).insert(p as usize, (size, trace));
            }
        });
    }
    pub(crate) fn untrack(p: *mut u8, size: usize) {
        if size < MIN || !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        guarded(|| {
            if let Ok(mut live) = LIVE.lock() {
                if let Some(live) = live.as_mut() {
                    live.remove(&(p as usize));
                }
            }
        });
    }
    /// Live tracked bytes grouped by the first game frames of their stack.
    pub(crate) fn dump(stage: &str) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        guarded(|| {
            let entries: Vec<(usize, String)> = {
                let Ok(live) = LIVE.lock() else { return };
                let Some(live) = live.as_ref() else { return };
                live.values().map(|(size, trace)| (*size, format!("{trace}"))).collect()
            };
            if let Some((_, text)) = entries.iter().max_by_key(|(size, _)| *size) {
                for line in text.lines().take(60) {
                    eprintln!("HEAP AUDIT {stage} raw: {line}");
                }
            }
            let mut groups: std::collections::HashMap<String, (usize, usize)> = Default::default();
            let mut total = 0;
            for (size, text) in entries {
                total += size;
                let frames: Vec<&str> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.contains("l4d2_rewrite::") && !l.contains("heap_audit") && !l.contains("CountingAllocator"))
                    .map(|l| l.split_once(": ").map_or(l, |(_, f)| f))
                    .take(4)
                    .collect();
                let key = if frames.is_empty() { "(no game frame)".to_string() } else { frames.join(" <- ") };
                let e = groups.entry(key).or_default();
                e.0 += size;
                e.1 += 1;
            }
            let mut rows: Vec<_> = groups.into_iter().collect();
            rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            eprintln!("HEAP AUDIT {stage}: {:.1} MiB in tracked allocations >= 256 KiB", total as f64 / 1048576.);
            for (key, (bytes, n)) in rows.iter().take(25) {
                eprintln!("HEAP AUDIT {stage}: {:8.1} MiB x{n:5} {}", *bytes as f64 / 1048576., key.chars().take(400).collect::<String>());
            }
        });
    }
}
mod bot_look;
mod lighting;
mod ladders;
mod loot;
mod map_modes;
mod map_transition;
mod mass_properties;
mod material;
mod source_water;
mod source_sun;
mod menu_video;
mod muzzle_fixture;
mod native_fog;
mod native_movement_audio;
mod native_tonemap;
mod parity;
mod particle_pull;
mod particle_sprite;
mod performance;
mod physics_data;
mod player_movement;
mod fall_damage;
mod ledge_hang;
mod survivor_music;
mod progress_bar;
mod projectiles;
mod prop_physics;
mod ragdoll;
mod render_warmup;
mod rigid_contact;
mod scene;
mod scoreboard;
mod bot_combat;
mod source_assets;
mod source_breakables;
mod source_break_debris;
mod source_break_surfaces;
mod source_dialogue;
mod source_director;
mod source_director_flow;
mod source_ice;
mod source_noise;
mod source_particles;
mod source_pcf;
mod source_script_options;
mod source_sound;
mod source_vm;
mod special_infected;
mod static_geometry;
mod studio;
mod studio_hitboxes;
mod studio_ik;
mod survivor_glow;
mod survivor_names;
mod survivor_items;
mod test_lab;
mod uncommon_behaviour;
mod upgrades;
mod vfont;
mod viewer;
mod visibility;
mod vpk;
mod vtf;

use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

#[derive(Component)]
struct Player;
#[derive(Component)]
struct Infected {
    health: f32,
    attack: f32,
}
#[derive(Component)]
struct Hud;
#[derive(Resource)]
struct Session {
    health: f32,
    ammo: u32,
    reserve: u32,
    reload: f32,
    cooldown: f32,
    kills: u32,
    wave: u32,
    spawn: f32,
    yaw: f32,
    pitch: f32,
    vertical: f32,
    paused: bool,
    won: bool,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            health: 100.,
            ammo: 15,
            reserve: 180,
            reload: 0.,
            cooldown: 0.,
            kills: 0,
            wave: 0,
            spawn: 2.,
            yaw: 0.,
            pitch: 0.,
            vertical: 0.,
            paused: true,
            won: false,
        }
    }
}
#[derive(Resource)]
struct Arena {
    walls: Vec<(Vec3, Vec3)>,
}
#[derive(Resource)]
struct EnemyAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

fn main() {
    if std::env::var_os("L4D2R_HEAP_AUDIT").is_some() {
        heap_audit::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let args: Vec<String> = std::env::args().collect();
    if let Some(output) = args
        .windows(2)
        .find(|a| a[0] == "--verify-ragdoll-contact")
        .map(|a| &a[1])
    {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(error) =
            ragdoll::verify_contact(std::path::Path::new(root), std::path::Path::new(output))
        {
            eprintln!("Ragdoll contact probe: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(output) = args
        .windows(2)
        .find(|a| a[0] == "--verify-prop-stack")
        .map(|a| &a[1])
    {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = prop_physics::verify_fixture(
            std::path::Path::new(root),
            std::path::Path::new(output),
            true,
        ) {
            eprintln!("Stack probe: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(model) = args
        .windows(2)
        .find(|a| a[0] == "--inspect-physics")
        .map(|a| &a[1])
    {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = physics_data::inspect(std::path::Path::new(root), model) {
            eprintln!("Physics data: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(output) = args
        .windows(2)
        .find(|a| a[0] == "--verify-prop-freefall")
        .map(|a| &a[1])
    {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) =
            prop_physics::verify_freefall(std::path::Path::new(root), std::path::Path::new(output))
        {
            eprintln!("Prop probe: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|a| a == "--verify-vscripts") {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = source_vm::audit(std::path::Path::new(root)) {
            eprintln!("VScript audit: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|a| a == "--verify-campaign-modes") {
        let root = args
            .windows(2)
            .find(|a| a[0] == "--game-dir")
            .map(|a| a[1].as_str())
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = map_modes::verify(std::path::Path::new(root)) {
            eprintln!("Campaign mode audit failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.get(1).is_some_and(|s| s == "--verify-test-map") {
        let root = args
            .iter()
            .position(|a| a == "--game-dir")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = test_lab::verify(std::path::Path::new(root)) {
            eprintln!("Laboratory: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args
        .get(1)
        .is_some_and(|a| a == "--capture-loading" || a == "--verify-loading")
    {
        if let Err(e) = game_loading::run(&args) {
            eprintln!("Loading screen: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args
        .get(1)
        .is_some_and(|s| s == "--map" || s == "--validate-map" || s == "--capture-map")
    {
        let path = args.get(2).filter(|s|!s.starts_with("--")).map(String::as_str).unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2\left4dead2\maps\c1m1_hotel.bsp");
        let explicit = args
            .iter()
            .position(|s| s == "--game-dir")
            .and_then(|i| args.get(i + 1));
        let root = explicit
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::path::Path::new(path)
                    .ancestors()
                    .find(|p| p.join("left4dead2.exe").is_file())
                    .map(std::path::Path::to_path_buf)
            })
            .unwrap_or_else(|| {
                std::path::PathBuf::from(
                    r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2",
                )
            });
        println!("Loading native map and owned assets: {path}");
        match bsp::load_file(std::path::Path::new(path), &root)
            .and_then(|w| scene::Scene::load(&root, w))
        {
            Ok(scene) => {
                println!("{}", scene.summary());
                if let Err(e) = scene.write_report() {
                    eprintln!("Report write failed: {e}");
                }
                if args[1] == "--validate-map" {
                    if scene.failed_props > 0 || !scene.report.is_empty() {
                        std::process::exit(1);
                    }
                } else if args[1] == "--capture-map" {
                    let output = args
                        .iter()
                        .position(|s| s == "--capture-output")
                        .and_then(|i| args.get(i + 1))
                        .cloned()
                        .unwrap_or_else(|| "hotel-preview.png".into());
                    let vector = |flag: &str| {
                        args.iter()
                            .position(|s| s == flag)
                            .and_then(|i| args.get(i + 1))
                            .map(|s| kv::vector(Some(s)))
                    };
                    viewer::capture(
                        scene,
                        output,
                        vector("--capture-origin"),
                        vector("--capture-angles"),
                    );
                } else {
                    viewer::run(scene);
                }
            }
            Err(e) => {
                eprintln!("Map load failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.get(1).is_some_and(|s| s == "--index-assets") {
        let root = args
            .get(2)
            .map(String::as_str)
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if let Err(e) = vpk::index_installation(std::path::Path::new(root)) {
            eprintln!("Asset indexing failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    if !args.get(1).is_some_and(|s| s == "--arena") {
        let root = args
            .iter()
            .position(|s| s == "--game-dir")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        // A plain PLAY.cmd start opens the main menu, whose full-screen video
        // hides the world, and Start always loads the chosen map in-window.
        // Preloading a full campaign map there (Hotel) left GPU-side residue
        // that halved the frame rate of the map loaded next (DEVELOPMENT.md,
        // 15 October), so the menu sits on the tiny laboratory map instead.
        let direct = args.iter().skip(1).any(|a| {
            matches!(
                a.as_str(),
                "--game"
                    | "--test-map"
                    | "--loading-game"
                    | "--verify-game"
                    | "--capture-game"
                    | "--capture-menu"
                    | "--menu-hotel"
            )
        });
        let default_map = if args.get(1).is_some_and(|s| s == "--test-map") || !direct {
            test_lab::path()
        } else {
            std::path::Path::new(root).join("left4dead2/maps/c1m1_hotel.bsp")
        };
        let map = args
            .get(2)
            .filter(|s| !s.starts_with("--") && s.ends_with(".bsp"))
            .map(std::path::PathBuf::from)
            .unwrap_or(default_map);
        let capture = args
            .get(1)
            .filter(|s| s.as_str() == "--capture-game" || s.as_str() == "--capture-menu")
            .map(|_| {
                args.iter()
                    .position(|s| s == "--capture-output")
                    .and_then(|i| args.get(i + 1))
                    .cloned()
                    .unwrap_or_else(|| "game-preview.png".into())
            });
        if let Err(e) = game::run(
            std::path::Path::new(root),
            &map,
            capture,
            args.get(1).is_some_and(|s| s == "--verify-game"),
        ) {
            eprintln!("Game initialization failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "L4D2 Rust / Bevy â€” combat prototype".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.035, 0.045, 0.05)))
        .init_resource::<Session>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (controls, movement, director, combat, infected_ai, hud).chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let concrete = materials.add(Color::srgb(0.19, 0.22, 0.23));
    let asphalt = materials.add(Color::srgb(0.09, 0.11, 0.12));
    let safe = materials.add(Color::srgb(0.12, 0.48, 0.27));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(42., 0.2, 70.))),
        MeshMaterial3d(asphalt),
        Transform::from_xyz(0., -0.1, -18.),
    ));
    let walls = vec![
        (Vec3::new(-14., 2., -18.), Vec3::new(1., 4., 64.)),
        (Vec3::new(14., 2., -18.), Vec3::new(1., 4., 64.)),
        (Vec3::new(0., 2., 14.), Vec3::new(28., 4., 1.)),
        (Vec3::new(0., 2., -50.), Vec3::new(28., 4., 1.)),
        (Vec3::new(-6., 1.5, -8.), Vec3::new(5., 3., 8.)),
        (Vec3::new(6., 1.5, -25.), Vec3::new(5., 3., 10.)),
        (Vec3::new(-5., 1., -35.), Vec3::new(4., 2., 5.)),
    ];
    for (center, size) in &walls {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::from_size(*size))),
            MeshMaterial3d(concrete.clone()),
            Transform::from_translation(*center),
        ));
    }
    commands.insert_resource(Arena { walls });
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(6., 0.05, 5.))),
        MeshMaterial3d(safe),
        Transform::from_xyz(0., 0.03, -45.),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 4500.,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, -0.5, 0.)),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0., 1.65, 8.),
        Player,
    ));
    commands.insert_resource(EnemyAssets {
        mesh: meshes.add(Capsule3d::new(0.35, 0.9)),
        material: materials.add(Color::srgb(0.48, 0.2, 0.16)),
    });
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(20.),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: px(24.),
            top: px(20.),
            ..default()
        },
        Hud,
    ));
    commands.spawn((
        Text::new("+"),
        TextFont {
            font_size: FontSize::Px(26.),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: percent(50.),
            top: percent(50.),
            ..default()
        },
    ));
}

fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut cursor: Single<&mut CursorOptions>,
    window: Single<&Window>,
    mut s: ResMut<Session>,
    mut player: Single<&mut Transform, With<Player>>,
    mut commands: Commands,
    enemies: Query<Entity, With<Infected>>,
) {
    if keys.just_pressed(KeyCode::Enter) && (s.health <= 0. || s.won) {
        for entity in &enemies {
            commands.entity(entity).despawn();
        }
        *s = Session::default();
        player.translation = Vec3::new(0., 1.65, 8.);
        player.rotation = Quat::IDENTITY;
    }
    if keys.just_pressed(KeyCode::Escape) || !window.focused {
        s.paused = true;
    }
    if mouse.just_pressed(MouseButton::Left) && window.focused && s.health > 0. && !s.won {
        s.paused = false;
    }
    cursor.visible = s.paused || s.health <= 0. || s.won;
    cursor.grab_mode = if cursor.visible {
        CursorGrabMode::None
    } else {
        CursorGrabMode::Locked
    };
}

fn slide(pos: Vec3, delta: Vec3, walls: &[(Vec3, Vec3)]) -> Vec3 {
    let mut result = pos;
    for axis in [0, 2] {
        let mut trial = result;
        trial[axis] += delta[axis];
        let blocked = walls.iter().any(|(c, size)| {
            (trial.x - c.x).abs() < size.x / 2. + 0.35 && (trial.z - c.z).abs() < size.z / 2. + 0.35
        });
        if !blocked {
            result = trial;
        }
    }
    result
}
fn movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<AccumulatedMouseMotion>,
    arena: Res<Arena>,
    mut s: ResMut<Session>,
    mut player: Single<&mut Transform, With<Player>>,
) {
    if s.paused || s.health <= 0. || s.won {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    s.yaw -= mouse.delta.x * 0.002;
    s.pitch = (s.pitch - mouse.delta.y * 0.002).clamp(-1.5, 1.5);
    player.rotation = Quat::from_euler(EulerRot::YXZ, s.yaw, s.pitch, 0.);
    let mut input = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        input.z -= 1.;
    }
    if keys.pressed(KeyCode::KeyS) {
        input.z += 1.;
    }
    if keys.pressed(KeyCode::KeyA) {
        input.x -= 1.;
    }
    if keys.pressed(KeyCode::KeyD) {
        input.x += 1.;
    }
    let crouched = keys.pressed(KeyCode::ControlLeft);
    let speed = if crouched { 2. } else { 4.4 };
    player.translation = slide(
        player.translation,
        Quat::from_rotation_y(s.yaw) * input.normalize_or_zero() * speed * dt,
        &arena.walls,
    );
    let floor = if crouched { 1. } else { 1.65 };
    if keys.just_pressed(KeyCode::Space) && player.translation.y <= floor + 0.01 {
        s.vertical = 5.;
    }
    s.vertical -= 14. * dt;
    player.translation.y += s.vertical * dt;
    if player.translation.y <= floor {
        player.translation.y = floor;
        s.vertical = 0.;
    }
    if player.translation.z < -42. && player.translation.x.abs() < 3. && s.kills >= 24 {
        s.won = true;
    }
}

fn director(
    time: Res<Time>,
    mut s: ResMut<Session>,
    assets: Res<EnemyAssets>,
    mut commands: Commands,
    enemies: Query<(), With<Infected>>,
) {
    if s.paused || s.health <= 0. || s.won {
        return;
    }
    s.spawn -= time.delta_secs().min(0.05);
    if s.spawn > 0. || enemies.iter().count() >= 32 || s.wave >= 8 {
        return;
    }
    s.wave += 1;
    s.spawn = 8.;
    for i in 0..6 {
        commands.spawn((
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            Transform::from_xyz(-10. + i as f32 * 4., 0.8, -40. - (s.wave % 3) as f32),
            Infected {
                health: 60.,
                attack: 0.,
            },
        ));
    }
}

fn ray_box(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut near: f32 = 0.;
    let mut far: f32 = f32::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < 0.00001 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    Some(near)
}
fn combat(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    player: Single<&Transform, With<Player>>,
    arena: Res<Arena>,
    mut s: ResMut<Session>,
    mut enemies: Query<(Entity, &Transform, &mut Infected), Without<Player>>,
    mut commands: Commands,
) {
    if s.paused || s.health <= 0. || s.won {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    s.cooldown = (s.cooldown - dt).max(0.);
    if s.reload > 0. {
        s.reload -= dt;
        if s.reload <= 0. {
            let amount = (15 - s.ammo).min(s.reserve);
            s.ammo += amount;
            s.reserve -= amount;
        }
        return;
    }
    if keys.just_pressed(KeyCode::KeyR) && s.ammo < 15 && s.reserve > 0 {
        s.reload = 1.7;
        return;
    }
    let shove = mouse.just_pressed(MouseButton::Right);
    let fire = mouse.pressed(MouseButton::Left) && s.ammo > 0;
    if s.cooldown > 0. || (!shove && !fire) {
        return;
    }
    s.cooldown = if shove { 0.7 } else { 0.18 };
    if !shove {
        s.ammo -= 1;
    }
    let direction = *player.forward();
    let origin = player.translation;
    let wall = arena
        .walls
        .iter()
        .filter_map(|(c, size)| ray_box(origin, direction, *c - *size / 2., *c + *size / 2.))
        .fold(100., f32::min);
    let mut target = None;
    let mut nearest = if shove { 2.2_f32.min(wall) } else { wall };
    for (entity, transform, _) in &enemies {
        if let Some(distance) = ray_box(
            origin,
            direction,
            transform.translation - Vec3::new(0.4, 0.8, 0.4),
            transform.translation + Vec3::new(0.4, 0.8, 0.4),
        ) {
            if distance < nearest {
                nearest = distance;
                target = Some(entity);
            }
        }
    }
    if let Some(entity) = target {
        if let Ok((_, _, mut infected)) = enemies.get_mut(entity) {
            infected.health -= if shove { 15. } else { 30. };
            infected.attack = if shove { 1.5 } else { 0.3 };
            if infected.health <= 0. {
                commands.entity(entity).despawn();
                s.kills += 1;
            }
        }
    }
}
fn infected_ai(
    time: Res<Time>,
    player: Single<&Transform, With<Player>>,
    arena: Res<Arena>,
    mut s: ResMut<Session>,
    mut enemies: Query<(&mut Transform, &mut Infected), Without<Player>>,
) {
    if s.paused || s.health <= 0. || s.won {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    for (mut transform, mut infected) in &mut enemies {
        infected.attack = (infected.attack - dt).max(0.);
        let delta = Vec3::new(
            player.translation.x - transform.translation.x,
            0.,
            player.translation.z - transform.translation.z,
        );
        if delta.length() > 1.1 {
            transform.translation = slide(
                transform.translation,
                delta.normalize_or_zero() * 2.5 * dt,
                &arena.walls,
            );
        } else if infected.attack <= 0. && player.translation.y < 2.4 {
            s.health = (s.health - 6.).max(0.);
            infected.attack = 0.8;
        }
    }
}
fn hud(s: Res<Session>, mut text: Single<&mut Text, With<Hud>>) {
    let status = if s.won {
        "SAFE ROOM REACHED â€” Enter to restart"
    } else if s.health <= 0. {
        "YOU DIED â€” Enter to restart"
    } else if s.paused {
        "Click to play â€” Esc to release mouse"
    } else if s.reload > 0. {
        "Reloadingâ€¦"
    } else {
        "Kill 24 infected, then reach the green safe zone"
    };
    text.0 = format!(
        "L4D2 / RUST COMBAT PROTOTYPE\nHealth {:3.0}    Pistol {} / {}    Kills {}    Wave {}/8\n{}\nWASD â€¢ Mouse aim â€¢ LMB fire â€¢ RMB shove â€¢ R reload â€¢ Space jump â€¢ Ctrl crouch",
        s.health, s.ammo, s.reserve, s.kills, s.wave, status
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walls_occlude_hits() {
        assert_eq!(
            ray_box(
                Vec3::ZERO,
                Vec3::NEG_Z,
                Vec3::new(-1., -1., -5.),
                Vec3::new(1., 1., -3.)
            ),
            Some(3.)
        );
        assert!(
            ray_box(
                Vec3::ZERO,
                Vec3::X,
                Vec3::new(-1., -1., -5.),
                Vec3::new(1., 1., -3.)
            )
            .is_none()
        );
    }
    #[test]
    fn movement_slides_along_walls() {
        let walls = vec![(Vec3::new(1., 1., 0.), Vec3::new(1., 2., 10.))];
        let result = slide(Vec3::ZERO, Vec3::new(1., 0., 1.), &walls);
        assert_eq!(result, Vec3::Z);
    }
    fn combat_app(blocked: bool) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(ButtonInput::<MouseButton>::default())
            .insert_resource(Session {
                paused: false,
                ..default()
            })
            .insert_resource(Arena {
                walls: if blocked {
                    vec![(Vec3::new(0., 1., -2.), Vec3::new(3., 3., 0.2))]
                } else {
                    vec![]
                },
            })
            .add_systems(Update, combat);
        app.world_mut()
            .spawn((Player, Transform::from_xyz(0., 1., 0.)));
        let enemy = app
            .world_mut()
            .spawn((
                Infected {
                    health: 60.,
                    attack: 0.,
                },
                Transform::from_xyz(0., 0.8, -4.),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        (app, enemy)
    }
    #[test]
    fn pistol_hits_enemy_in_ecs_schedule() {
        let (mut app, enemy) = combat_app(false);
        app.update();
        assert_eq!(app.world().get::<Infected>(enemy).unwrap().health, 30.);
        assert_eq!(app.world().resource::<Session>().ammo, 14);
    }
    #[test]
    fn wall_blocks_enemy_damage_in_ecs_schedule() {
        let (mut app, enemy) = combat_app(true);
        app.update();
        assert_eq!(app.world().get::<Infected>(enemy).unwrap().health, 60.);
        assert_eq!(app.world().resource::<Session>().ammo, 14);
    }
    #[test]
    fn reload_conserves_finite_ammunition() {
        let (mut app, _) = combat_app(false);
        {
            let mut s = app.world_mut().resource_mut::<Session>();
            s.ammo = 0;
            s.reserve = 7;
            s.reload = 0.01;
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(50));
        app.update();
        let s = app.world().resource::<Session>();
        assert_eq!(s.ammo, 7);
        assert_eq!(s.reserve, 0);
    }
    #[test]
    fn complete_game_schedule_runs_headless() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Session>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (controls, movement, director, combat, infected_ai, hud).chain(),
            );
        app.world_mut()
            .spawn((Window::default(), CursorOptions::default()));
        app.update();
        {
            let mut s = app.world_mut().resource_mut::<Session>();
            s.paused = false;
            s.spawn = 0.;
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(17));
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<Infected>>()
                .iter(world)
                .count(),
            6
        );
        assert_eq!(world.resource::<Session>().wave, 1);
    }
}
