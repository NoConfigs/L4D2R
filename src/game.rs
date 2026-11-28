//! Playable native campaign runtime. Behavior parity is researched incrementally.
use crate::{
    binary, bsp,
    collision::{Collision, DUCK_HEIGHT, EYE, HEIGHT, RADIUS, UNIT},
    game_audio, game_menu,
    game_state::{Companion, Gun, Match, Rules, Squad, Weapon},
    game_ui,
    scene::Scene,
    source_assets::SourceAssets,
    viewer,
};
use bevy::{
    camera::visibility::NoFrustumCulling,
    camera::visibility::RenderLayers,
    input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};
use std::{collections::HashMap, io, path::Path};
#[derive(Component)]
pub(crate) struct Body {
    pub(crate) feet: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) grounded: bool,
    pub(crate) crouched: bool,
    pub(crate) stance: crate::player_movement::Stance,
    pub(crate) previous_feet: Vec3,
    previous_eye: f32,
    pub(crate) contact_normal: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}
impl Body {
    #[cfg(test)]
    pub(crate) fn at(feet: Vec3) -> Self {
        Self {
            feet,
            velocity: Vec3::ZERO,
            grounded: true,
            crouched: false,
            stance: default(),
            previous_feet: feet,
            previous_eye: EYE,
            contact_normal: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
        }
    }
}
#[derive(Resource, Default)]
pub(crate) struct Input {
    pub(crate) throw_held: bool,
    pub(crate) fire: bool,
    pub(crate) fire_held: bool,
    primary_wait_for_release: bool,
    pub(crate) shove: bool,
    pub(crate) shove_held: bool,
    pub(crate) reload: bool,
    jump: bool,
    pub(crate) use_item: bool,
}
impl Input {
    pub(crate) fn consume_spawn_attack(&mut self) {
        self.primary_wait_for_release = true;
        self.fire = false;
        self.fire_held = false;
        self.throw_held = false;
    }
    fn sample_primary(&mut self, pressed: bool, just_pressed: bool) {
        // The spawn press can last many rendered frames/server ticks. Clearing
        // one tick's attack does not consume the physical button being held.
        if self.primary_wait_for_release {
            self.primary_wait_for_release = pressed;
            self.fire = false;
            self.fire_held = false;
            self.throw_held = false;
            return;
        }
        self.fire |= just_pressed;
        self.fire_held = pressed;
        self.throw_held = pressed;
    }
}
#[derive(Component)]
pub(crate) struct Enemy {
    pub(crate) model: String,
    pub(crate) death_loot: Option<String>,
    pub(crate) loot_dropped: bool,
    pub(crate) death: crate::infected_death::Context,
    pub(crate) movement_speed: f32,
    pub(crate) reaction_serial: u64,
    pub(crate) attack_serial: u64,
    pub(crate) health: f32,
    pub(crate) attack: f32,
    pub(crate) reaction: u8,
    pub(crate) corpse_time: f32,
    pub(crate) burning: f32,
    pub(crate) vertical: f32,
}
impl Enemy {
    pub(crate) fn record_death(
        &mut self,
        cause: crate::infected_death::Cause,
        transform: &Transform,
        incoming: Vec3,
        hit_height: f32,
    ) {
        self.death = crate::infected_death::Context {
            cause,
            head: hit_height >= 1.35,
            // 10415d80: ground speed above 150 units/s dies running.
            running: self.movement_speed > 150. * crate::collision::UNIT,
            incoming: transform.rotation.inverse() * incoming.normalize_or_zero(),
            // Wounds already cut (DeathThroes "none") keep their ragdoll.
            ragdoll: self.death.ragdoll,
            far: false,
        };
    }
}
#[derive(Component)]
pub(crate) struct Pickup {
    pub(crate) class: String,
    pub(crate) origin: Vec3,
}
#[derive(Component)]
pub(crate) struct HeldWeapon(pub String);
#[derive(Component)]
pub struct ViewCamera;
fn held_transform(forward: [f32; 3]) -> Transform {
    let forward = Vec3::from_array(forward).normalize_or_zero();
    let yaw = std::f32::consts::PI - forward.x.atan2(forward.z);
    let pitch = -forward.y.atan2(Vec2::new(forward.x, forward.z).length());
    Transform::from_xyz(0.16, -0.16, -0.22)
        .with_rotation(Quat::from_rotation_x(pitch) * Quat::from_rotation_y(yaw))
}
fn weapon_display(weapon: &Weapon) -> (&str, Transform) {
    if weapon.class == "weapon_gascan" {
        return (
            &weapon.model,
            Transform::from_xyz(0.22, -0.35, -0.55)
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        );
    }
    if !weapon.view_model.is_empty() {
        (
            &weapon.view_model,
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        )
    } else {
        (&weapon.model, held_transform(weapon.muzzle_forward))
    }
}
#[derive(Component)]
pub(crate) struct Taken;
#[derive(Component)]
pub(crate) struct CompanionActor(pub usize);
#[derive(Component)]
pub(crate) struct CompanionGun(pub String);
#[derive(Resource)]
pub(crate) struct Campaign {
    spawn: Vec3,
    angles: [f32; 3],
    pickups: Vec<(usize, String, Vec3)>,
    pub enemy_model: String,
    pub(crate) population: crate::infected_population::Population,
    total_spawned: u32,
}
#[derive(Resource)]
struct CaptureTelemetry(usize);
#[derive(Resource, Default)]
struct AudioHistory {
    class: String,
    ammo: u32,
    reload: f32,
    health: f32,
    temporary: f32,
    medkit: bool,
    pills: bool,
    slot: u8,
    position: Vec3,
    step_distance: f32,
    bot_cooldowns: Vec<f32>,
    seen: bool,
    grounded: bool,
    left_foot: bool,
    step_timer: f32,
    bot_steps: Vec<(Vec3, f32, bool)>,
}
fn gameplay_audio(
    time: Res<Time>,
    native: Res<crate::native_movement_audio::NativeAudio>,
    collision: Res<Collision>,
    s: Res<Match>,
    _rules: Res<Rules>,
    body: Single<&Body>,
    squad: Res<Squad>,
    mut history: ResMut<AudioHistory>,
    mut queue: ResMut<game_audio::Queue>,
) {
    if s.paused || s.dead {
        history.seen = false;
        return;
    }
    if history.seen {
        // Survivor inventory cues (team switches change slot/class too).
        if s.slot != history.slot && !s.infected {
            queue.play("Player.WeaponSelected", None);
        }
        if (s.medkit && !history.medkit
            || s.pills && !history.pills
            || history.class != s.gun().class)
            && !s.infected
        {
            queue.play("Player.PickupWeapon", None);
        }
        if !s.pills && history.pills && s.temporary > history.temporary {
            queue.play("PainPills.Use", None);
        }
        // Survivor hurt sound; infected players get class pain voices.
        if s.health < history.health && !s.infected {
            queue.play("Zombie.Punch", None);
        }
        let travel = (body.feet - history.position) * Vec3::new(1., 0., 1.);
        history.step_timer = (history.step_timer - time.delta_secs()).max(0.);
        // Spawn-mode ghosts make no movement sounds.
        let ghost = s.infected && s.special_phase == 5;
        if body.grounded && !history.grounded && !ghost {
            // Retail 102d2bc0: infected players land with PlayerZombie.JumpLand.
            queue.play(
                if s.infected {
                    "PlayerZombie.JumpLand"
                } else {
                    "Player.JumpLand"
                },
                None,
            );
            history.step_timer = 0.15;
        }
        if body.grounded && !ghost && travel.length() < 1. && travel.length_squared() > 1e-8 {
            history.step_distance += travel.length();
            let walk = body.crouched || body.velocity.xz().length() < 150. * UNIT;
            if history.step_timer <= 0. && history.step_distance > 0.08 {
                history.step_distance = 0.;
                history.left_foot = !history.left_foot;
                history.step_timer = if body.crouched {
                    0.6
                } else if walk {
                    0.5
                } else {
                    0.3
                };
                let old_len = queue.0.len();
                let prefix = if s.infected {
                    s.special_class.step_prefix()
                } else {
                    "Survivor"
                };
                queue.play(
                    native.step(body.feet, walk, history.left_foot, prefix),
                    None,
                );
                if let Some(request) = queue.0.back_mut().filter(|_| old_len < 64) {
                    request.gain = if body.crouched { 0.25 } else { 1. };
                }
            }
        } else {
            history.step_distance = 0.;
        }
        for (i, bot) in squad.0.iter().enumerate() {
            while history.bot_steps.len() <= i {
                history.bot_steps.push((bot.position, 0., false));
            }
            let step = &mut history.bot_steps[i];
            let travel = (bot.position - step.0) * Vec3::new(1., 0., 1.);
            step.0 = bot.position;
            step.1 = (step.1 - time.delta_secs()).max(0.);
            if !bot.state.dead
                && !bot.state.incap
                && travel.length() > 0.015
                && travel.length() < 1.
                && collision.grounded(bot.position, HEIGHT)
                && step.1 <= 0.
            {
                step.1 = 0.4;
                step.2 = !step.2;
                queue.play(
                    native.step(bot.position, false, step.2, "Survivor"),
                    Some(bot.position),
                );
            }
            // Bot gunfire is played with the shot itself (each bot's real
            // weapon sound); a second generic Pistol.Fire here doubled it.
        }
    }
    history.class = s.gun().class.clone();
    history.ammo = s.gun().ammo;
    history.reload = s.reload;
    history.health = s.health;
    history.temporary = s.temporary;
    history.medkit = s.medkit;
    history.pills = s.pills;
    history.slot = s.slot;
    history.position = body.feet;
    history.bot_cooldowns = squad.0.iter().map(|b| b.cooldown).collect();
    if !history.seen {
        history.bot_steps = squad.0.iter().map(|b| (b.position, 0., false)).collect();
    }
    history.grounded = body.grounded;
    history.seen = true;
}
pub(crate) struct Prepared {
    session: crate::campaign_settings::SessionSettings,
    catalog: crate::campaign_settings::Catalog,
    selection: crate::campaign_settings::Selection,
    collision: Collision,
    native_audio: crate::native_movement_audio::NativeAudio,
    native_director: crate::source_director::Director,
    initial: Match,
    rules: Rules,
    squad: Squad,
    animations: crate::l4d2r_animation::AnimationLibrary,
    map_logic: crate::l4d2r_cinematic::MapLogic,
    campaign: Campaign,
    scene: Scene,
}
/// The centre (Source units) of the map's NAV PLAYER_START area nearest
/// `near`, where retail starts survivors that have no player start or
/// survivor position for the mode.
fn player_start_near(path: &Path, near: [f32; 3]) -> Option<[f32; 3]> {
    // PLAYER_START first; maps without one (c4m2) start in the CHECKPOINT
    // (saferoom) area nearest the landmark.
    const PLAYER_START: u32 = 0x80;
    const CHECKPOINT: u32 = 0x800;
    let nav = l4d2_formats::nav::parse_nav(&std::fs::read(path.with_extension("nav")).ok()?).ok()?;
    let nearest = |bit: u32| {
        nav.areas
            .iter()
            .filter(|a| a.spawn_attributes & bit != 0)
            .map(|a| {
                let z = (a.nw[2] + a.se[2] + a.ne_z + a.sw_z) / 4.;
                [(a.nw[0] + a.se[0]) / 2., (a.nw[1] + a.se[1]) / 2., z]
            })
            .min_by(|a, b| {
                let d = |p: &[f32; 3]| (0..3).map(|i| (p[i] - near[i]).powi(2)).sum::<f32>();
                d(a).total_cmp(&d(b))
            })
    };
    nearest(PLAYER_START).or_else(|| nearest(CHECKPOINT))
}

pub(crate) fn prepare(
    root: &Path,
    path: &Path,
    capture: Option<String>,
    verify: bool,
    session_override: Option<crate::campaign_settings::SessionSettings>,
) -> io::Result<Option<Prepared>> {
    crate::game_loading::milestone(0.1);
    if std::env::args().any(|s| s == "--verify-audio") {
        return game_audio::verify(root).map(|_| None);
    }
    if std::env::args().any(|s| s == "--verify-projectiles") {
        let mut assets = SourceAssets::new(root, vec![])?;
        let registry = crate::source_sound::Registry::load(&mut assets)?;
        let mut keys = registry
            .definitions
            .keys()
            .filter(|k| {
                k.contains("pipebomb")
                    || k.contains("molotov")
                    || k.contains("vomitjar")
                    || k.contains("swing")
                    || k.contains("grenade")
            })
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        std::fs::write("local-research/throwable-sounds.txt", keys.join("\n"))?;
        for class in ["weapon_molotov", "weapon_pipe_bomb", "weapon_vomitjar"] {
            let weapon = Weapon::load(&mut assets, class)?;
            assert!(weapon.throwable());
            let meshes = crate::studio::load(&mut assets, &weapon.model, 0, 0)?;
            if meshes.is_empty() {
                return Err(binary::invalid("Throwable model has no geometry"));
            }
        }
        println!("Throwable scripts/world models validated; native sound names exported.");
        return Ok(None);
    }
    let mut session = session_override.unwrap_or_else(|| crate::campaign_settings::from_args(path));
    let catalog = crate::campaign_settings::Catalog::load(root)?;
    if std::env::args().any(|a| a == "--verify-triggers-all") {
        let mut failures = vec![];
        let mut report = String::from("campaign\tchapter\tmap\tgeometry\n");
        for campaign in &catalog.campaigns {
            for chapter in &campaign.chapters {
                println!("Auditing triggers: {} / {}", campaign.name, chapter.name);
                let result = crate::l4d2r_cinematic::MapLogic::load(root, &chapter.map, true)
                    .and_then(|logic| logic.verify_triggers(&Collision::for_trigger_audit(&logic)));
                let status = match result {
                    Ok(()) => "passed".to_owned(),
                    Err(e) => {
                        failures.push(chapter.map.clone());
                        e.to_string()
                    }
                };
                report += &format!(
                    "{}\t{}\t{}\t{status}\n",
                    campaign.name,
                    chapter.name,
                    chapter.map.display()
                );
            }
        }
        std::fs::create_dir_all("local-research")?;
        std::fs::write("local-research/campaign-trigger-geometry.tsv", report)?;
        println!(
            "Campaign trigger geometry audit: {} chapters, {} failures",
            catalog
                .campaigns
                .iter()
                .map(|c| c.chapters.len())
                .sum::<usize>(),
            failures.len()
        );
        return if failures.is_empty() {
            Ok(None)
        } else {
            Err(io::Error::other(
                "See local-research/campaign-trigger-geometry.tsv",
            ))
        };
    }
    println!(
        "Installed campaign catalog: {} campaigns, {} chapters",
        catalog.campaigns.len(),
        catalog
            .campaigns
            .iter()
            .map(|c| c.chapters.len())
            .sum::<usize>()
    );
    if let Some(campaign) = catalog
        .campaigns
        .iter()
        // By map name: a launch path can differ in case or separators from
        // the catalog's (a direct c8m1 launch kept the L4D2 survivors).
        .find(|c| {
            c.chapters.iter().any(|ch| {
                ch.map == path
                    || ch.map.file_stem().zip(path.file_stem()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
            })
        })
    {
        session.original = campaign.original;
        if !crate::campaign_settings::Survivor::roster(campaign.original)
            .contains(&session.survivor)
        {
            session.survivor = crate::campaign_settings::Survivor::roster(campaign.original)[0];
        }
    }
    let mut selection = catalog.selection(&session);
    if std::env::args().any(|a| a == "--capture-campaign-flyout") {
        selection.open = Some(0);
    }
    crate::performance::memory_checkpoint("Prep: before BSP");
    // Survival maps load their holdout entity patch and OnSurvival outputs.
    let mode = crate::survival::map_mode();
    let mut world = bsp::load_file_mode(path, root, mode)?;
    crate::performance::memory_checkpoint("Prep: BSP loaded");
    let mut map_logic = crate::l4d2r_cinematic::MapLogic::load_mode(
        root,
        path,
        capture.is_some() && !std::env::args().any(|a| a == "--capture-intro"),
        mode,
    )?;
    map_logic.world.cine.selected_survivor = Some(session.survivor.name().into());
    let spawn_entities = world
        .entities
        .iter()
        .enumerate()
        .filter(|(id, _)| map_logic.world.ents.get(*id).is_some_and(|e| e.alive))
        .map(|(_, e)| e)
        .collect::<Vec<_>>();
    let survival_spawn = (mode == bsp::MapMode::Survival)
        .then(|| {
            spawn_entities.iter().copied().find(|e| {
                e.get("classname").is_some_and(|c| c == "info_survivor_position")
                    && e.get("gamemode").is_some_and(|m| m.eq_ignore_ascii_case("survival"))
            })
        })
        .flatten();
    // A position with a GameMode belongs to that mode only (c1m4's
    // "scavenge_position4" names Coach; taken in coop, it put the player
    // below the atrium).
    let mode_name = match mode {
        bsp::MapMode::Campaign => "coop",
        bsp::MapMode::Survival => "survival",
        bsp::MapMode::Scavenge => "scavenge",
        bsp::MapMode::Versus => "versus",
    };
    let for_mode = |e: &&std::collections::BTreeMap<String, String>| {
        e.get("gamemode").is_none_or(|m| {
            m.split(',').any(|m| m.trim().eq_ignore_ascii_case(mode_name))
                || (mode == bsp::MapMode::Versus && m.trim().eq_ignore_ascii_case("coop"))
        })
    };
    let local_spawn = survival_spawn
        .or_else(|| {
            spawn_entities.iter().copied().filter(for_mode).find(|e| {
                e.get("classname")
                    .is_some_and(|c| c == "info_survivor_position")
                    && e.get("survivorname")
                        .is_some_and(|n| n.eq_ignore_ascii_case(session.survivor.name()))
            })
        })
        .or_else(|| {
            spawn_entities
                .iter()
                .copied()
                .find(|e| e.get("classname").is_some_and(|c| c == "info_player_start"))
        })
        .or_else(|| {
            // Match by map name: a capture or menu start may pass an
            // absolute path where the catalog holds a mount-relative one.
            let stem = |p: &std::path::Path| p.file_stem().map(|s| s.to_ascii_lowercase());
            let previous = catalog.campaigns.iter().find_map(|c| {
                let i = c.chapters.iter().position(|ch| ch.map == path || stem(&ch.map) == stem(path))?;
                i.checked_sub(1).map(|p| &c.chapters[p].map)
            })?;
            let previous_entities =
                l4d2_formats::bsp::Bsp::parse(bsp::load_bytes(previous, root).ok()?).ok()?;
            let current_name = path.file_stem()?.to_str()?;
            let incoming = previous_entities.entities.iter().find_map(|e| {
                // L4D2 maps use info_changelevel.
                (matches!(e.get("classname"), Some("trigger_changelevel" | "info_changelevel"))
                    && e.get("map")
                        .is_some_and(|m| m.eq_ignore_ascii_case(current_name)))
                .then(|| e.get("landmark"))
                .flatten()
            });
            let previous = previous.file_stem()?.to_str()?.split('_').next()?;
            let current = current_name.split('_').next()?;
            spawn_entities.iter().copied().find(|e| {
                e.get("classname").is_some_and(|c| c == "info_landmark")
                    && e.get("targetname").is_some_and(|n| {
                        incoming.is_some_and(|i| n.eq_ignore_ascii_case(i))
                            || (n.contains(previous) && n.contains(current))
                    })
            })
        });
    let start_angles = local_spawn.map(|e| crate::kv::vector(e.get("angles")));
    if let Some(entity) = local_spawn {
        let mut origin = crate::kv::vector(entity.get("origin"));
        // A bare landmark (no coop player start or survivor position, as on
        // c4m2) can sit inside the wall between two rooms: retail spawns in
        // the start area instead (the NAV PLAYER_START area nearest it).
        if entity.get("classname").is_some_and(|c| c == "info_landmark") {
            if let Some(start) = player_start_near(path, origin) {
                origin = start;
            }
        }
        let mut position = binary::source_position(origin);
        position[1] += EYE;
        world.spawn = position;
    }
    if capture.is_some() {
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(origin) = args
            .iter()
            .position(|s| s == "--capture-origin")
            .and_then(|i| args.get(i + 1))
        {
            let mut position = binary::source_position(crate::kv::vector(Some(origin)));
            position[1] += EYE;
            world.spawn = position;
        }
        if args.iter().any(|a| a == "--capture-elevator-fire") {
            for name in [
                "ballroom_inferno_1",
                "ballroom_inferno_2",
                "fire_flat_wall",
                "fire_kitchen_particles",
            ] {
                for id in map_logic.world.find(name) {
                    map_logic.world.input(id, "Start", "", None, None);
                }
            }
        }
        if let Some(hammer) = args
            .iter()
            .position(|s| s == "--capture-open-door")
            .and_then(|i| args.get(i + 1))
        {
            let id = map_logic
                .world
                .ents
                .iter()
                .position(|e| e.me.kv("hammerid") == Some(hammer.as_str()))
                .ok_or_else(|| binary::invalid("Capture door hammerid not found"))?;
            // Same path as a player's +use, so linked double doors follow.
            map_logic.world.use_ent(id, None);
            for _ in 0..60 {
                map_logic.world.tick(1. / 60.);
            }
        }
    }
    crate::performance::memory_checkpoint("Prep: map logic loaded");
    let mut collision = Collision::load(path)?;
    crate::game_loading::milestone(0.25);
    collision.enable_l4d2r(root, &map_logic)?;
    collision.add_terrain(&world);
    if std::env::args().any(|a| a == "--verify-triggers") {
        return map_logic.verify_triggers(&collision).map(|_| None);
    }
    let start_feet = Vec3::from_array(world.spawn) - Vec3::Y * EYE;
    let spawn = collision.supported_spawn_near(start_feet, HEIGHT)?;
    world.spawn = (spawn + Vec3::Y * EYE).to_array();
    let mut angles = spawn_entities
        .iter()
        .copied()
        .filter(for_mode)
        .find(|e| {
            e.get("classname")
                .is_some_and(|c| c == "info_survivor_position")
                && e.get("survivorname")
                    .is_some_and(|n| n.eq_ignore_ascii_case(session.survivor.name()))
        })
        .or_else(|| {
            spawn_entities.iter().copied().find(|e| {
                e.get("classname")
                    .is_some_and(|c| c == "info_survivor_position" || c == "info_player_start")
            })
        })
        .map(|e| crate::kv::vector(e.get("angles")))
        .map_or_else(|| start_angles.unwrap_or([0.; 3]), |a| a);
    if capture.is_some() {
        let args: Vec<String> = std::env::args().collect();
        if let Some(value) = args
            .iter()
            .position(|s| s == "--capture-angles")
            .and_then(|i| args.get(i + 1))
        {
            angles = crate::kv::vector(Some(value));
        }
    }
    let pickups = world
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            let class = crate::scene::pickup_class(e, index)?;
            Some((
                index,
                class,
                Vec3::from_array(binary::source_position(crate::kv::vector(e.get("origin")))),
            ))
        })
        .collect();
    let mut assets = SourceAssets::new(root, vec![])?;
    let pistol = Weapon::load(&mut assets, "weapon_pistol")?;
    let mut weapons = HashMap::new();
    for class in [
        "weapon_pistol",
        "weapon_pistol_dual",
        "weapon_pistol_magnum",
        "weapon_smg",
        "weapon_smg_silenced",
        "weapon_pumpshotgun",
        "weapon_shotgun_chrome",
        "weapon_rifle",
        "weapon_rifle_ak47",
        "weapon_rifle_desert",
        "weapon_autoshotgun",
        "weapon_shotgun_spas",
        "weapon_hunting_rifle",
        "weapon_sniper_military",
        "weapon_smg_mp5",
        "weapon_rifle_sg552",
        "weapon_sniper_awp",
        "weapon_sniper_scout",
        "weapon_first_aid_kit",
        "weapon_pain_pills",
        "weapon_adrenaline",
        "weapon_defibrillator",
        "weapon_molotov",
        "weapon_pipe_bomb",
        "weapon_vomitjar",
        "weapon_gascan",
        "weapon_cola_bottles",
        "weapon_upgradepack_incendiary",
        "weapon_upgradepack_explosive",
    ] {
        if let Ok(mut weapon) = Weapon::load(&mut assets, class) {
            if class == "weapon_gascan" {
                weapon.view_model.clear();
            }
            weapons.insert(class.into(), weapon);
        }
    }
    for kind in [
        crate::upgrades::Kind::Incendiary,
        crate::upgrades::Kind::Explosive,
    ] {
        if let Some(mut weapon) = weapons.get(kind.pack_class()).cloned() {
            weapon.class = kind.floor_class().into();
            weapon.model = kind.floor_model().into();
            weapon.player_model = weapon.model.clone();
            weapon.view_model.clear();
            weapons.insert(weapon.class.clone(), weapon);
        }
    }
    let mut laser = pistol.clone();
    laser.class = "upgrade_laser_sight".into();
    laser.model = "models/w_models/weapons/w_laser_sights.mdl".into();
    laser.player_model = laser.model.clone();
    laser.view_model.clear();
    weapons.insert(laser.class.clone(), laser);
    for name in [
        "fireaxe",
        "crowbar",
        "frying_pan",
        "machete",
        "katana",
        "baseball_bat",
        "cricket_bat",
        "electric_guitar",
        "tonfa",
        "knife",
        "golfclub",
        "pitchfork",
        "shovel",
    ] {
        let class = format!("weapon_melee_{name}");
        if let Ok(weapon) = Weapon::load(&mut assets, &class) {
            weapons.insert(class, weapon);
        }
    }
    println!(
        "Native game: {} solid brushes; {} weapon scripts; pistol clip={} damage={} cycle={}",
        collision.count(),
        weapons.len(),
        pistol.clip,
        pistol.damage,
        pistol.cycle
    );
    if verify {
        return verify_native(&collision, spawn, &pistol).map(|_| None);
    }
    for role in crate::special_infected::SpecialClass::ALL {
        let mut claw = pistol.clone();
        claw.class = role.weapon().into();
        claw.model = role.model().into();
        claw.player_model = claw.model.clone();
        claw.view_model = role.view().into();
        weapons.insert(claw.class.clone(), claw);
    }
    crate::performance::memory_checkpoint("Prep: collision and weapons loaded");
    let mut scene = Scene::load(root, world)?;
    crate::performance::memory_checkpoint("Prep: scene loaded");
    crate::source_break_surfaces::prepare_materials(root,&mut scene,&mut map_logic)?;
    let door_models = map_logic.door_damage.values().flat_map(|p| p.stages.iter().map(|m| m.path.clone()))
        .collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    scene.add_posed_props(root, &door_models)?;
    scene.add_posed_props(root, &map_logic.break_debris.render_models())?;
    if crate::test_lab::active(path) {
        scene.add_models(root, &crate::test_lab::models())?;
    }
    // Tank rock (tank_rock entity model) for the playable Tank.
    if let Err(e) = scene.add_models(root, &[crate::special_infected::tank::ROCK_MODEL.into()]) {
        eprintln!("Tank rock model: {e}");
    }
    crate::game_loading::milestone(0.6);
    if std::env::args().any(|a| a == "--verify-pickups") {
        let mut report = String::from("entity\tclass\tmodel\tvertices\n");
        let mut missing = 0;
        let mut checked = 0;
        for (index, e) in scene.world.entities.iter().enumerate() {
            let Some(class) = crate::scene::pickup_class(e, index) else {
                if e.get("classname").is_some_and(|c| c == "weapon_spawn") {
                    checked += 1;
                    missing += 1;
                    report += &format!("{index}\tUNKNOWN_SELECTION\tMISSING\t0\n");
                }
                continue;
            };
            checked += 1;
            let prop = scene.world.props.iter().find(|p| {
                e.get("classname") == Some(&p.classname)
                    && crate::kv::vector(e.get("origin")) == p.origin
            });
            let vertices = prop
                .and_then(|p| scene.models.get(&(p.model.clone(), p.skin, p.body)))
                .map_or(0, |draws| {
                    draws.iter().map(|d| d.positions.len()).sum::<usize>()
                });
            missing += usize::from(vertices == 0);
            report += &format!(
                "{index}\t{class}\t{}\t{vertices}\n",
                prop.map_or("MISSING", |p| p.model.as_str())
            );
        }
        std::fs::create_dir_all("local-research")?;
        std::fs::write("local-research/pickup-model-audit.tsv", report)?;
        scene.write_report()?;
        println!("Pickup model audit: {checked} pickups; {missing} missing models");
        return if missing == 0 {
            Ok(None)
        } else {
            Err(io::Error::other(
                "Missing pickup geometry; see local-research/pickup-model-audit.tsv",
            ))
        };
    }
    let native_audio =
        crate::native_movement_audio::NativeAudio::load(&mut assets, &scene.world, &map_logic);
    crate::performance::memory_checkpoint("Prep: native audio loaded");
    let mut native_director =
        crate::source_director::Director::load(&mut assets, path, session.difficulty)?;
    crate::performance::memory_checkpoint("Prep: director loaded");
    if let Some(first) = catalog.campaigns.iter().find_map(|c| {
        c.chapters
            .iter()
            .position(|ch| ch.map == path)
            .map(|i| i == 0)
    }) {
        native_director.session_start_map(first);
    } else if crate::test_lab::active(path) {
        native_director.session_start_map(true);
    }
    native_director.sync_world(&map_logic.world);
    let runtime_args = std::env::args().collect::<Vec<_>>();
    if let Some(name) = runtime_args
        .windows(2)
        .find(|a| a[0] == "--mutation")
        .map(|a| a[1].as_str())
    {
        native_director.challenge_options(name)?;
    }
    native_director.flow_start(spawn);
    // Rotating doors are posed once at scene load (scene.rs posed_prop_keys,
    // "closed"/"idle"); posing them again here laid metal_door_112 on its side.
    for weapon in weapons.values_mut() {
        scene.add_models(root, &[weapon.model.clone()])?;
        if !weapon.player_model.is_empty() {
            scene.add_models(root, &[weapon.player_model.clone()])?;
        }
        if crate::special_infected::SpecialClass::from_weapon(&weapon.class).is_some() {
            scene.pose_model(root, &weapon.view_model, &["idle"], None, true)?;
            continue;
        }
        if !weapon.view_model.is_empty() {
            let pose = scene.pose_model(
                root,
                &weapon.view_model,
                &["a_idle_1", "idle", "a_idle", "idle1"],
                None,
                true,
            );
            if let Ok(pose) = pose {
                let arms_path = session.survivor.arms();
                let arms = arms_path.as_str();
                let mut assets = SourceAssets::new(root, vec![])?;
                let mut draws = crate::studio::load(&mut assets, arms, 0, 0)?;
                crate::studio::pose::apply(&mut assets, arms, &mut draws, &pose, true)?;
                // Load arm materials once, then share the weapon's posed draws.
                scene.add_models(root, &[arms.into()])?;
                scene
                    .models
                    .get_mut(&(weapon.view_model.clone(), 0, 0))
                    .unwrap()
                    .extend(draws);
            } else if let Err(e) = pose {
                eprintln!("Viewmodel {}: {e}", weapon.class);
                weapon.view_model.clear();
            }
        }
    }
    let mut population =
        crate::infected_population::Population::load(&mut assets, path, session.original)?;
    population.configure_fallen(&scene.world.entities);
    let infected_models = population.models(crate::test_lab::active(path));
    for model in &infected_models {
        scene.add_models(root, &[model.clone()])?;
        scene.pose_model(root, model, &["Idle", "ACT_IDLE"], None, false)?;
    }
    // The Witch boss is spawned at runtime (special_infected/witch.rs).
    let witch_model = crate::special_infected::witch::MODEL.to_owned();
    if let Err(e) = scene.add_models(root, &[witch_model.clone()]) {
        eprintln!("Witch model {witch_model}: {e}");
    }
    // c6m1's info_zombie_spawn bride (population witch_bride).
    if scene.world.entities.iter().any(|e| {
        e.get("classname").is_some_and(|c| c == "info_zombie_spawn")
            && e.get("population").is_some_and(|p| p.eq_ignore_ascii_case("witch_bride"))
    }) {
        let bride = crate::special_infected::witch::BRIDE_MODEL.to_owned();
        if let Err(e) = scene.add_models(root, &[bride.clone()]) {
            eprintln!("Witch model {bride}: {e}");
        }
    }
    let enemy_model = "models/infected/common_male01.mdl".to_owned();
    for model in [&pistol.model, &pistol.player_model, &enemy_model] {
        if let Err(e) = scene.add_models(root, &[model.clone()]) {
            eprintln!("Gameplay model {model}: {e}");
        }
    }
    scene.pose_model(root, &enemy_model, &["Idle"], None, false)?;
    scene.add_models(
        root,
        &[format!(
            "models/survivors/survivor_{}.mdl",
            session.survivor.character()
        )],
    )?;
    let mut squad = vec![];
    // Unnamed start positions near the player's spawn (c2m2's four
    // survivor_start_coop), nearest first, the player's own excluded: bots
    // take these like retail. The sideways fallback put c2m2's Coach on the
    // saferoom table.
    let mut free_starts: Vec<Vec3> = scene
        .world
        .entities
        .iter()
        .filter(|e| {
            e.get("classname").is_some_and(|c| c == "info_survivor_position")
                && e.get("survivorname").is_none()
                && e.get("gamemode").is_none()
        })
        .map(|e| Vec3::from_array(binary::source_position(crate::kv::vector(e.get("origin")))))
        .filter(|p| {
            let d = (*p - spawn).with_y(0.).length();
            d > 0.3 && d < 12. && (p.y - spawn.y).abs() < 3.
        })
        .collect();
    free_starts.sort_by(|a, b| a.distance_squared(spawn).total_cmp(&b.distance_squared(spawn)));
    let mut free_starts = free_starts.into_iter();
    for (index, survivor) in crate::campaign_settings::Survivor::roster(session.original)
        .into_iter()
        .filter(|s| *s != session.survivor)
        .enumerate()
    {
        let name = survivor.name();
        let character = survivor.character();
        let model = format!("models/survivors/survivor_{character}.mdl");
        scene.add_models(root, &[model.clone()])?;
        scene.pose_model(root, &model, &["Idle_Standing_pistol"], None, false)?;
        // Only this mode's named positions: c4m2's Scavenge-only
        // "Coach" spot put the bots ~7,000 units from the campaign start.
        let position = scene
            .world
            .entities
            .iter()
            .filter(|e| for_mode(e))
            .find(|e| {
                e.get("classname")
                    .is_some_and(|c| c == "info_survivor_position")
                    && e.get("survivorname")
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|e| Vec3::from_array(binary::source_position(crate::kv::vector(e.get("origin")))))
            .or_else(|| free_starts.next())
            .unwrap_or(spawn + Vec3::X * (index as f32 + 1.) * 1.2);
        // A sideways fallback can land in a wall: share the player's spot.
        let position = collision.supported_spawn_near(position, HEIGHT).unwrap_or(spawn);
        squad.push(Companion {
            state: Match::new(&pistol),
            position,
            spawn: position,
            model,
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::X,
        });
    }
    if std::env::args().any(|a| a == "--capture-team-hud") {
        for member in &mut squad {
            member.state.medkit = true;
        }
    }
    // L4D2R_BOT_DEFIB: debug, the first bot starts with a defibrillator.
    if std::env::var_os("L4D2R_BOT_DEFIB").is_some() {
        if let Some(member) = squad.first_mut() {
            member.state.defibrillator = true;
        }
    }
    if std::env::args().any(|a| a == "--capture-survivor-glow") {
        for member in &mut squad {
            let y = if member.model.contains("mechanic") {
                5404.
            } else if member.model.contains("coach") {
                5504.
            } else {
                5604.
            };
            let x = if member.model.contains("producer") {
                320.
            } else {
                440.
            };
            member.position = Vec3::from_array(binary::source_position([x, y, 2848.]));
            member.spawn = member.position;
        }
    }
    // `--capture-bot-lineup`: the bots stand 150 units in front of the
    // capture view, 48 apart, facing it (side-by-side retail comparisons
    // use the same layout from a VScript).
    if std::env::args().any(|a| a == "--capture-bot-lineup") {
        let args: Vec<String> = std::env::args().collect();
        let yaw = args
            .windows(2)
            .find(|w| w[0] == "--capture-angles")
            .map(|w| crate::kv::vector(Some(&w[1]))[1])
            .unwrap_or(0.)
            .to_radians();
        let forward = Vec3::from_array(binary::source_position([yaw.cos(), yaw.sin(), 0.])).normalize_or_zero();
        let right = Vec3::from_array(binary::source_position([yaw.sin(), -yaw.cos(), 0.])).normalize_or_zero();
        let count = squad.len() as f32;
        for (i, member) in squad.iter_mut().enumerate() {
            let side = (i as f32 - (count - 1.) * 0.5) * 48. * UNIT;
            member.position = spawn + forward * 150. * UNIT + right * side;
            member.spawn = member.position;
            member.aim = -forward;
            // `--capture-bot-aim-turn <deg>`: aim this far to the side of the camera.
            if let Some(turn) = args.windows(2).find(|w| w[0] == "--capture-bot-aim-turn").and_then(|w| w[1].parse::<f32>().ok()) {
                member.aim = Quat::from_rotation_y(turn.to_radians()) * -forward;
            }
            if args.iter().any(|a| a == "--capture-bot-dual") {
                member.state.pistol.class = "weapon_pistol_dual".into();
            }
            let p = member.position / UNIT;
            println!("Bot lineup {i}: setpos {:.0} {:.0} {:.0}", p.x, -p.z, p.y);
        }
    }
    if std::env::args().any(|a| a == "--capture-hotel-elevator") {
        for (i, member) in squad.iter_mut().enumerate() {
            member.position = spawn + Vec3::X * (i as f32 - 1.) * 0.45 + Vec3::Z * 0.45;
            member.spawn = member.position;
        }
    }
    if crate::test_lab::active(path) && std::env::args().any(|a| a == "--lab-bot-combat") {
        for (index, member) in squad.iter_mut().enumerate() {
            member.position = if std::env::args().any(|a| a == "--lab-bot-clear-lanes") {
                spawn + Vec3::X * 3. + Vec3::Z * (index as f32 - 1.) * 1.2
            } else {
                spawn + Vec3::X * (index as f32 + 1.) * 1.2
            };
            member.spawn = member.position;
            let class = if index == 1 {
                "weapon_pumpshotgun"
            } else {
                "weapon_rifle"
            };
            member.state.primary = Some(Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: class.into(),
                ammo: 0,
                reserve: 100,
            });
            member.state.slot = 1;
        }
    }
    println!("{}", scene.summary());
    let enemy_draws = scene
        .models
        .get(&(enemy_model.clone(), 0, 0))
        .map_or(0, Vec::len);
    if enemy_draws == 0 {
        return Err(binary::invalid(
            "Native infected model has no drawable geometry",
        ));
    }
    println!(
        "Native common infected: {enemy_draws} mesh draws; squad models loaded={}",
        squad.len()
    );
    let active_capture = capture.is_some() && std::env::args().any(|s| s == "--capture-active");
    let mut initial = Match::new(&pistol);
    initial.paused = !active_capture;
    initial.infected = std::env::args().any(|a| a == "--capture-infected");
    // --capture-class <smoker|boomer|...> selects the infected role for captures.
    if let Some(role) = std::env::args()
        .skip_while(|a| a != "--capture-class")
        .nth(1)
        .and_then(|name| crate::special_infected::SpecialClass::parse(&name))
    {
        initial.special_class = role;
    }
    if std::env::args().any(|a| a == "--capture-dead") {
        initial.health = 0.;
        initial.dead = true;
    }
    if capture.is_some() {
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(class) = args
            .iter()
            .position(|a| a == "--capture-weapon")
            .and_then(|i| args.get(i + 1))
        {
            let weapon = weapons
                .get(class)
                .ok_or_else(|| binary::invalid("Unknown capture weapon"))?;
            if matches!(class.as_str(), "weapon_gascan" | "weapon_cola_bottles") {
                initial.carried = Some(class.clone());
                initial.slot = 6;
            }
            let gun = crate::game_state::Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: class.clone(),
                ammo: weapon.clip,
                reserve: 100,
            };
            if weapon.throwable() {
                initial.throwable = Some(class.clone());
                initial.slot = 3;
            } else if weapon.melee() || weapon.infinite() {
                initial.pistol = gun;
                initial.slot = 2;
            } else {
                initial.primary = Some(gun);
                initial.slot = 1;
            }
        }
        if let Some(kind) = args
            .windows(2)
            .find(|a| a[0] == "--capture-upgrade-pack")
            .map(|a| {
                if a[1] == "explosive" {
                    crate::upgrades::Kind::Explosive
                } else {
                    crate::upgrades::Kind::Incendiary
                }
            })
        {
            initial.upgrade_pack = Some(kind);
            initial.slot = 4;
        }
        if args.iter().any(|a| a == "--capture-laser") {
            if let Some(g) = initial.primary.as_mut() {
                g.laser = true;
            }
        }
        if initial.carried.is_some() {
            initial.primary = None;
            initial.slot = 6;
        }
        if let Some(ammo) = args
            .iter()
            .position(|a| a == "--capture-ammo")
            .and_then(|i| args.get(i + 1))
            .and_then(|a| a.parse::<u32>().ok())
        {
            initial.gun_mut().ammo = ammo;
        }
        if let Some(value) = args
            .windows(2)
            .find(|a| a[0] == "--capture-reserve")
            .and_then(|a| a[1].parse::<u32>().ok())
        {
            initial.gun_mut().reserve = value;
        }
        if let Some(class) = args
            .windows(2)
            .find(|a| a[0] == "--capture-secondary")
            .map(|a| &a[1])
        {
            let weapon = weapons
                .get(class)
                .ok_or_else(|| binary::invalid("Unknown capture secondary"))?;
            initial.pistol = crate::game_state::Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: class.clone(),
                ammo: weapon.clip,
                reserve: 0,
            };
        }
        if args.iter().any(|a| a == "--capture-inventory") {
            initial.medkit = true;
            initial.pills = true;
            initial.throwable = Some("weapon_pipe_bomb".into());
        }
        if let Some(slot) = args
            .windows(2)
            .find(|a| a[0] == "--capture-slot")
            .and_then(|a| a[1].parse::<u8>().ok())
        {
            initial.slot = slot.clamp(1, 5);
        }
        if let Some(value) = args
            .windows(2)
            .find(|a| a[0] == "--capture-secondary-ammo")
            .and_then(|a| a[1].parse::<u32>().ok())
        {
            initial.pistol.ammo =
                value.min(weapons.get(&initial.pistol.class).unwrap_or(&pistol).clip);
        }
        if args.iter().any(|a| a == "--capture-reload") {
            let weapon = weapons.get(&initial.gun().class).unwrap_or(&pistol);
            initial.reload = weapon.reload_times[0];
            initial.reload_phase = if weapon.shell_reload() { 1 } else { 0 };
        }
    }
    let mut animated_models: Vec<_> = squad.iter().map(|member| member.model.clone()).collect();
    animated_models.push(format!(
        "models/survivors/survivor_{}.mdl",
        session.survivor.character()
    ));
    animated_models.push(enemy_model.clone());
    animated_models.extend(infected_models);
    animated_models.extend(map_logic.scripted_models());
    if crate::test_lab::active(path) {
        animated_models.extend(crate::test_lab::models());
    }
    animated_models.extend(
        crate::special_infected::SpecialClass::ALL
            .into_iter()
            .map(|role| role.model().to_owned()),
    );
    animated_models.push(witch_model);
    let animations = crate::l4d2r_animation::AnimationLibrary::load(
        root,
        &animated_models,
        &weapons,
        &session.survivor.arms(),
    )?;
    for path in animations.gore.paths().into_iter().chain(crate::special_infected::boomer_gibs::paths()) {
        if let Err(error) = scene.add_models(root, &[path.clone()]) {
            eprintln!("l4d2r gore model {path}: {error}");
        }
    }
    if std::env::args().any(|a| a == "--verify-animations") {
        return animations.audit(&weapons).map(|_| None);
    }
    for weapon in weapons.values_mut() {
        weapon.calibrate_reload_times(animations.reload_times(&weapon.view_model));
        weapon.reload_empty_time = animations.empty_reload_time(&weapon.view_model);
    }
    if std::env::args().any(|a| a == "--capture-reload") {
        initial.begin_reload(weapons.get(&initial.gun().class).unwrap_or(&pistol));
    }
    // A new map brings back any survivor bot removed with `kick`.
    crate::game_state::clear_kicked();
    Ok(Some(Prepared {
        session,
        catalog,
        selection,
        collision,
        native_audio,
        native_director,
        initial,
        rules: Rules { weapons, pistol },
        squad: Squad(squad),
        animations,
        map_logic,
        campaign: Campaign {
            spawn,
            angles,
            pickups,
            enemy_model,
            population,
            total_spawned: 0,
        },
        scene,
    }))
}
pub fn run(root: &Path, path: &Path, capture: Option<String>, verify: bool) -> io::Result<()> {
    let Some(Prepared {
        session,
        catalog,
        selection,
        collision,
        native_audio,
        native_director,
        initial,
        rules,
        squad,
        animations,
        map_logic,
        campaign,
        scene,
    }) = prepare(root, path, capture.clone(), verify, None)?
    else {
        return Ok(());
    };
    let menu_capture = std::env::args().any(|a| a == "--capture-menu");
    let direct_game = (capture.is_some() && !menu_capture)
        || std::env::args().any(|a| a == "--game" || a == "--test-map" || a == "--loading-game");
    let mut app = App::new();
    // A command aimed at an object a map release already removed must not
    // close the game (Bevy's default panics). Log it and carry on.
    app.set_error_handler(bevy::ecs::error::warn);
    let plugins = DefaultPlugins
        .set(crate::viewer::render_plugin())
        .set(AssetPlugin {
            // iOS: relative to the app bundle (Bevy's base path is the
            // executable's folder). Android reads the APK assets instead.
            file_path: if cfg!(target_os = "ios") {
                "assets".into()
            } else {
                concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into()
            },
            ..default()
        })
        .set(WindowPlugin {
            primary_window: if capture.is_some() {
                None
            } else {
                Some(Window {
                    title: "L4D2 RUST".into(),
                    visible: !std::env::args().any(|a| a == "--load-status"),
                    resolution: (1280, 720).into(),
                    // UIKit owns the drawable size; a desktop-sized UIWindow
                    // can extend beyond the phone's actual display.
                    mode: if cfg!(target_os = "ios") {
                        bevy::window::WindowMode::BorderlessFullscreen(
                            bevy::window::MonitorSelection::Current,
                        )
                    } else {
                        bevy::window::WindowMode::Windowed
                    },
                    ..default()
                })
            },
            exit_condition: if capture.is_some() {
                bevy::window::ExitCondition::DontExit
            } else {
                bevy::window::ExitCondition::OnPrimaryClosed
            },
            ..default()
        });
    #[cfg(feature = "profile-trace")]
    let plugins = plugins.set(bevy::log::LogPlugin {
        custom_layer: crate::performance::trace_layer,
        ..default()
    });
    #[cfg(target_os = "ios")]
    let plugins = plugins.set(bevy::log::LogPlugin {
        custom_layer: crate::ios_file_log,
        // winit's iOS event loop warns twice per frame ("processing non
        // RedrawRequested event after the main event loop"); that flood
        // costs CPU and buries the game's own log lines.
        filter: format!("{}winit::platform_impl::ios::app_state=error", bevy::log::DEFAULT_FILTER),
        ..default()
    });
    if capture.is_some() {
        app.add_plugins(plugins.disable::<bevy::winit::WinitPlugin>())
            .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
                if std::env::args().any(|a| a == "--uncapped-profile") {
                    std::time::Duration::ZERO
                } else {
                    std::time::Duration::from_secs_f64(1. / 60.)
                },
            ));
    } else {
        app.add_plugins(plugins);
    }
    crate::particle_pull::install(&mut app);
    crate::ui_presentation::install(&mut app);
    crate::adrenaline::install(&mut app);
    install_map_logic(app.world_mut(), map_logic);
    app.add_plugins(MaterialPlugin::<crate::particle_sprite::SpriteMaterial>::default())
        .insert_resource(ClearColor(Color::srgb(0.1, 0.12, 0.14)))
        .insert_resource(session)
        .insert_resource(catalog)
        .insert_resource(selection)
        .insert_resource(collision)
        .insert_resource(native_audio)
        .insert_resource(native_director)
        .insert_resource(initial)
        .insert_resource(rules)
        .insert_resource(squad)
        .insert_resource(animations)
        .init_resource::<crate::l4d2r_animation::PlaybackState>()
        .init_resource::<crate::l4d2r_animation::BotFlashlights>()
        .init_resource::<MeleeHits>()
        .init_resource::<crate::studio_hitboxes::Audit>()
        .insert_resource(crate::l4d2r_navigation::Navigation::load(path))
        .insert_resource(campaign)
        .insert_resource(game_ui::HudSource(root.to_path_buf()))
        .insert_resource(game_menu::Frontend {
            page: if std::env::args().any(|a| a == "--capture-vote") {
                game_menu::Page::Vote
            } else if std::env::args().any(|a| a == "--capture-difficulty") {
                game_menu::Page::Difficulty
            } else if std::env::args().any(|a| a == "--capture-video") {
                game_menu::Page::Video
            } else if std::env::args().any(|a| a == "--capture-advanced-video") {
                game_menu::Page::Advanced
            } else if std::env::args().any(|a| a == "--capture-pause") {
                game_menu::Page::Pause
            } else if std::env::args().any(|a| a == "--capture-glows") {
                game_menu::Page::Glow
            } else if std::env::args().any(|a| a == "--capture-options") {
                game_menu::Page::Options
            } else if std::env::args().any(|a| a == "--capture-touch-hud") {
                game_menu::Page::Touch
            } else if std::env::args().any(|a| a == "--capture-addons" || a == "--capture-addons-steam") {
                game_menu::Page::Addons
            } else if std::env::args().any(|a| a == "--capture-lobby") {
                game_menu::Page::Lobby
            } else if std::env::args().any(|a| a == "--capture-browser") {
                game_menu::Page::Browser
            } else if std::env::args()
                .any(|a| a == "--capture-campaign" || a == "--capture-campaign-flyout")
            {
                game_menu::Page::Campaign
            } else if direct_game {
                game_menu::Page::Game
            } else {
                game_menu::Page::Home
            },
            mode: std::env::args()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|a| a[0] == "--capture-menu-mode")
                .and_then(|a| a[1].parse::<usize>().ok())
                .filter(|_| menu_capture)
                .map(|m| m % 8)
                .unwrap_or_else(|| {
                    usize::from(
                        menu_capture && std::env::args().any(|a| a == "--capture-menu-next"),
                    )
                }),
            restart: false,
            started: direct_game,
        })
        .init_resource::<game_audio::Queue>()
        .init_resource::<game_audio::Settings>()
        .init_resource::<AudioHistory>()
        .init_resource::<Input>()
        .init_resource::<crate::projectiles::Projectiles>()
        .insert_resource(Time::<Fixed>::from_hz(30.));
    if capture.is_some() {
        app.insert_resource(CaptureTelemetry(0))
            .add_systems(Update, capture_telemetry)
            .add_systems(Update, finale_capture_assist);
        crate::muzzle_fixture::install(&mut app);
        if std::env::args().any(|a| a == "--capture-glow-resize") {
            app.world_mut().spawn(Window::default());
        }
    }
    if std::env::args().any(|a| a == "--hotel-live-test") {
        app.add_systems(
            FixedUpdate,
            crate::hotel_live_test::update.before(crate::l4d2r_cinematic::tick),
        );
    }
    crate::performance::install_pipeline_status(&mut app);
    if std::env::args().any(|a| a == "--profile-frames") {
        app.init_resource::<crate::performance::FrameProfile>();
        crate::performance::install_profile(&mut app);
    }
    crate::performance::install_autopilot(&mut app);
    crate::performance::install_counts(&mut app);
    app.add_systems(Update, (crate::performance::asset_memory_report, crate::performance::keep_render_device));
    crate::performance::install_memory_release(&mut app);
    crate::stack_sampler::install(&mut app);
    crate::skate_mod::install(&mut app);
    app.add_systems(
        PostUpdate,
        crate::gun_effects::update.after(bevy::transform::TransformSystems::Propagate),
    );
    app.add_systems(
        FixedUpdate,
        crate::carryables::update.before(interact).before(combat),
    );
    app.add_systems(
        FixedUpdate,
        crate::upgrades::update
            .after(interact)
            .before(crate::survivor_items::update)
            .before(combat),
    );
    app.add_systems(
        PostUpdate,
        crate::upgrades::laser.after(bevy::transform::TransformSystems::Propagate),
    );
    app.init_resource::<crate::performance::DisplaySettings>();
    crate::graphics_options::install(&mut app);
    app.add_systems(Update, crate::performance::display);
    app.add_systems(Last, crate::performance::record);
    app.add_systems(First, crate::ragdoll::refill_budget);
    app.add_systems(
        PostUpdate,
        crate::source_particles::rebuild_after_restart.before(crate::source_particles::update),
    );
    app.add_systems(
        PostUpdate,
        crate::source_particles::sync_sky_fog.before(bevy::asset::AssetEventSystems),
    );
    app.add_systems(
        PostUpdate,
        crate::precipitation::update.after(bevy::transform::TransformSystems::Propagate),
    );
    app.add_systems(
        PostUpdate,
        crate::source_particles::update
            .after(crate::gun_effects::update)
            .after(crate::precipitation::update)
            .before(bevy::asset::AssetEventSystems)
            .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
    );
    viewer::install_game(&mut app, scene, capture);
    app.add_systems(
        Startup,
        (
            initialize,
            game_ui::setup,
            crate::l4d2r_cinematic::setup,
            crate::gun_effects::setup,
            crate::precipitation::setup,
            crate::source_particles::setup,
            crate::graphics_options::setup_grain,
            game_ui::target_camera,
            game_audio::setup,
            game_menu::setup,
            crate::performance::setup,
            crate::bullet_impacts::setup,
            crate::upgrades::setup,
            ui_fit::setup,
            touch::setup,
            touch::editor_setup,
        )
            .chain()
            .after(viewer::RenderSetup::Scene),
    )
    .add_systems(PreUpdate, touch::inject.after(bevy::input::InputSystems))
    .add_systems(Update, (crate::performance::play_memory, crate::performance::frame_shots))
    .add_systems(
        Update,
        (
            (game_menu::update, touch::editor_update, touch::draw, ui_fit::read_safe_area, ui_fit::apply_render_scale, ui_fit::fit_hud, ui_fit::hud_clipping, ui_fit::fit_menu, ui_fit::menu_clipping).chain(),
            controls,
            sync_camera,
            sync_weapon,
            sync_companion_items,
            (smooth_companions, crate::l4d2r_animation::update, crate::survivor_flashlight::update).chain(),
            (
                game_ui::update,
                game_ui::update_frames,
                (timed_button::update, crate::progress_bar::update).chain(),
            )
                .chain(),
            (game_ui::death_presentation, game_ui::team_presentation, game_ui::local_presentation, game_ui::health_tint, game_ui::kill_feed::update, game_ui::place_team_panels),
            gameplay_audio,
            game_audio::playback,
            game_audio::music_volume,
            game_menu::presentation,
            (game_menu::volume_label, game_menu::hybrid_paint),
            (game_menu::detail_labels, game_menu::addons_tabs),
            game_menu::campaign_presentation,
            crate::menu_video::update,
            crate::l4d2r_cinematic::presentation,
            crate::source_breakables::presentation,
            crate::source_break_debris::presentation,
            crate::source_break_surfaces::presentation,
        )
            .chain(),
    )
    .add_systems(
        Update,
        crate::l4d2r_cinematic::scripted_skinning.after(crate::l4d2r_cinematic::presentation),
    )
    .init_resource::<crate::loot::scavenge::Scavenge>()
    .init_resource::<crate::l4d2r_cinematic::PlatformRiders>()
    .init_resource::<crate::ledge_hang::Tracker>()
    .add_systems(
        FixedUpdate,
        (
            begin_player_tick,
            crate::l4d2r_cinematic::tick,
            move_player,
            timers,
            interact,
            crate::survivor_items::update,
            crate::projectiles::controls,
            combat,
            melee::swing,
            crate::projectiles::tick,
            crate::infected_population::death_loot,
            director,
            crate::loot::scavenge::update,
            crate::loot::bot_pickups,
            crate::survivor_items::bot_care,
            squad_ai,
            enemy_ai,
            crate::uncommon_behaviour::clown_steps,
        )
            .chain()
            .run_if(crate::game_loading::visible),
    );
    if std::env::args().any(|a| a == "--capture-break-fragments") {
        app.add_systems(FixedUpdate, crate::source_break_debris::capture.before(crate::l4d2r_cinematic::tick));
    }
    app.add_systems(Update, crate::infected_population::gallery);
    crate::game_loading::milestone(0.9);
    if std::env::args().any(|a| a == "--capture-simulate") {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1. / 60.),
        ));
    }
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .set_max_delta(std::time::Duration::from_secs_f64(0.1));
    app.add_systems(
        Update,
        crate::game_loading::ready
            .after(controls)
            .before(sync_camera),
    );
    crate::test_lab::install(&mut app);
    crate::ivp_playtest::install(&mut app);
    crate::special_infected::install(&mut app);
    app.add_systems(FixedUpdate, crate::special_infected::tick.after(enemy_ai));
    crate::parity::install(&mut app);
    crate::survivor_glow::install(&mut app);
    crate::survivor_names::install(&mut app);
    crate::infected_gore::install(&mut app);
    crate::infected_appearance::install(&mut app);
    crate::uncommon_behaviour::install(&mut app);
    crate::bile_screen::install(&mut app);
    crate::model_shadows::install(&mut app);
    crate::light_styles::install(&mut app);
    crate::ropes::install(&mut app);
    crate::env_sprites::install(&mut app);
    crate::detail_props::install(&mut app);
    crate::env_sprites::install_transform_debug(&mut app);
    crate::soundscapes::install(&mut app);
    crate::room_dsp::install(&mut app);
    crate::render_warmup::install(&mut app);
    crate::map_transition::install(&mut app);
    crate::game_console::install(&mut app);
    crate::dev_console::install(&mut app);
    crate::runtime_props::install(&mut app);
    crate::world_editor::install(&mut app);
    crate::edit_world_ui::install(&mut app);
    crate::glow_menu::install(&mut app);
    crate::game_chat::install(&mut app);
    crate::multiplayer::install(&mut app);
    crate::xbox_link::install(&mut app);
    crate::net_play::install(&mut app);
    crate::net_flow::install(&mut app);
    crate::survival::install(&mut app);
    crate::scavenge_rules::install(&mut app);
    crate::survivor_music::install(&mut app);
    app.add_systems(Update, crate::low_violence::update_labels);
    crate::net_infected::install(&mut app);
    app.init_resource::<game_menu::lobby::Lobby>().add_systems(
        Update,
        (game_menu::lobby::chat_tap, game_menu::lobby::chat_keys, game_menu::lobby::presentation, game_menu::lobby::typing_bar).chain(),
    );
    crate::scoreboard::install(&mut app);
    app.add_systems(Update, crate::bullet_impacts::update);
    app.add_systems(Update, crate::survivor_items::black_white);
    app.init_resource::<crate::campaign_flow::Checkpoint>()
        .add_systems(FixedUpdate, crate::campaign_flow::update.after(squad_ai))
        .add_systems(FixedUpdate, crate::campaign_flow::finale_won);
    #[cfg(target_os = "ios")]
    app.add_systems(Last, crate::ios_frame_status);
    app.run();
    Ok(())
}
pub(crate) fn carry_prepared(prepared: &mut Prepared, carry: crate::campaign_flow::Carry) {
    prepared.initial = crate::campaign_flow::carry_state(carry.player);
    for (member, state) in prepared.squad.0.iter_mut().zip(carry.bots) {
        member.state = crate::campaign_flow::carry_state(state);
    }
}
fn install_map_logic(world: &mut World, logic: crate::l4d2r_cinematic::MapLogic) {
    // Entity ids belong to this map. Menu starts and chapter changes must
    // replace the ladder registry together with the world they index.
    world.insert_resource(crate::ladders::Ladders::load(&logic.world));
    world.insert_resource(logic);
}
/// Phones: clear the outgoing map before the next one is prepared, so the
/// two never sit in memory together (iPhone campaign-change kill: Dead
/// Center's ~650 MiB of map data and ~330 MiB of GPU textures stayed loaded
/// while Dark Carnival was built on top). Deletes every scene object except
/// cameras (the loading overlay renders through them), drops the skybox and
/// the collision/navigation data, and resets the resources that refer to
/// the deleted objects, as `apply_prepared` does later.
pub(crate) fn clear_outgoing_map(world: &mut World) {
    clear_outgoing(world, false);
}
/// Exit to menu: the same release, but the menu UI and its music stay. The
/// old map's sounds stop and its objects go at once instead of when the
/// laboratory map behind the menu is applied seconds later.
pub(crate) fn clear_outgoing_world(world: &mut World) {
    clear_outgoing(world, true);
}
fn clear_outgoing(world: &mut World, keep_menu: bool) {
    let mut entities = world
        .query_filtered::<Entity, (
            Without<Window>,
            Without<bevy::ecs::resource::IsResource>,
            Without<Camera>,
            Or<(With<Transform>, With<Node>, With<AudioPlayer>)>,
        )>()
        .iter(world)
        .collect::<Vec<_>>();
    if keep_menu {
        entities.retain(|e| {
            let e = world.entity(*e);
            !e.contains::<Node>() && !e.contains::<game_audio::MenuMusic>()
        });
    }
    let count = entities.len();
    for entity in entities {
        if world.get_entity(entity).is_ok() {
            world.despawn(entity);
        }
    }
    let cameras: Vec<Entity> = world.query_filtered::<Entity, With<bevy::light::Skybox>>().iter(world).collect();
    for camera in cameras {
        world.entity_mut(camera).remove::<bevy::light::Skybox>();
    }
    crate::survivor_glow::reset(world);
    crate::infected_gore::reset(world);
    crate::source_particles::forget_materials(world);
    // Normal maps kept for the shader-detail toggle: keyed by material, so
    // every map's stayed loaded for the session (~37 MiB per map change;
    // the iPhone Hotel -> menu -> motel kill).
    if let Some(mut originals) = world.get_resource_mut::<crate::graphics_options::Originals>() {
        originals.0.clear();
    }
    world.insert_resource(Collision::default());
    world.insert_resource(crate::l4d2r_navigation::Navigation::default());
    // The old scene (BSP world, model and material data) is the largest
    // part of a prepared map: ~370 MiB for c2m1 on PC.
    world.remove_resource::<crate::viewer::Map>();
    // The old map's lightmap atlas (32 MiB on c1m1).
    world.remove_resource::<crate::light_styles::Lightmap>();
    world.insert_resource(Input::default());
    world.insert_resource(AudioHistory::default());
    // Keep a menu-music request queued this frame (EXIT TO MAIN MENU).
    let menu_music: std::collections::VecDeque<_> = if keep_menu {
        world.resource_mut::<game_audio::Queue>().0.drain(..).filter(|r| r.looping && r.map_entity.is_none() && r.position.is_none()).collect()
    } else {
        Default::default()
    };
    world.insert_resource(game_audio::Queue(menu_music, vec![]));
    world.insert_resource(crate::projectiles::Projectiles::default());
    world.insert_resource(crate::l4d2r_animation::PlaybackState::default());
    println!("Map release: cleared {count} outgoing map objects{}", if keep_menu { " (exit to menu)" } else { "" });
}
pub(crate) fn apply_prepared(world: &mut World, prepared: Prepared, root: &Path) {
    use bevy::ecs::system::RunSystemOnce;
    let Prepared {
        session,
        catalog,
        selection,
        collision,
        native_audio,
        native_director,
        mut initial,
        rules,
        squad,
        animations,
        map_logic,
        campaign,
        scene,
    } = prepared;
    let path = session.map.clone();
    initial.paused = true;
    // Delete scene objects, UI and playing sounds, while retaining Bevy's
    // window, resources, observers and registered system entities.
    let entities = world
        .query_filtered::<Entity, (
            Without<Window>,
            Without<bevy::ecs::resource::IsResource>,
            Or<(With<Transform>, With<Node>, With<AudioPlayer>)>,
        )>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in entities {
        if world.get_entity(entity).is_ok() {
            world.despawn(entity);
        }
    }
    if std::env::var_os("L4D2R_ENTITY_AUDIT").is_some() {
        let sizes: Vec<usize> = {
            let mut v: Vec<usize> = world.archetypes().iter().map(|a| a.len() as usize).filter(|n| *n > 0).collect();
            v.sort_unstable_by(|a, b| b.cmp(a));
            v.truncate(12);
            v
        };
        println!("ENTITY AUDIT after old-map despawn: {} entities; largest groups {sizes:?}", world.entities().len());
    }
    world.remove_resource::<crate::menu_video::Video>();
    crate::survivor_glow::reset(world);
    crate::infected_gore::reset(world);
    world.insert_resource(session);
    world.insert_resource(catalog);
    world.insert_resource(selection);
    let audit = |stage: &str| if std::env::var_os("L4D2R_ENTITY_AUDIT").is_some() { crate::performance::release_freed_memory(stage); };
    audit("AUDIT despawned");
    world.insert_resource(collision);
    audit("AUDIT collision replaced");
    world.insert_resource(native_audio);
    world.insert_resource(native_director);
    world.insert_resource(initial);
    world.insert_resource(rules);
    world.insert_resource(squad);
    world.insert_resource(animations);
    audit("AUDIT rules/squad/animations replaced");
    install_map_logic(world, map_logic);
    audit("AUDIT map logic replaced");
    world.insert_resource(campaign);
    world.insert_resource(game_ui::HudSource(root.to_path_buf()));
    world.insert_resource(game_menu::Frontend {
        page: game_menu::Page::Game,
        mode: 0,
        restart: false,
        started: true,
    });
    world.insert_resource(Input::default());
    world.insert_resource(AudioHistory::default());
    world.insert_resource(game_audio::Queue::default());
    world.insert_resource(crate::projectiles::Projectiles::default());
    world.insert_resource(crate::l4d2r_animation::PlaybackState::default());
    world.insert_resource(crate::l4d2r_navigation::Navigation::load(&path));
    audit("AUDIT navigation replaced");
    world.insert_resource(crate::campaign_flow::Checkpoint::default());
    crate::viewer::replace_scene(world, scene);
    audit("AUDIT scene replaced");
    world
        .run_system_once(initialize)
        .expect("Initialize existing-window game");
    world.run_system_once(game_ui::setup).expect("Rebuild HUD");
    world
        .run_system_once(crate::scoreboard::setup)
        .expect("Rebuild scoreboard");
    world
        .run_system_once(crate::graphics_options::setup_grain)
        .expect("Rebuild film grain");
    crate::render_warmup::begin(world);
    world
        .run_system_once(crate::l4d2r_cinematic::setup)
        .expect("Rebuild cinematic UI");
    world
        .run_system_once(crate::gun_effects::setup)
        .expect("Rebuild weapon effects");
    world
        .run_system_once(crate::precipitation::setup)
        .expect("Rebuild precipitation");
    world
        .run_system_once(crate::source_particles::setup)
        .expect("Rebuild effects");
    world
        .run_system_once(game_ui::target_camera)
        .expect("Target HUD camera");
    touch::rebuild(world);
    world
        .run_system_once(game_audio::setup)
        .expect("Rebuild sounds");
    world
        .run_system_once(game_menu::setup)
        .expect("Rebuild menus");
    crate::game_console::rebuild(world);
    world
        .run_system_once(crate::upgrades::setup)
        .expect("Rebuild laser effects");
    world
        .run_system_once(crate::bullet_impacts::setup)
        .expect("Rebuild impact pool");
    world
        .run_system_once(crate::performance::setup)
        .expect("Rebuild FPS display");
    if crate::test_lab::active(&path) {
        crate::test_lab::rebuild(world);
    }
    world.resource_mut::<Match>().paused = false;
    for mut cursor in world.query::<&mut CursorOptions>().iter_mut(world) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}
fn initialize(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    campaign: Res<Campaign>,
    rules: Res<Rules>,
    models: Res<viewer::RenderModels>,
    collision: Res<Collision>,
    rendered: Query<(
        Entity,
        &Mesh3d,
        &Transform,
        &crate::l4d2r_cinematic::MapEntity,
    )>,
    camera: Single<
        (
            Entity,
            Option<&bevy::camera::RenderTarget>,
            Option<&bevy::post_process::auto_exposure::AutoExposure>,
        ),
        With<viewer::GameCamera>,
    >,
    squad: Res<Squad>,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
) {
    let (camera, target, _exposure) = *camera;
    for (id, e) in logic.world.ents.iter().enumerate() {
        if e.me.classname != "info_survivor_position" || e.me.kv("survivorintrosequence").is_none()
        {
            continue;
        }
        let Some(survivor) =
            e.me.kv("survivorname")
                .and_then(crate::campaign_settings::Survivor::parse)
        else {
            continue;
        };
        let model = format!("models/survivors/survivor_{}.mdl", survivor.character());
        for mesh_model in [&model, &rules.pistol.player_model] {
            if let Some(draws) = models.0.get(&(mesh_model.clone(), 0, 0)) {
                for (draw, (source, material)) in draws.iter().enumerate() {
                    let Some(mesh) = meshes.get(source).cloned() else {
                        continue;
                    };
                    let mesh = meshes.add(mesh);
                    commands.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material.clone()),
                        crate::viewer::DynamicModelLighting::default(),
                        crate::l4d2r_cinematic::transform(logic.world.world_matrix(id)),
                        Visibility::Hidden,
                        RenderLayers::layer(1),
                        crate::l4d2r_cinematic::IntroDraw {
                            ent: id,
                            model: model.clone(),
                            mesh_model: mesh_model.clone(),
                            draw,
                        },
                    ));
                }
            }
        }
    }
    let mut view = commands.spawn((
        Camera3d::default(),
        bevy::camera::Hdr,
        Msaa::Sample4,
        Camera {
            order: 1,
            // This transparent overlay must not copy the world camera's
            // single-sample target into its multisample foreground buffer.
            msaa_writeback: bevy::camera::MsaaWriteback::Off,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: bevy::camera::CameraOutputMode::Write {
                // Resolved foreground RGB already contains coverage/alpha.
                // Preserve additive particles whose RGB is nonzero but alpha is
                // zero, and avoid applying MSAA edge coverage a second time.
                blend_state: Some(
                    bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                ),
                clear_color: ClearColorConfig::None,
            },
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: crate::performance::viewmodel_vertical_fov(51.),
            near: 0.01,
            far: 10.,
            ..default()
        }),
        // Source draws the view model with the world's tonemap scale; it has
        // no exposure of its own (native_tonemap::share_world_exposure).
        bevy::camera::Exposure { ev100: 0. },
        crate::native_tonemap::WORLD_TONEMAPPING,
        Transform::default(),
        RenderLayers::layer(2),
        ViewCamera,
        ChildOf(camera),
    ));
    if let Some(target) = target {
        view.insert(target.clone());
    }
    commands.entity(camera).insert(Body {
        feet: campaign.spawn,
        velocity: Vec3::ZERO,
        grounded: false,
        crouched: false,
        stance: default(),
        previous_feet: campaign.spawn,
        previous_eye: EYE,
        contact_normal: Vec3::ZERO,
        yaw: campaign.angles[1].to_radians() - std::f32::consts::FRAC_PI_2,
        pitch: -campaign.angles[0].to_radians(),
    });
    for (map_id, class, origin) in &campaign.pickups {
        let id = Some(*map_id);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut draws = vec![];
        if let Some(id) = id.filter(|id| logic.world.ents[*id].parent.is_none()) {
            for (entity, mesh, t, map_id) in &rendered {
                if map_id.0 != id {
                    continue;
                }
                draws.push(entity);
                if let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = meshes
                    .get(&mesh.0)
                    .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
                {
                    for p in positions {
                        let p = t.transform_point(Vec3::from_array(*p)) - *origin;
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
        }
        let placed = crate::loot::spawn_position(&collision, class, *origin, lo, hi);
        let offset = crate::loot::PickupPlacement(placed - *origin);
        if std::env::args().any(|a| a == "--audit-pickup-placement") {
            println!(
                "Pickup placement: {class} original={origin:?} root={placed:?} lo={lo:?} hi={hi:?} draws={}",
                draws.len()
            );
        }
        let center = if lo.is_finite() && hi.is_finite() {
            (lo + hi) * 0.5
        } else {
            Vec3::ZERO
        };
        let pickup = commands
            .spawn((
                Pickup {
                    class: class.clone(),
                    origin: placed + center,
                },
                crate::loot::PickupMapEntity(*map_id),
                Transform::from_translation(placed),
                Visibility::Inherited,
                Name::new(format!("{class}_pickup")),
                crate::loot::stock(if class == "upgrade_laser_sight" {
                    -1
                } else {
                    logic
                        .world
                        .ents
                        .get(*map_id)
                        .and_then(|e| e.me.kv_i32("count"))
                        .unwrap_or(1)
                }),
            ))
            .id();
        if matches!(
            class.as_str(),
            "upgrade_ammo_incendiary" | "upgrade_ammo_explosive"
        ) {
            commands
                .entity(pickup)
                .insert(crate::upgrades::Claims::default());
        }
        for entity in draws {
            commands
                .entity(entity)
                .insert((offset, crate::loot::PickupOwner(pickup)));
        }
    }
    for (index, member) in squad.0.iter().enumerate() {
        if let Some(draws) = models.0.get(&(member.model.clone(), 0, 0)) {
            let actor = commands
                .spawn((
                    Transform::from_translation(member.position),
                    Visibility::default(),
                    CompanionActor(index),
                ))
                .id();
            for (mesh, material) in draws {
                commands.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    crate::viewer::DynamicModelLighting::default(),
                    Transform::default(),
                    RenderLayers::layer(1),
                    ChildOf(actor),
                ));
            }
            if let Some(gun) = models.0.get(&(rules.pistol.player_model.clone(), 0, 0)) {
                for (index, (mesh, material)) in gun.iter().enumerate() {
                    // Native player models merge matching weapon bones with the
                    // survivor skeleton. Each bot needs its own animated mesh.
                    let Some(original) = meshes.get(mesh).cloned() else {
                        continue;
                    };
                    let mesh = meshes.add(original);
                    commands.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material.clone()),
                        crate::viewer::DynamicModelLighting::default(),
                        Transform::default(),
                        CompanionGun(rules.pistol.class.clone()),
                        crate::l4d2r_animation::AnimatedDraw {
                            root: actor,
                            model: rules.pistol.player_model.clone(),
                            draw: index,
                        },
                        NoFrustumCulling,
                        RenderLayers::layer(1),
                        ChildOf(actor),
                    ));
                }
            }
        }
    }
    let (path, transform) = weapon_display(&rules.pistol);
    if let Some(draws) = models.0.get(&(path.into(), 0, 0)) {
        let weapon = commands
            .spawn((
                transform,
                Visibility::default(),
                HeldWeapon(rules.pistol.class.clone()),
                ChildOf(camera),
            ))
            .id();
        for (mesh, material) in draws {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                crate::viewer::DynamicModelLighting::default(),
                Transform::default(),
                RenderLayers::layer(2),
                NoFrustumCulling,
                ChildOf(weapon),
            ));
        }
    }
}
pub(crate) fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mouse_state: (
        Res<AccumulatedMouseMotion>,
        Local<Option<bool>>,
        MessageReader<MouseWheel>,
        Local<f32>,
        Local<bool>,
    ),
    mut windows: Query<(&Window, &mut CursorOptions)>,
    mut s: ResMut<Match>,
    mut input: ResMut<Input>,
    mut body: Single<&mut Body>,
    mut campaign: ResMut<Campaign>,
    rules: Res<Rules>,
    mut commands: Commands,
    enemies: Query<Entity, With<Enemy>>,
    mut squad: ResMut<Squad>,
    pickups: Query<Entity, With<Pickup>>,
    mut rendered_pickups: Query<(
        Entity,
        &Name,
        Option<&mut crate::loot::Stock>,
        Option<&crate::loot::Dropped>,
    )>,
    mut frontend: ResMut<game_menu::Frontend>,
    runtime: (
        Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
        Option<ResMut<crate::source_director::Director>>,
        Option<Res<crate::game_console::Console>>,
        Option<Res<touch::TouchControls>>,
    ),
) {
    let (motion, mut focus, mut wheel, mut wheel_remainder, mut refocus) = mouse_state;
    // Drain even while menus/focus block input, so it cannot replay on resume.
    let wheel_delta: f32 = wheel
        .read()
        .filter(|e| e.y.is_finite())
        .map(|e| match e.unit {
            MouseScrollUnit::Line => e.y,
            MouseScrollUnit::Pixel => e.y / 40.,
        })
        .sum();
    *wheel_remainder = if !s.paused && !s.dead && !s.incap {
        *wheel_remainder + wheel_delta
    } else {
        0.
    };
    let (mut cinematic, mut native_director, console, touch_controls) = runtime;
    // `--capture-restart-at <seconds>`: one scripted map restart (captures
    // of effects and state after a restart).
    {
        static RESTART_AT: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let at = *RESTART_AT.get_or_init(|| {
            let args: Vec<String> = std::env::args().collect();
            args.windows(2).find(|w| w[0] == "--capture-restart-at").and_then(|w| w[1].parse().ok())
        });
        if at.is_some_and(|t| s.elapsed >= t) && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            println!("Capture: restarting the map at {:.2}s", s.elapsed);
            frontend.restart = true;
        }
        // `--capture-use-at <s>`: press use once (opening cabinets/doors).
        // A comma list presses use at each time.
        static USE_AT: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
        static USED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let at = USE_AT.get_or_init(|| {
            let args: Vec<String> = std::env::args().collect();
            args.windows(2)
                .find(|w| w[0] == "--capture-use-at")
                .map(|w| w[1].split(',').filter_map(|t| t.trim().parse().ok()).collect())
                .unwrap_or_default()
        });
        let next = USED.load(std::sync::atomic::Ordering::Relaxed);
        if at.get(next).is_some_and(|&t| s.elapsed >= t) {
            USED.store(next + 1, std::sync::atomic::Ordering::Relaxed);
            println!("Capture: pressing use at {:.2}s", s.elapsed);
            input.use_item = true;
        }
    }
    // Before the window lookup so windowless captures can restart too;
    // still only while playing with the console closed, as before.
    // Online a dead player waits for the team (net_flow restarts everyone).
    if ((keys.just_pressed(KeyCode::Enter) && s.dead && !crate::net_flow::online()) || frontend.restart)
        && frontend.playing()
        && !console.as_ref().is_some_and(|c| c.open)
    {
        frontend.restart = false;
        commands.queue(|world: &mut World| crate::infected_gore::reset(world));
        for e in &enemies {
            commands.entity(e).despawn();
        }
        if let Some(logic) = cinematic.as_mut() {
            logic.restart();
        }
        *s = Match::new(&rules.pistol);
        body.feet = campaign.spawn;
        body.velocity = Vec3::ZERO;
        body.grounded = true;
        body.crouched = false;
        body.stance = default();
        body.yaw = campaign.angles[1].to_radians() - std::f32::consts::FRAC_PI_2;
        body.pitch = 0.;
        if let Some(d) = native_director.as_mut() {
            d.reset();
            d.flow_start(campaign.spawn);
            if let Some(logic) = cinematic.as_ref() {
                d.sync_world(&logic.world);
            }
        }
        campaign.total_spawned = 0;
        campaign.population.restart();
        for entity in &pickups {
            commands.entity(entity).remove::<Taken>();
        }
        for (entity, name, stock, dropped) in &mut rendered_pickups {
            if dropped.is_some() {
                commands.entity(entity).despawn();
                continue;
            }
            if let Some(mut stock) = stock {
                stock.remaining = stock.initial;
            }
            if name.as_str().starts_with("weapon_") {
                commands
                    .entity(entity)
                    .remove::<Taken>()
                    .insert(Visibility::Inherited);
            }
        }
        crate::game_state::clear_kicked();
        for member in &mut squad.0 {
            member.state = Match::new(&rules.pistol);
            member.position = member.spawn;
            member.vertical = 0.;
            member.cooldown = 0.;
            member.revive_time = 0.;
        }
    }
    let Some((window, mut cursor)) = windows.iter_mut().next() else {
        return;
    };
    let changed = focus.is_some_and(|previous| previous != window.focused);
    *focus = Some(window.focused);
    // Losing focus does not pause (the game keeps running, as in retail, so
    // a map started in a background window still fades in and online play
    // continues). Input is ignored until a click back in, which only
    // recaptures the mouse.
    if changed {
        *wheel_remainder = 0.;
        *input = Input::default();
        *refocus = window.focused;
    }
    if frontend.in_menu() || console.as_ref().is_some_and(|c| c.open) {
        *wheel_remainder = 0.;
        // Online the world keeps running under the menu; only our input stops.
        s.paused = !crate::game_menu::keep_running();
        if !cursor.visible {
            cursor.visible = true;
        }
        if cursor.grab_mode != CursorGrabMode::None {
            cursor.grab_mode = CursorGrabMode::None;
        }
        *input = Input::default();
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && !crate::game_menu::keep_running() {
        s.paused = true;
    }
    // EDIT WORLD (Hammer / Roblox Studio camera): the cursor stays free for
    // the editor's panels and only turns the view while RMB is held.
    let edit_cursor = crate::world_editor::edit_cursor();
    if edit_cursor.is_some() && !keys.just_pressed(KeyCode::Escape) {
        s.paused = false;
        *refocus = false;
    }
    let start = (s.paused || *refocus) && mouse.just_pressed(MouseButton::Left) && window.focused && !s.dead;
    if start {
        s.paused = false;
        *refocus = false;
    }
    let away = !window.focused || *refocus;
    let visible = s.paused || s.dead || away || edit_cursor == Some(false);
    // iOS rejects cursor grabs and Bevy restores the previous mode. Retrying
    // it each frame makes capture_changed permanently true, dropping all look
    // deltas. Touch dragging never requires a captured desktop cursor.
    let touch_look = cfg!(any(target_os = "ios", target_os = "android"))
        || touch_controls.as_ref().is_some_and(|c| c.enabled);
    let mode = if visible || touch_look {
        CursorGrabMode::None
    } else {
        if cfg!(windows) {
            CursorGrabMode::Confined
        } else {
            CursorGrabMode::Locked
        }
    };
    let capture_changed = cursor.grab_mode != mode;
    if cursor.visible != visible {
        cursor.visible = visible;
    }
    if capture_changed {
        cursor.grab_mode = mode;
    }
    if s.paused || s.dead || away {
        *wheel_remainder = 0.;
        *input = Input::default();
        return;
    }
    if cinematic.as_ref().is_some_and(|c| c.active) {
        *input = Input::default();
        *wheel_remainder = 0.;
        return;
    }
    if !changed && !capture_changed && edit_cursor != Some(false) {
        body.yaw -= motion.delta.x * 0.002;
        body.pitch = (body.pitch - motion.delta.y * 0.002).clamp(-1.5, 1.5);
    }
    // EDIT WORLD is a spectator camera: only the look above (RMB held) and
    // noclip flying (move_player's WASD). No firing, shoving, use, reload,
    // slot keys or mouse-wheel weapon switching (the wheel scrolls the
    // editor's browser).
    if edit_cursor.is_some() {
        *wheel_remainder = 0.;
        *input = Input::default();
        return;
    }
    input.sample_primary(
        mouse.pressed(MouseButton::Left) && !start,
        mouse.just_pressed(MouseButton::Left) && !start,
    );
    input.shove |= mouse.just_pressed(MouseButton::Right);
    input.shove_held = mouse.pressed(MouseButton::Right);
    input.reload |= keys.just_pressed(KeyCode::KeyR);
    input.jump |= keys.just_pressed(KeyCode::Space);
    input.use_item |= keys.just_pressed(KeyCode::KeyE);
    for (key, slot) in [
        (KeyCode::Digit1, 1),
        (KeyCode::Digit2, 2),
        (KeyCode::Digit3, 3),
        (KeyCode::Digit4, 4),
        (KeyCode::Digit5, 5),
    ] {
        if keys.just_pressed(key) {
            s.select_inventory_slot(slot);
        }
    }
    if changed || capture_changed || start {
        *wheel_remainder = 0.;
    } else {
        let steps = wheel_remainder.trunc().clamp(-20., 20.) as i32;
        *wheel_remainder -= wheel_remainder.trunc();
        // Native default bindings: MWHEELUP invprev, MWHEELDOWN invnext.
        for _ in 0..steps.abs() {
            s.cycle_inventory(-steps.signum());
        }
    }
}
fn begin_player_tick(mut bodies: Query<&mut Body>) {
    for mut body in &mut bodies {
        body.previous_feet = body.feet;
        body.previous_eye = body.stance.eye();
    }
}
pub(crate) fn move_player(
    console: Option<Res<crate::game_console::Console>>,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    c: Res<Collision>,
    rules: Res<Rules>,
    mut s: ResMut<Match>,
    mut input: ResMut<Input>,
    mut body: Single<&mut Body>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    mut fixture: Option<ResMut<crate::parity::MovementFixture>>,
    native: Option<Res<crate::source_director::Director>>,
    mut audio: Option<ResMut<game_audio::Queue>>,
    ladders: Option<Res<crate::ladders::Ladders>>,
    mut ladder: Local<crate::ladders::State>,
    skate: Option<Res<crate::skate_mod::Skate>>,
    mut ledge: (Option<ResMut<crate::ledge_hang::Tracker>>, Option<Res<crate::game_state::Squad>>),
) {
    let _timing = crate::performance::scope("game::move_player");
    // The skate mod moves the survivor while they ride (skate_mod::ride).
    if skate.is_some_and(|s| s.riding) {
        return;
    }
    if let Some(tracker) = ledge.0.as_deref_mut() {
        let body = &mut **body;
        if crate::ledge_hang::hold(
            tracker,
            &mut body.feet,
            &mut body.velocity,
            &mut body.grounded,
            &mut s,
            audio.as_deref_mut(),
        ) {
            return;
        }
    }
    let c = c.for_actor(s.infected, s.special_phase == 5);
    if s.paused || s.dead || s.incap || s.pinned || cinematic.as_ref().is_some_and(|c| c.active) {
        return;
    }
    let dt = time.delta_secs();
    let fall_settings=native.as_ref().map_or_else(Default::default,|d|d.fall_damage);
    let was_grounded=body.grounded;
    let mut impact_speed=0.;
    if console.as_ref().is_some_and(|c| c.noclip) {
        ladder.reset();
        let mut wish = Vec3::ZERO;
        let fixture_fly = fixture.as_ref().is_some_and(|f| f.case == "walklong");
        for (key, axis) in [
            (KeyCode::KeyW, Vec3::NEG_Z),
            (KeyCode::KeyS, Vec3::Z),
            (KeyCode::KeyA, Vec3::NEG_X),
            (KeyCode::KeyD, Vec3::X),
        ] {
            if keys.pressed(key) || (fixture_fly && key == KeyCode::KeyW) {
                wish += axis;
            }
        }
        let rotation = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.);
        wish = rotation * wish;
        if keys.pressed(KeyCode::Space) {
            wish += Vec3::Y;
        }
        if keys.pressed(KeyCode::ControlLeft) {
            wish -= Vec3::Y;
        }
        let velocity = wish.normalize_or_zero()
            * if keys.pressed(KeyCode::ShiftLeft) || fixture_fly {
                20.
            } else {
                5.
            };
        body.feet += velocity * dt;
        // Retail noclip keeps its movement as velocity (the viewmodel walk
        // blend reads it); nothing else integrates it while noclipping.
        body.velocity = velocity;
        body.grounded = false;
        return;
    }
    let clock = s.elapsed;
    if let Some(f)=fixture.as_mut() {
        if f.case=="fall" && clock>=0.3 && !f.jumped {
            body.feet+=Vec3::Y*4.*UNIT;
            body.velocity.y=-640.*UNIT;
            body.grounded=false;
            f.jumped=true;
        }
    }
    let fixture_duck = fixture
        .as_ref()
        .is_some_and(|f| f.case == "duck" && (0.2..1.5).contains(&clock));
    let fixture_forward = fixture.as_ref().is_some_and(|f| match f.case.as_str() {
        "walk" => (0.3..0.9).contains(&clock),
        "stairs" => (0.3..1.5).contains(&clock),
        "ladder" => (0.3..2.8).contains(&clock),
        "slowhold" => clock < 3.,
        // Steady forward motion for viewmodel walk captures.
        "walklong" => clock > 0.3,
        "duck" => (0.6..1.2).contains(&clock),
        _ => false,
    });
    if let Some(f) = fixture.as_mut() {
        if f.case == "jump" && clock >= 0.3 && !f.jumped {
            input.jump = true;
            f.jumped = true;
        }
    }
    let duck = keys.pressed(KeyCode::ControlLeft) || fixture_duck;
    if body.stance.jump_duck && c.grounded(body.feet, DUCK_HEIGHT) {
        body.stance.jump_duck = false;
    }
    if body.stance.jump_duck && body.velocity.y < 0. && !duck {
        let lower = body.feet - Vec3::Y * crate::player_movement::JUMP_DUCK_SHIFT;
        if c.grounded(lower + Vec3::Y * body.velocity.y * dt * 2., HEIGHT)
            && c.clear_hull(lower, HEIGHT)
        {
            body.feet = lower;
            body.stance.jump_duck = false;
            body.stance.fraction = 0.;
        }
    }
    let can_stand = c.clear_hull(body.feet, HEIGHT);
    body.stance.update(duck, can_stand, dt);
    body.crouched = body.stance.fraction >= 0.999 || (body.crouched && body.stance.fraction > 0.);
    let mut height = if body.crouched { DUCK_HEIGHT } else { HEIGHT };
    // An upward launch still touching its support must not have velocity zeroed.
    body.grounded = body.velocity.y <= 140. * UNIT && c.grounded(body.feet, height);
    if !was_grounded && body.grounded { impact_speed=(-body.velocity.y/UNIT).max(0.); }
    let mut wish = Vec3::ZERO;
    for (key, v) in [
        (KeyCode::KeyW, Vec3::NEG_Z),
        (KeyCode::KeyS, Vec3::Z),
        (KeyCode::KeyA, Vec3::NEG_X),
        (KeyCode::KeyD, Vec3::X),
    ] {
        if keys.pressed(key) || (key == KeyCode::KeyW && fixture_forward) {
            wish += v;
        }
    }
    let mut speed = rules
        .weapons
        .get(&s.gun().class)
        .unwrap_or(&rules.pistol)
        .speed
        .min(220. * UNIT)
        // Original m_flMaxspeed follows the weapon (Chrome=220), rather than
        // the movement limit measured with pistol/SMG is 220, not their
        // generic script default of250. Duck scales movement wishes to one quarter.
        * if duck || body.crouched { 0.25 } else { 1. };
    if s.infected {
        speed = s.special_class.speed() * UNIT * if duck || body.crouched { 0.25 } else { 1. };
    }
    if s.adrenaline_time > 0. && !body.crouched && !duck {
        speed = 260. * UNIT;
    } else if !s.infected && s.health + s.temporary < 40. && !body.crouched {
        speed = speed.min(85. * UNIT);
    }
    // Original +speed (Shift) caps ground wishes at 85 Source units/sec.
    let walk_key = keys.pressed(KeyCode::ShiftLeft) || fixture.as_ref().is_some_and(|f| f.case == "slowhold");
    if walk_key {
        speed = speed.min(85. * UNIT);
    }
    if walk_key || duck || body.crouched {
        s.walk_flag = true;
    } else if body.velocity.xz().length() > 85. * UNIT {
        s.walk_flag = false;
    }
    // Retail: kits, the defibrillator and upgrade packs hold the survivor
    // still while in use; swallowing pills or injecting adrenaline does not
    // slow them.
    let held_still = s.backpack_action() || s.reviving_teammate();
    if held_still {
        speed = 0.;
        // Retail roots the survivor outright: no sliding to a stop, no jump.
        body.velocity.x = 0.;
        body.velocity.z = 0.;
        input.jump = false;
    } else if s.action > 0. && !matches!(s.medical_kind, 4 | 5) {
        speed *= 0.25;
    }
    if let (Some(ladders), Some(logic)) = (ladders.as_deref(), cinematic.as_deref()) {
        if !held_still && !(s.infected && matches!(s.special_phase, 2 | 3)) {
            let rotation = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.);
            let mut feet = body.feet;
            let mut velocity = body.velocity;
            let climbing = ladder.step(ladders, &logic.world, &c, &mut feet, &mut velocity,
                height, rotation * wish * (if s.special_phase == 5 { 200. } else { 100. } * UNIT),
                rotation * Vec3::NEG_Z, input.jump, s.infected, clock, dt);
            body.feet = feet;
            body.velocity = velocity;
            if ladder.jumped() { body.grounded = false; input.jump = false; }
            if climbing {
                body.grounded = false;
                body.contact_normal = Vec3::ZERO;
                input.jump = false;
                return;
            }
        } else { ladder.reset(); }
    }
    let target = Quat::from_rotation_y(body.yaw) * wish.normalize_or_zero() * speed;
    body.velocity = if s.infected && matches!(s.special_phase, 2 | 3) {
        body.velocity
    } else if duck || body.crouched {
        // Retail duck acceleration retains its75-unit maxspeed while the
        // movement wish is one quarter of normal speed (55 for a220 weapon).
        crate::player_movement::horizontal_with_acceleration(
            body.velocity,
            target,
            speed,
            75. * UNIT,
            body.grounded,
            dt,
        )
    } else {
        crate::player_movement::horizontal(body.velocity, target, speed, body.grounded, dt)
    };
    if body.grounded && input.jump {
        body.velocity.y = crate::player_movement::JUMP;
        body.grounded = false;
        if !body.crouched {
            body.feet.y += crate::player_movement::JUMP_DUCK_SHIFT;
            body.stance.jump_duck = true;
            body.stance.fraction = 1.;
            body.crouched = true;
            height = DUCK_HEIGHT;
        }
    }
    input.jump = false;
    if body.grounded {
        body.velocity.y = 0.;
    } else {
        body.velocity.y -= crate::player_movement::GRAVITY * dt * 0.5;
    }
    body.contact_normal = Vec3::ZERO;
    if s.infected && !body.grounded {
        let center = body.feet + Vec3::Y * height * 0.5;
        let hit = c.trace(
            center,
            center + body.velocity * dt,
            Vec3::new(RADIUS, height * 0.5, RADIUS),
        );
        if !hit.start_solid && hit.fraction < 1. {
            body.contact_normal = hit.normal;
        }
    }
    let falling_speed=(-body.velocity.y/UNIT).max(0.);
    let (mut next, contact_velocity) =
        c.walk_velocity(body.feet, body.velocity, dt, height, body.grounded);
    body.velocity = contact_velocity;
    if body.grounded {
        next = c.snap_ground(next, height);
    }
    if (next.y - body.feet.y - body.velocity.y * dt).abs() > 0.001 {
        body.velocity.y = 0.;
    } else if !body.grounded {
        body.velocity.y -= crate::player_movement::GRAVITY * dt * 0.5;
    }
    if !body.grounded && falling_speed>0. && c.grounded(next,height) {
        impact_speed=falling_speed;
        body.grounded=true;
        body.velocity.y=0.;
    }
    body.feet = next;
    let damage=fall_settings.land(&mut s,impact_speed);
    if damage>0. {
        if let Some(audio)=audio.as_mut() { audio.play("Player.FallDamage",Some(next)); }
    }
    if let Some(tracker) = ledge.0.as_deref_mut() {
        crate::ledge_hang::detect(
            tracker,
            &c,
            body.feet,
            body.velocity,
            body.grounded,
            &mut s,
            ledge.1.as_deref(),
            native.as_deref(),
            audio.as_deref_mut(),
        );
    }
}
/// Retail client.dll third-person camera used while a survivor heals,
/// defibrillates or deploys an upgrade pack: cam_idealdist 150 behind the
/// eye along the view angles (cam_idealpitch/yaw 0), kept out of walls by a
/// CAM_HULL_OFFSET (14) box trace (cam_collision 1), looking where the
/// player looks. Its height is therefore the survivor's own eye height.
pub(crate) fn action_camera(
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    collision: Option<&Collision>,
) -> Transform {
    const IDEAL_DIST: f32 = 150. * UNIT;
    const HULL: f32 = 14. * UNIT;
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.);
    let desired = eye + rotation * Vec3::Z * IDEAL_DIST;
    let fraction = collision.map_or(1., |c| c.trace(eye, desired, Vec3::splat(HULL)).fraction);
    Transform::from_translation(eye.lerp(desired, fraction)).with_rotation(rotation)
}
fn sync_camera(
    mut camera: Single<(&Body, &mut Transform), With<viewer::GameCamera>>,
    s: Res<Match>,
    fixed: Res<Time<Fixed>>,
    time: Option<Res<Time>>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    mut stairs: Local<crate::player_movement::StairView>,
    pin: Option<Res<crate::special_infected::Hunter>>,
    skate: Option<Res<crate::skate_mod::Skate>>,
) {
    let (body, transform) = &mut *camera;
    let interpolate = !s.paused && body.feet.distance_squared(body.previous_feet) < 1.;
    let alpha = if interpolate {
        fixed.overstep_fraction()
    } else {
        1.
    };
    let mut feet = body.previous_feet.lerp(body.feet, alpha);
    // Retail cam_idealdist view for backpack actions and for a Tank punch or
    // rock knockdown (client 1026c880 leaves first person; no pin camera).
    let knocked = s.pinned && s.pinned_by == Some(crate::special_infected::SpecialClass::Tank);
    let skating = skate.is_some_and(|k| k.riding) && !s.pinned;
    if (s.backpack_action() || s.reviving_teammate()) && !s.pinned || knocked || skating {
        let eye = feet + Vec3::Y * (body.previous_eye + (body.stance.eye() - body.previous_eye) * alpha);
        **transform = action_camera(eye, body.yaw, body.pitch, collision.as_deref());
        return;
    }
    if s.survivor_third_person() {
        **transform = crate::special_infected::victim_camera(
            feet,
            body.yaw,
            body.pitch,
            collision.as_deref(),
        );
        return;
    }
    if s.infected_third_person() {
        **transform = crate::special_infected::third_person_camera(
            s.special_class,
            pin.as_ref().is_some_and(|p| p.chase_camera),
            feet,
            body.yaw,
            body.pitch,
            collision.as_deref(),
        );
        return;
    }
    let eye = body.previous_eye + (body.stance.eye() - body.previous_eye) * alpha;
    let moving_support = body.grounded
        && body.previous_feet.y != body.feet.y
        && logic.as_ref().is_some_and(|logic| {
            logic.world.ents.iter().enumerate().any(|(id, e)| {
                let moving = matches!(
                    e.kind,
                    l4d2r_game_core::entities::Kind::Elevator { moving: true, .. }
                ) || matches!(
                    e.me.classname.as_str(),
                    "func_movelinear" | "func_tracktrain"
                );
                moving
                    && logic.bounds(id).is_some_and(|(lo, hi)| {
                        body.feet.x >= lo.x
                            && body.feet.x <= hi.x
                            && body.feet.z >= lo.z
                            && body.feet.z <= hi.z
                            && body.feet.y >= lo.y - 2. * UNIT
                            && body.feet.y <= hi.y + 2. * UNIT
                    })
            })
        });
    static SMOOTH: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let allow = *SMOOTH.get_or_init(|| !std::env::args().any(|a| a == "--no-stair-smoothing"))
        && !s.paused
        && !s.dead
        && !s.incap
        && !moving_support;
    feet.y = stairs.sample(
        feet,
        eye,
        body.grounded,
        allow,
        time.as_ref().map_or(1. / 60., |t| t.delta_secs()),
    );
    let translation = feet + Vec3::Y * if s.incap { s.downed_eye() } else { eye };
    transform
        .reborrow()
        .map_unchanged(|t| &mut t.translation)
        .set_if_neq(translation);
    transform
        .reborrow()
        .map_unchanged(|t| &mut t.rotation)
        .set_if_neq(Quat::from_euler(
            EulerRot::YXZ,
            body.yaw,
            (body.pitch + s.view_punch.to_radians()).clamp(-1.55, 1.55),
            0.,
        ));
}
fn companion_item(state: &Match) -> Option<&str> {
    if state.dead {
        return None;
    }
    if state.incap {
        return Some(&state.pistol.class);
    }
    if state.action > 0. {
        match state.medical_kind {
            1 | 2 | 5 => return Some("weapon_first_aid_kit"),
            3 | 6 => return Some("weapon_defibrillator"),
            7 => return Some("weapon_adrenaline"),
            8 => return Some("weapon_pain_pills"),
            _ => {}
        }
    }
    Some(&state.gun().class)
}
fn sync_companion_items(
    mut commands: Commands,
    squad: Res<Squad>,
    rules: Res<Rules>,
    models: Res<viewer::RenderModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    actors: Query<(Entity, &CompanionActor)>,
    held: Query<(Entity, &CompanionGun, &ChildOf)>,
) {
    for (actor, index) in &actors {
        let wanted = companion_item(&squad.0[index.0].state);
        let mut current = false;
        for (entity, item, parent) in &held {
            if parent.parent() != actor {
                continue;
            }
            if wanted == Some(item.0.as_str()) {
                current = true;
            } else {
                commands.entity(entity).despawn();
            }
        }
        if current {
            continue;
        }
        let Some(class) = wanted else {
            continue;
        };
        let Some(weapon) = rules.weapons.get(class) else {
            continue;
        };
        let path = if weapon.player_model.is_empty() {
            &weapon.model
        } else {
            &weapon.player_model
        };
        let Some(draws) = models.0.get(&(path.clone(), 0, 0)) else {
            continue;
        };
        for (index, (mesh, material)) in draws.iter().enumerate() {
            let Some(original) = meshes.get(mesh).cloned() else {
                continue;
            };
            commands.spawn((
                Mesh3d(meshes.add(original)),
                MeshMaterial3d(material.clone()),
                viewer::DynamicModelLighting::default(),
                Transform::default(),
                CompanionGun(class.into()),
                crate::l4d2r_animation::AnimatedDraw {
                    root: actor,
                    model: path.clone(),
                    draw: index,
                },
                NoFrustumCulling,
                RenderLayers::layer(1),
                ChildOf(actor),
            ));
        }
    }
}
#[cfg(test)]
mod companion_item_tests {
    use super::*;
    #[test]
    fn medical_actions_replace_the_gun_and_completion_and_incapacitation_restore_visibility() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        for (kind, item) in [
            (1, "weapon_first_aid_kit"),
            (2, "weapon_first_aid_kit"),
            (3, "weapon_defibrillator"),
            (5, "weapon_first_aid_kit"),
            (6, "weapon_defibrillator"),
            (7, "weapon_adrenaline"),
            (8, "weapon_pain_pills"),
        ] {
            state.action = 1.;
            state.medical_kind = kind;
            assert_eq!(companion_item(&state), Some(item));
            state.action = 0.;
            assert_eq!(companion_item(&state), Some("weapon_pistol"));
        }
        state.incap = true;
        assert_eq!(companion_item(&state), Some("weapon_pistol"));
        state.incap = false;
        state.dead = true;
        assert!(companion_item(&state).is_none());
    }
}
fn sync_weapon(
    mut commands: Commands,
    s: Res<Match>,
    rules: Res<Rules>,
    models: Res<viewer::RenderModels>,
    camera: Single<Entity, With<viewer::GameCamera>>,
    mut held: Query<(Entity, &HeldWeapon, &mut Visibility)>,
) {
    let class = s.held_class();
    let mut matching = false;
    for (entity, weapon, mut visibility) in &mut held {
        if weapon.0 == class {
            matching = true;
            *visibility = if !s.dead && !s.infected_arms_hidden() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        } else {
            commands.entity(entity).despawn();
        }
    }
    if matching {
        return;
    }
    let weapon = rules.weapons.get(class).unwrap_or(&rules.pistol);
    let (path, transform) = weapon_display(weapon);
    if let Some(draws) = models.0.get(&(path.into(), 0, 0)) {
        let root = commands
            .spawn((
                transform,
                if !s.dead && !s.infected_arms_hidden() {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
                HeldWeapon(class.to_owned()),
                ChildOf(*camera),
            ))
            .id();
        for (mesh, material) in draws {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                crate::viewer::DynamicModelLighting::default(),
                Transform::default(),
                RenderLayers::layer(2),
                NoFrustumCulling,
                ChildOf(root),
            ));
        }
    }
}
fn timers(
    time: Res<Time<Fixed>>,
    mut s: ResMut<Match>,
    bodies: Query<&Body, With<crate::viewer::GameCamera>>,
    rules: Res<Rules>,
    native: Option<Res<crate::source_director::Director>>,
) {
    if s.paused || s.dead {
        return;
    }
    let dt = time.delta_secs();
    s.elapsed += dt;
    let accuracy = rules
        .weapons
        .get(s.held_class())
        .unwrap_or(&rules.pistol)
        .accuracy;
    // Retail spread eases toward its stance/movement target (103e8e60).
    let penalties = crate::ballistics::Penalties {
        incap: s.incap,
        magnum: s.held_class() == "weapon_pistol_magnum",
        dragged: s.pinned && s.pinned_by == Some(crate::special_infected::SpecialClass::Smoker),
        laser: s.gun().laser,
    };
    let target = bodies.single().map_or(accuracy.stand, |body| {
        accuracy.target(body.grounded, body.crouched, body.velocity.length() / UNIT, penalties)
    });
    s.accuracy_bloom = accuracy.ease(s.accuracy_bloom, target, dt);
    s.view_punch = decay_punch(s.view_punch, dt);
    s.post_pills = (s.post_pills - dt).max(0.);
    s.post_healed = (s.post_healed - dt).max(0.);
    s.cooldown = (s.cooldown - dt).max(0.);
    s.shove = (s.shove - dt).max(0.);
    s.damage_flash = (s.damage_flash - dt).max(0.);
    s.temporary = (s.temporary - dt * native.as_ref().map_or(0.27, |d| d.temporary_decay)).max(0.);
    if s.incap {
        s.incap_health = (s.incap_health - dt * 3.).max(0.);
        // A hanging survivor lets go instead (ledge_hang::hold).
        s.dead = s.incap_health <= 0. && s.ledge.is_none();
    }
    if s.reload > 0. {
        let weapon = rules.weapons.get(&s.gun().class).unwrap_or(&rules.pistol);
        s.advance_reload(weapon, dt);
    }
}
// Select the item under the player's aim, within Source's short use reach.
// The tolerance accounts for origins at the bottom of pickup world models.
pub(crate) fn pickup_aim_score(eye: Vec3, forward: Vec3, origin: Vec3) -> Option<f32> {
    let delta = origin + Vec3::Y * 0.12 - eye;
    let distance = delta.length();
    let aim = forward.dot(delta.normalize_or_zero());
    // Retail cvars (live, 25 Nov): player_use_radius 96 units and
    // z_use_tolerance 0.82. The old 0.45 cone (~63 degrees) focused a
    // c3m1 medkit far off the crosshair: prompt, outline and the
    // item-reach hand where retail shows none.
    const USE_RADIUS: f32 = 96. * 0.0254;
    const USE_TOLERANCE: f32 = 0.82;
    (distance < USE_RADIUS && aim >= USE_TOLERANCE).then_some(aim - distance * 0.025)
}
fn interact(
    mut commands: Commands,
    mut s: ResMut<Match>,
    mut input: ResMut<Input>,
    rules: Res<Rules>,
    collision: Res<Collision>,
    body: Single<&Body>,
    pickups: Query<(Entity, &Pickup, Option<&crate::loot::PickupMapEntity>), Without<Taken>>,
    session: Res<crate::campaign_settings::SessionSettings>,
    rendered: Query<
        (Entity, &Name, &Transform, Option<&crate::loot::PickupOwner>),
        Without<Pickup>,
    >,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut stocks: Query<&mut crate::loot::Stock>,
    dropped: Query<&crate::loot::Dropped>,
    models: Option<Res<viewer::RenderModels>>,
    mesh_assets: Option<Res<Assets<Mesh>>>,
    mut upgrade_claims: Query<&mut crate::upgrades::Claims>,
    cabinet: (
        Option<ResMut<crate::game_audio::Queue>>,
        Option<Res<crate::l4d2r_navigation::Navigation>>,
        Query<&crate::loot::PickupInside>,
    ),
) {
    let (mut audio, navigation, inside) = cabinet;
    let _timing = crate::performance::scope("game::interact");
    // Script-spawned scavenge item spawners (c14m2's finale gas cans): their
    // SpawnItem creates a gas can bound to the spawner, whose pickup fires
    // its OnItemPickedUp.
    if let Some(logic) = cinematic.as_mut() {
        for id in std::mem::take(&mut logic.world.cine.item_spawns) {
            let e = &logic.world.ents[id];
            let origin = Vec3::from_array(crate::binary::source_position(e.me.origin.to_array()));
            let rotation = Quat::from_rotation_y(e.me.angles.y.to_radians());
            let item = crate::loot::place_item(
                &mut commands,
                models.as_deref(),
                mesh_assets.as_deref(),
                &rules,
                "weapon_gascan",
                Transform::from_translation(origin + Vec3::Y * 0.02).with_rotation(rotation),
            );
            commands.entity(item).insert(crate::loot::PickupMapEntity(id));
        }
    }
    if s.infected || s.paused || s.dead || s.incap || s.pinned || cinematic.as_ref().is_some_and(|c| c.active) {
        return;
    }
    let eye = body.feet + Vec3::Y * EYE;
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let nearest = pickups
        .iter()
        .filter(|(e, p, binding)| {
            if !crate::loot::pickup_active(*binding, cinematic.as_deref()) {
                return false;
            }
            if upgrade_claims.get(*e).is_ok_and(|c| c.0[0]) {
                return false;
            }
            if p.class == "upgrade_laser_sight" && s.primary.as_ref().is_none_or(|g| g.laser) {
                return false;
            }
            if stocks.get(*e).is_ok_and(|s| s.remaining == 0) {
                return false;
            }

            if p.class == "weapon_pistol" && s.pistol.class == "weapon_pistol_dual" {
                return false;
            }
            let equipped = p.class != "weapon_pistol"
                && p.class == s.gun().class
                && rules.weapons.get(&p.class).is_none_or(|w| {
                    s.gun().ammo >= w.clip && s.gun().reserve >= w.reserve_capacity()
                });
            !(equipped
                || p.class == "weapon_first_aid_kit" && s.medkit
                || p.class == "weapon_pain_pills" && s.pills
                || p.class == "weapon_adrenaline" && s.adrenaline
                || p.class == "weapon_defibrillator" && s.defibrillator)
        })
        .filter_map(|(entity, p, _)| {
            pickup_aim_score(eye, forward, p.origin).map(|score| (entity, p, score))
        })
        .filter(|(entity, p, _)| {
            let target = p.origin + Vec3::Y * 0.12;
            let binding = pickups.get(*entity).ok().and_then(|(_, _, b)| b);
            // Items inside a prop (medical cabinets) ignore that prop.
            let owner = binding.map(|b| b.0).or_else(|| inside.get(*entity).ok().map(|i| i.0));
            owner
                .map_or_else(
                    || collision.trace(eye, target, Vec3::ZERO),
                    |o| collision.prop_trace(o, eye, target, Vec3::ZERO),
                )
                .fraction
                >= 0.99
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(entity, p, _)| (entity, p));
    if s.carried.is_none() {
    s.hint = nearest
        .map(|(_, p)| {
            format!(
                "E · Pick up {}",
                match p.class.as_str() {
                    "upgrade_laser_sight" => "laser sight".into(),
                    "upgrade_ammo_incendiary" => "incendiary rounds".into(),
                    "upgrade_ammo_explosive" => "explosive rounds".into(),
                    "weapon_upgradepack_incendiary" => "incendiary ammo pack".into(),
                    "weapon_upgradepack_explosive" => "explosive ammo pack".into(),
                    _ => p.class.trim_start_matches("weapon_").replace('_', " "),
                }
            )
        })
        .unwrap_or_default();
    }
    if input.use_item && std::env::var_os("L4D2R_USE_LOG").is_some() {
        for (entity, p, binding) in &pickups {
            if p.origin.distance(eye) > 3. {
                continue;
            }
            let target = p.origin + Vec3::Y * 0.12;
            let owner = binding.map(|b| b.0).or_else(|| inside.get(entity).ok().map(|i| i.0));
            let reach = owner
                .map_or_else(|| collision.trace(eye, target, Vec3::ZERO), |o| collision.prop_trace(o, eye, target, Vec3::ZERO));
            println!(
                "use: {} at {:?} owner {owner:?} active {} aim {:?} reach {:.2} solid {} | pills {} medkit {}",
                p.class,
                p.origin,
                crate::loot::pickup_active(binding, cinematic.as_deref()),
                pickup_aim_score(eye, forward, p.origin),
                reach.fraction,
                reach.start_solid,
                s.pills,
                s.medkit
            );
        }
        println!("use: eye {eye:?} nearest {:?}", nearest.map(|(_, p)| &p.class));
    }
    if input.use_item {
        input.use_item = false;
        let eye = body.feet + Vec3::Y * EYE;
        let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
        if nearest.is_none() {
            if let Some(logic) = cinematic.as_mut() {
                if let Some((cabinet_id, position, count, mut slots)) = logic.open_cabinet(eye, forward, &collision) {
                    if let Some(audio) = audio.as_mut() {
                        audio.play("Doors.Medkit_Cabinet", Some(position));
                    }
                    // Retail 102c1770: pills, or first aid kits in a
                    // checkpoint (saferoom), on randomly shuffled slots.
                    let class = if navigation.as_ref().and_then(|n| n.checkpoint_at(position)) == Some(true) {
                        "weapon_first_aid_kit"
                    } else {
                        "weapon_pain_pills"
                    };
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(1, |d| d.subsec_nanos() as i32);
                    let mut rng = l4d2r_game_core::random::UniformRandomStream::new(seed);
                    for i in (1..slots.len()).rev() {
                        slots.swap(i, rng.random_int(0, i as i32) as usize);
                    }
                    for slot in slots.into_iter().take(count) {
                        let item = crate::loot::place_item(
                            &mut commands,
                            models.as_deref(),
                            mesh_assets.as_deref(),
                            &rules,
                            class,
                            slot,
                        );
                        commands.entity(item).insert(crate::loot::PickupInside(cabinet_id));
                    }
                } else {
                    logic.use_near(eye, forward, &collision);
                }
            }
        }
        if let Some((entity, p)) = nearest {
            let drop_position = body.feet
                + (forward * Vec3::new(1., 0., 1.)).normalize_or_zero() * 0.6
                + Vec3::Y * 0.08;
            let old_medical = if matches!(
                p.class.as_str(),
                "weapon_upgradepack_incendiary" | "weapon_upgradepack_explosive"
            ) {
                if s.medkit {
                    Some("weapon_first_aid_kit")
                } else if s.defibrillator {
                    Some("weapon_defibrillator")
                } else {
                    s.upgrade_pack.map(|k| k.pack_class())
                }
            } else if matches!(
                p.class.as_str(),
                "weapon_first_aid_kit" | "weapon_defibrillator"
            ) && s.upgrade_pack.is_some()
            {
                s.upgrade_pack.map(|k| k.pack_class())
            } else {
                match p.class.as_str() {
                    "weapon_first_aid_kit" if s.defibrillator => Some("weapon_defibrillator"),
                    "weapon_defibrillator" if s.medkit => Some("weapon_first_aid_kit"),
                    "weapon_pain_pills" if s.adrenaline => Some("weapon_adrenaline"),
                    "weapon_adrenaline" if s.pills => Some("weapon_pain_pills"),
                    _ => None,
                }
            };
            let accepted = match p.class.as_str() {
                "upgrade_laser_sight" => {
                    if let Some(gun) = s.primary.as_mut() {
                        gun.laser = true;
                    }
                    false
                }
                "upgrade_ammo_incendiary" | "upgrade_ammo_explosive" => {
                    let kind = if p.class.ends_with("incendiary") {
                        crate::upgrades::Kind::Incendiary
                    } else {
                        crate::upgrades::Kind::Explosive
                    };
                    if let Ok(mut claims) = upgrade_claims.get_mut(entity) {
                        crate::upgrades::claim(&mut s, &rules, &mut claims, 0, kind);
                    }
                    false
                }
                "weapon_upgradepack_incendiary" | "weapon_upgradepack_explosive" => {
                    let kind = if p.class.ends_with("incendiary") {
                        crate::upgrades::Kind::Incendiary
                    } else {
                        crate::upgrades::Kind::Explosive
                    };
                    s.upgrade_pack = Some(kind);
                    s.medkit = false;
                    s.defibrillator = false;
                    s.slot = 4;
                    true
                }
                "weapon_gascan" | "weapon_cola_bottles" => {
                    if s.carried.is_some() {
                        false
                    } else {
                        s.carried = Some(p.class.clone());
                        s.slot = 6;
                        s.reload = 0.;
                        s.action = 0.;
                        s.burst_left = 0;
                        if let Some((_, _, Some(binding))) =
                            pickups.iter().find(|(id, _, _)| *id == entity)
                        {
                            if let Some(logic) = cinematic.as_mut() {
                                let output=if logic.world.ents[binding.0].me.classname.starts_with("weapon_scavenge_item") {
                                    "OnItemPickedUp"
                                } else { "OnPlayerPickup" };
                                logic.world.fire_output_pub(binding.0, output);
                                logic.world.ents[binding.0].alive = false;
                                collision.sync_entities(logic);
                            }
                        }
                        true
                    }
                }
                "weapon_first_aid_kit" => {
                    if s.medkit {
                        false
                    } else {
                        s.medkit = true;
                        s.upgrade_pack = None;
                        s.defibrillator = false;
                        true
                    }
                }
                "weapon_pain_pills" => {
                    if s.pills {
                        false
                    } else {
                        s.pills = true;
                        s.adrenaline = false;
                        true
                    }
                }
                "weapon_adrenaline" => {
                    if s.adrenaline {
                        false
                    } else {
                        s.adrenaline = true;
                        s.pills = false;
                        true
                    }
                }
                "weapon_defibrillator" => {
                    if s.defibrillator {
                        false
                    } else {
                        s.defibrillator = true;
                        s.upgrade_pack = None;
                        s.medkit = false;
                        true
                    }
                }
                "weapon_ammo" => {
                    if let Some(gun) = &mut s.primary {
                        gun.reserve = rules
                            .weapons
                            .get(&gun.class)
                            .map_or(0, |w| w.reserve_capacity());
                    }
                    false
                }
                class
                    if matches!(
                        class,
                        "weapon_molotov" | "weapon_pipe_bomb" | "weapon_vomitjar"
                    ) =>
                {
                    if s.throwable.is_some() {
                        false
                    } else {
                        s.throwable = Some(class.into());
                        s.slot = 3;
                        true
                    }
                }
                class => {
                    if let Some(weapon) = rules.weapons.get(class) {
                        let mut gun = dropped
                            .get(entity)
                            .ok()
                            .and_then(|d| d.0.clone())
                            .unwrap_or_else(|| Gun {
                                laser: false,
                                ammo_upgrade: None,
                                upgraded_rounds: 0,
                                class: class.into(),
                                ammo: weapon.clip,
                                reserve: weapon.reserve_capacity(),
                            });
                        if dropped.get(entity).is_err() {
                            if let Some(old) = s.primary.as_ref().filter(|g| g.class == p.class) {
                                gun.laser = old.laser;
                                gun.ammo_upgrade = old.ammo_upgrade;
                                gun.upgraded_rounds = old.upgraded_rounds;
                            }
                        }
                        if !weapon.melee() && !weapon.infinite() {
                            if let Some(old) = s.primary.as_ref().filter(|g| g.class != p.class) {
                                crate::loot::drop_item(
                                    &mut commands,
                                    models.as_deref(),
                                    mesh_assets.as_deref(),
                                    &rules,
                                    &collision,
                                    &old.class,
                                    drop_position,
                                    Some(old.clone()),
                                );
                            }
                        }
                        if weapon.infinite() || weapon.melee() {
                            if !s.pickup_secondary(gun) {
                                return;
                            }
                        } else {
                            s.primary = Some(gun);
                            s.slot = 1;
                        }
                        s.reload = 0.;
                        true
                    } else {
                        false
                    }
                }
            };
            if accepted {
                if let Some(old) = old_medical {
                    crate::loot::drop_item(
                        &mut commands,
                        models.as_deref(),
                        mesh_assets.as_deref(),
                        &rules,
                        &collision,
                        old,
                        drop_position,
                        None,
                    );
                }
                if let Some(logic) = cinematic.as_mut() {
                    let now = logic.world.time;
                    if let Some(dialogue) = &mut logic.dialogue {
                        let item = match p.class.as_str() {
                            "weapon_first_aid_kit" => "FirstAidKit",
                            "weapon_pain_pills" => "PainPills",
                            class if class.starts_with("weapon_melee_") => {
                                class.trim_start_matches("weapon_melee_")
                            }
                            class => class.trim_start_matches("weapon_"),
                        };
                        let group =
                            format!("SurvivorPickup{}{}", item, session.survivor.character());
                        dialogue.response(session.survivor.character(), &group, now);
                    }
                }
                crate::loot::consume(
                    &mut commands,
                    entity,
                    &p.class,
                    p.origin,
                    &mut stocks,
                    &rendered,
                );
            }
        }
    }
}
#[path = "game_melee.rs"]
pub(crate) mod melee;
#[path = "game_infected_mind.rs"]
pub(crate) mod mind;
#[path = "game_touch.rs"]
pub(crate) mod touch;
#[path = "ui_fit.rs"]
pub(crate) mod ui_fit;
#[path = "game_timed_button.rs"]
pub(crate) mod timed_button;
#[cfg(test)]
#[path = "finale_harness.rs"]
pub(crate) mod finale_harness;
/// Per-survivor hearing inputs for unalerted commons: running time and the
/// last shot serial (index 0 is the player, then squad members).
#[derive(Default)]
pub(crate) struct SurvivorNoise {
    running: Vec<f32>,
    shots: Vec<u64>,
    positions: Vec<Vec3>,
    clock: f32,
}
pub(crate) fn combat(
    mut commands: Commands,
    c: Res<Collision>,
    rules: Res<Rules>,
    mut s: ResMut<Match>,
    mut input: ResMut<Input>,
    mut body: Single<&mut Body>,
    mut enemies: Query<(Entity, &mut Transform, &mut Enemy)>,
    mut sounds: Option<ResMut<game_audio::Queue>>,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut impacts: Option<ResMut<crate::bullet_impacts::Impacts>>,
    animation: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    playback: Option<Res<crate::l4d2r_animation::PlaybackState>>,
    mut hit_audit: Option<ResMut<crate::studio_hitboxes::Audit>>,
    damage_session: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut upgrade_fx: Option<ResMut<crate::upgrades::Effects>>,
    mut squad: Option<ResMut<Squad>>,
) {
    let _timing = crate::performance::scope("game::combat");
    if s.infected
        || s.paused
        || s.dead
        || (s.pinned && !s.pin_allows_fire)
        // Reviving holds the weapon down: no shots or shoves.
        || s.reviving_teammate()
        || cinematic.as_ref().is_some_and(|c| c.active)
    {
        return;
    }
    let weapon = rules.weapons.get(&s.gun().class).unwrap_or(&rules.pistol);
    if s.gun().ammo == 0 || s.slot > 2 {
        s.burst_left = 0;
    }
    let reload = std::mem::take(&mut input.reload)
        || (s.gun().ammo == 0
            && s.cooldown <= 0.
            && s.action <= 0.
            && s.shove <= 0.
            && s.throw_stage == 0);
    if reload {
        s.burst_left = 0;
    }
    if reload
        && s.slot <= 2
        && !weapon.melee()
        && s.reload <= 0.
        && s.gun().ammo < weapon.clip
        && (weapon.infinite() || s.gun().reserve > 0)
    {
        s.begin_reload(weapon);
    }
    // Capture simulation bypasses live mouse input. Stage the opt-in shove
    // here so the fixture exercises the same gameplay and animation path.
    if std::env::args().any(|a| a == "--lab-capture-shove")
        && s.elapsed > 2.4
        && s.shove_serial == 0
    {
        input.shove = true;
    }
    let shove = std::mem::take(&mut input.shove) && s.shove <= 0. && !s.incap && s.throw_stage == 0;
    // L4D2R_AUTOFIRE=<seconds>: from then on, aim at the nearest living
    // common's body and fire (live gore checks).
    let autofire = std::env::var("L4D2R_AUTOFIRE").ok().and_then(|v| v.parse::<f32>().ok())
        .is_some_and(|after| s.elapsed > after) && {
        if s.slot != 1 {
            s.select_inventory_slot(1);
        }
        let eye = body.feet + Vec3::Y * body.stance.eye();
        // L4D2R_AUTOFIRE_FIXED: keep the start view and fire straight ahead
        // for 1.5 s (bullet-hole checks).
        let fixed = std::env::var_os("L4D2R_AUTOFIRE_FIXED").is_some();
        let target = enemies.iter()
            .filter(|_| !fixed)
            .filter(|(_, _, e)| e.health > 0. && e.model.contains("/common_"))
            .map(|(_, t, _)| t.translation + Vec3::Y * 1.1)
            .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)));
        if let Some(p) = target.filter(|p| p.distance(eye) > 6.) {
            // Step in 2 m from the target, on its side facing us.
            let back = ((eye - p) * Vec3::new(1., 0., 1.)).normalize_or_zero();
            body.feet = p - Vec3::Y * 1.1 + back * 2.;
            body.velocity = Vec3::ZERO;
        }
        if let Some(p) = target {
            let d = (p - eye).normalize_or_zero();
            body.yaw = (-d.x).atan2(-d.z);
            body.pitch = d.y.asin();
        }
        if s.cooldown <= 0. {
            println!("L4D2RAUTO t={:.2} target={:?} slot={} class={} ammo={} reload={:.2} feet={:?}",
                s.elapsed, target.map(|p| p.distance(eye)), s.slot, s.gun().class, s.gun().ammo, s.reload, body.feet);
        }
        target.is_some() || (fixed && s.elapsed < std::env::var("L4D2R_AUTOFIRE").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.) + 1.5)
    };
    let fire = autofire && s.cooldown <= 0. || std::mem::take(&mut input.fire)
        || (weapon.automatic() && input.fire_held)
        || s.burst_left > 0
        || (std::env::args().any(|a| a == "--capture-muzzle") && s.elapsed > 0.5);
    if fire && weapon.shell_reload() && s.reload > 0. && s.gun().ammo > 0 {
        s.reload = 0.;
        s.reload_phase = 0;
    }
    if !shove
        && (s.slot > 2
            || s.reload > 0.
            || s.cooldown > 0.
            || !fire
            || s.ledge.is_some()
            || (!weapon.melee() && s.gun().ammo == 0))
    {
        return;
    }
    if shove {
        s.shove = 0.7;
        s.shove_serial = s.shove_serial.wrapping_add(1);
        // Shoving a teammate held by a Smoker's tongue frees them.
        let forward = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
        for (i, m) in squad.iter().flat_map(|q| q.0.iter().enumerate()) {
            let to = (m.position - body.feet) * Vec3::new(1., 0., 1.);
            if m.state.pinned_by == Some(crate::special_infected::SpecialClass::Smoker)
                && !m.state.dead
                && to.length() < 1.3
                && to.normalize_or_zero().dot(forward) > 0.3
            {
                crate::special_infected::ai::shove_victim(crate::special_infected::ai::Victim::Bot(i));
            }
        }
    } else {
        if !weapon.melee() {
            s.gun_mut().ammo -= 1;
        }
        if weapon.class == "weapon_rifle_desert" {
            if s.burst_left == 0 {
                s.burst_left = 3;
            }
            s.burst_left -= 1;
        } else {
            s.burst_left = 0;
        }
        s.shot_serial = s.shot_serial.wrapping_add(1);
        if weapon.melee() {
            melee::next_attack(&mut s, weapon);
        }
        s.cooldown = weapon.shot_cycle(s.gun().ammo, s.incap);
        if let Some(queue) = sounds.as_mut() {
            queue.play(&weapon.fire_sound, None);
        }
    }
    // Melee swings sweep over time (`melee::swing`), not one trace here.
    if !shove && weapon.melee() {
        return;
    }
    let origin = body.feet + Vec3::Y * if s.incap { s.downed_eye() } else { body.stance.eye() };
    let rotation = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.);
    let direction = rotation * Vec3::NEG_Z;
    let range = if shove { 75. * UNIT } else { weapon.range };
    if shove {
        if let Some(logic) = cinematic.as_mut() {
            logic.shoot_prop(origin, direction, range, 150., &c);
        }
        for (entity, mut transform, mut enemy) in &mut enemies {
            let delta = transform.translation + Vec3::Y - origin;
            if enemy.health > 0.
                && delta.length() < range + 0.35
                && direction.dot(delta.normalize_or_zero()) > 0.65
                && c.trace(origin, transform.translation + Vec3::Y, Vec3::ZERO)
                    .fraction
                    >= 0.99
            {
                if crate::infected_population::Kind::model(&enemy.model)
                    == crate::infected_population::Kind::Riot
                {
                    transform.rotation *= Quat::from_rotation_y(std::f32::consts::PI);
                } else {
                    enemy.health -= 5.;
                    crate::game_ui::kill_feed::hit(entity, crate::game_ui::kill_feed::Killer::Player);
                }
                enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
                enemy.attack = animation.as_ref().map_or(0.7, |a| {
                    a.infected_reaction_duration(
                        &enemy.model,
                        "ACT_TERROR_SHOVED_BACKWARD",
                        entity
                            .to_bits()
                            .wrapping_add(enemy.reaction_serial.wrapping_mul(7919)),
                    )
                });
                enemy.reaction = 1;
                let away = (direction * Vec3::new(1., 0., 1.)).normalize_or_zero();
                transform.translation = c.walk(
                    transform.translation,
                    away * 0.55,
                    HEIGHT,
                    c.grounded(transform.translation, HEIGHT),
                );
                if let Some(queue) = sounds.as_mut() {
                    queue.play(
                        if crate::infected_population::Kind::model(&enemy.model)
                            == crate::infected_population::Kind::Clown
                        {
                            "Weapon.HitClown"
                        } else {
                            "Zombie.Shoved"
                        },
                        Some(transform.translation),
                    );
                }
                if enemy.health <= 0. {
                    enemy.record_death(
                        crate::infected_death::Cause::Blunt,
                        &transform,
                        direction,
                        1.,
                    );
                    enemy.corpse_time = animation.as_ref().map_or(5., |a| {
                        a.death_duration(
                            &enemy.model,
                            enemy.death,
                            entity
                                .to_bits()
                                .wrapping_add(enemy.reaction_serial.wrapping_mul(7919)),
                        ) + 6.
                    });
                    s.kills += 1;
                }
            }
        }
        return;
    }
    let mut bleeding = std::collections::HashSet::new();
    // Build bone inverses once per attack, shared by every shotgun pellet.
    let hitboxes: std::collections::HashMap<_, _> = enemies
        .iter()
        .filter(|(_, _, e)| e.health > 0.)
        .filter_map(|(entity, t, enemy)| {
            animation
                .as_ref()
                .and_then(|a| a.hitboxes(&enemy.model, entity, t, playback.as_deref()))
                .map(|boxes| (entity, boxes))
        })
        .collect();
    let special_round = if !weapon.melee() && s.slot == 1 {
        crate::upgrades::take_round(s.gun_mut())
    } else {
        None
    };
    let laser = s.gun().laser;
    // The shot uses the current spread, then SpreadPerShot is added
    // (CTerrorGun 103e9370 / 103e7090).
    let cone = if weapon.melee() { 0. } else { s.accuracy_bloom };
    if !weapon.melee() {
        s.accuracy_bloom = weapon.accuracy.after_shot(s.accuracy_bloom, laser);
    }
    for direction in
        crate::ballistics::directions_with_upgrade(weapon, rotation, s.shot_serial, cone, laser)
    {
        let trace = c.trace(origin, origin + direction * range, Vec3::ZERO);
        if let Some(logic) = cinematic.as_mut() {
            logic.shoot_prop(origin, direction, range, weapon.damage * 2., &c);
            logic.shoot_car_alarm(origin, direction, range, &c);
            logic.shoot_breakable(
                origin,
                direction,
                range,
                weapon.damage,
                if weapon.melee() {
                    l4d2r_game_core::entities::DamageKind::Club
                } else {
                    l4d2r_game_core::entities::DamageKind::Bullet
                },
                &c,
            );
        }
        // Retail penetration: the bullet walks every infected and wall on
        // its path (bullet_penetration). Explosive rounds get one layer.
        let candidates: Vec<_> = enemies
            .iter()
            .filter(|(_, _, enemy)| enemy.health > 0.)
            .filter_map(|(entity, t, enemy)| {
                let (distance, group) = if let Some(boxes) = hitboxes.get(&entity) {
                    boxes
                        .trace(origin, direction, range)
                        .map(|hit| (hit.distance, Some(hit.group)))?
                } else {
                    crate::ray_box(
                        origin,
                        direction,
                        t.translation + Vec3::new(-0.35, 0., -0.35),
                        t.translation + Vec3::new(0.35, 1.8, 0.35),
                    )
                    .map(|d| (d, None))?
                };
                Some(crate::bullet_penetration::Target {
                    distance,
                    tank: enemy.model.contains("hulk"),
                    absorbs: !matches!(weapon.class.as_str(), "weapon_chainsaw" | "weapon_melee_chainsaw")
                        && crate::infected_population::Kind::model(&enemy.model).blocks_front(t, direction),
                    value: (entity, group),
                })
            })
            .collect();
        let penetration = if weapon.melee() {
            crate::bullet_penetration::Penetration::default()
        } else if special_round == Some(crate::upgrades::Kind::Explosive) {
            // Upgraded rounds have no penetration power; explosive ones
            // also get a single layer (103c7470 / 103c6bf0).
            crate::bullet_penetration::Penetration { layers: 1, power: 0., ..weapon.penetration }
        } else if special_round.is_some() {
            crate::bullet_penetration::Penetration { power: 0., ..weapon.penetration }
        } else {
            weapon.penetration
        };
        let path = crate::bullet_penetration::walk(
            penetration,
            range,
            candidates,
            crate::bullet_penetration::world_wall(&c, origin, direction, range),
        );
        let nearest = path.hits.first().map_or(path.end, |h| h.0);
        let target = path.hits.first().map(|h| h.1);
        // Friendly fire: a survivor in front of the first infected or wall takes
        // the hit, scaled like retail's survivor_friendly_fire_factor_<difficulty>
        // (easy 0, normal 0.1, advanced 0.3, expert 0.5). Melee never hurts teammates.
        // In a real Xbox's game the Xbox already decides this from our usercmds.
        if !weapon.melee() && !crate::xbox_link::joined_xbox() {
            use crate::campaign_settings::Difficulty;
            let factor = match damage_session.as_ref().map(|d| d.difficulty) {
                Some(Difficulty::Easy) => 0.,
                Some(Difficulty::Advanced) => 0.3,
                Some(Difficulty::Expert) => 0.5,
                _ => 0.1,
            };
            let struck = |p: Vec3, height: f32| {
                crate::ray_box(origin, direction, p + Vec3::new(-0.35, 0., -0.35), p + Vec3::new(0.35, height, 0.35))
                    .filter(|d| *d > 0.3 && *d < nearest)
            };
            if factor > 0. {
                if !crate::net_play::guest() {
                    if let Some(q) = squad.as_deref_mut() {
                        for m in q.0.iter_mut().filter(|m| !m.state.dead) {
                            if let Some(d) = struck(m.position, if m.state.incap { 0.65 } else { HEIGHT }) {
                                m.state.damage(weapon.damage_at(d) * factor);
                            }
                        }
                    }
                }
                for (survivor, p, incap) in crate::net_play::remote_survivors() {
                    if let Some(d) = struck(p, if incap { 0.65 } else { HEIGHT }) {
                        crate::net_play::friendly_fire(&survivor, weapon.damage_at(d) * factor);
                    }
                }
            }
        }
        if std::env::var_os("L4D2R_PEN_LOG").is_some() && !weapon.melee() {
            eprintln!(
                "L4D2R_PEN {}: hits {:?} walls {:?} end {:.0}",
                weapon.class,
                path.hits.iter().map(|h| (h.0 / UNIT).round()).collect::<Vec<_>>(),
                path.marks.iter().map(|m| (m.0 / UNIT).round()).collect::<Vec<_>>(),
                path.end / UNIT
            );
        }
        // Thrown Tank rocks take bullet damage (z_tank_throw_health).
        if !weapon.melee() {
            crate::special_infected::tank::shot_at_rocks(origin, direction, path.end, weapon.damage_at(path.end));
        }
        if let Some(kind) = special_round {
            if let Some(fx) = upgrade_fx.as_mut() {
                if target.is_some() || trace.fraction < 1. {
                    fx.hits.push((kind, origin + direction * nearest));
                }
            }
        }
        if !weapon.melee() {
            for &(distance, normal, material) in &path.marks {
                let p = origin + direction * distance;
                if !decal_on_movable(cinematic.as_deref(), p) {
                    if let Some(impacts) = impacts.as_mut() {
                        impacts.hits.push((p, normal, material));
                    }
                }
            }
        }
        if weapon.melee() && (target.is_some() || trace.fraction < 1.) {
            let sound = if target.is_some() {
                &weapon.melee_hit_sound
            } else {
                &weapon.melee_world_sound
            };
            if !sound.is_empty() {
                if let Some(queue) = sounds.as_mut() {
                    queue.play(sound, Some(origin + direction * nearest));
                }
            }
        }
        for (nearest, (entity, hit_group)) in path.hits {
            if !matches!(
                weapon.class.as_str(),
                "weapon_chainsaw" | "weapon_melee_chainsaw"
            ) && enemies.get(entity).is_ok_and(|(_, t, e)| {
                crate::infected_population::Kind::model(&e.model).blocks_front(t, direction)
            }) {
                continue;
            }
            if bleeding.insert(entity) {
                if let Some(impacts) = impacts.as_mut() {
                    impacts.blood.push(crate::bullet_impacts::BloodHit {
                        position: origin + direction * nearest,
                        direction,
                        shotgun: weapon.pellets > 1,
                    });
                }
            }
            if let Ok((_, transform, mut enemy)) = enemies.get_mut(entity) {
                if let (Some(impacts), Some(boxes)) = (impacts.as_mut(), hitboxes.get(&entity)) {
                    if let Some(hit) = boxes.trace(origin, direction, nearest + 0.001) {
                        impacts.gore.push(crate::infected_gore::Hit {
                            damage_force_source: if special_round.is_some() {
                                None
                            } else {
                                crate::infected_gore::bullet_damage_force_source(&weapon.class, direction)
                            },
                            entity, position: origin + direction * nearest, direction,
                            box_index: hit.index, distance: nearest,
                            damage_type: if special_round == Some(crate::upgrades::Kind::Explosive) {
                                crate::infected_gore::DMG_BLAST
                            } else if weapon.class.contains("chainsaw") {
                                crate::infected_gore::DMG_CHAINSAW
                            } else {
                                crate::infected_gore::Hit::bullet_type(&weapon.class, Some(hit.group as i32))
                            },
                            sniper: crate::infected_gore::Hit::is_sniper(&weapon.class),
                            swing: None, back: false, decapitates: false, seed: s.shot_serial,
                            cause: if special_round == Some(crate::upgrades::Kind::Explosive) { crate::infected_death::Cause::Explosion }
                                else if weapon.melee() && weapon.damage_flags & 4 != 0 { crate::infected_death::Cause::Slash }
                                else if weapon.melee() { crate::infected_death::Cause::Blunt }
                                else if weapon.pellets > 1 { crate::infected_death::Cause::Shotgun }
                                else { crate::infected_death::Cause::Bullet },
                        });
                    }
                }
                let multiplier = if !weapon.melee() && hit_group.is_some_and(|group| group != 1) {
                    damage_session.as_ref().map_or(1., |s| {
                        crate::studio_hitboxes::body_damage_factor(s.difficulty)
                    })
                } else {
                    1.
                };
                enemy.health -= weapon.damage_at(nearest) * multiplier;
                crate::game_ui::kill_feed::hit(entity, crate::game_ui::kill_feed::Killer::Player);
                if !weapon.melee() && hit_group == Some(1) {
                    crate::special_infected::witch::head_shot(entity, weapon.damage_at(nearest));
                }
                if special_round == Some(crate::upgrades::Kind::Incendiary)
                    && !crate::infected_population::Kind::model(&enemy.model).fireproof()
                {
                    enemy.burning = 4.;
                    commands.entity(entity).insert(crate::upgrades::Burn(4.));
                }
                if special_round == Some(crate::upgrades::Kind::Explosive) {
                    enemy.reaction = 1;
                }

                if let (Some(audit), Some(group)) = (hit_audit.as_mut(), hit_group) {
                    if audit.groups.len() == 128 {
                        audit.groups.remove(0);
                    }
                    audit.groups.push(group);
                }
                enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
                // Commons flinch as a gesture over whatever they are doing
                // (retail AddGesture(ACT_TERROR_FLINCH), 10416750): a shot
                // does not stop the chase. Others keep the full-body flinch.
                if !enemy.model.contains("/common_") {
                    enemy.attack = animation.as_ref().map_or(0.5, |a| {
                        a.infected_reaction_duration(
                            &enemy.model,
                            "ACT_TERROR_FLINCH",
                            entity
                                .to_bits()
                                .wrapping_add(enemy.reaction_serial.wrapping_mul(7919)),
                        )
                    });
                    enemy.reaction = 2;
                }
                if enemy.health <= 0. {
                    let cause = if special_round == Some(crate::upgrades::Kind::Explosive) {
                        crate::infected_death::Cause::Explosion
                    } else if weapon.melee() {
                        if weapon.damage_flags & 4 != 0 {
                            crate::infected_death::Cause::Slash
                        } else {
                            crate::infected_death::Cause::Blunt
                        }
                    } else if weapon.pellets > 1 {
                        crate::infected_death::Cause::Shotgun
                    } else {
                        crate::infected_death::Cause::Bullet
                    };
                    enemy.record_death(
                        cause,
                        &transform,
                        direction,
                        (origin + direction * nearest).y - transform.translation.y,
                    );
                    enemy.death.far = nearest / UNIT > 500.;
                    if let Some(group) = hit_group {
                        enemy.death.head = group == 1;
                    }
                    enemy.corpse_time = animation.as_ref().map_or(5., |a| {
                        a.death_duration(
                            &enemy.model,
                            enemy.death,
                            entity
                                .to_bits()
                                .wrapping_add(enemy.reaction_serial.wrapping_mul(7919)),
                        ) + 6.
                    });
                    s.kills += 1;
                }
            }
        }
    }
    if !shove {
        s.view_punch += gun_punch(weapon.punch, &body);
    }
}
/// Retail CTerrorGun punch (server 103e8c40; z_gun_vertical_punch 1,
/// z_gun_horiz_punch 0, z_gun_kick 0): VerticalPunch kicks the view, not the
/// aim. x2 in the air; standing x1 still, x1.25 moving; crouched x0.5 still,
/// x0.9 moving (moving: over 5 units/s).
pub(crate) fn gun_punch(vertical: f32, body: &Body) -> f32 {
    let moving = body.velocity.xz().length() / UNIT > 5.;
    vertical
        * if !body.grounded {
            2.
        } else if body.crouched {
            if moving { 0.9 } else { 0.5 }
        } else if moving {
            1.25
        } else {
            1.
        }
}
/// Retail punch decay (server 10239310): the punch shrinks by
/// (length * 0.5 + punch_angle_decay_rate 20) degrees per second.
pub(crate) fn decay_punch(punch: f32, dt: f32) -> f32 {
    let length = punch.abs() - (punch.abs() * 0.5 + 20.) * dt;
    if length <= 0. { 0. } else { length * punch.signum() }
}
fn director(
    time: Res<Time<Fixed>>,
    s: Res<Match>,
    mut campaign: ResMut<Campaign>,
    c: Res<Collision>,
    body: Single<&Body>,
    models: Res<viewer::RenderModels>,
    enemies: Query<(
        Entity,
        &Transform,
        &Enemy,
        Option<&crate::special_infected::ai::AiSpecial>,
        Option<&crate::special_infected::witch::Witch>,
    )>,
    mut commands: Commands,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut native: Option<ResMut<crate::source_director::Director>>,
    squad: Res<Squad>,
    mut audio: ResMut<crate::game_audio::Queue>,
    specials: Query<(&crate::special_infected::ai::AiSpecial, &Enemy)>,
    mut quick_specials: Local<bool>,
    mut witches: Option<ResMut<crate::special_infected::witch::Requests>>,
    mut living: Local<HashMap<Entity, Vec3>>,
) {
    let _timing = crate::performance::scope("game::director");
    let Some(native) = native.as_mut() else {
        return;
    };
    let _dir0 = crate::performance::scope("director::logic");
    if let Some(logic) = cinematic.as_mut() {
        native.set_script_time(logic.world.time);
        if native.script_sync_due(time.delta_secs()) || !logic.world.cine.script_commands.is_empty()
        {
            if !native.sync_world(&logic.world) { return; }
        }
        for request in std::mem::take(&mut logic.world.cine.script_commands) {
            if request.input == "RegisterThink" {
                native.configure_think(request.entity, &request.param);
            } else {
                native.entity_script_context(
                    request.entity,
                    &request.input,
                    &request.param,
                    request.activator,
                    request.caller,
                );
            }
        }
        native.run_thinks(&logic.world);
        // point_clientcommand `Command` and player AddOutput origin/angles:
        // run as the local player's console lines (testscripts' PD1).
        for (target, amount) in std::mem::take(&mut *crate::game_state::DAMAGE_REQUESTS.lock().unwrap()) {
            let mut ids = logic.world.find(&target);
            if ids.is_empty() {
                ids = logic.world.find_hammer_id(&target);
            }
            for id in ids {
                logic.world.damage_entity(id, amount, l4d2r_game_core::entities::DamageKind::Bullet);
            }
        }
        for line in std::mem::take(&mut logic.world.cine.client_commands) {
            crate::source_commands::submit(&line);
        }
        for (input, param) in std::mem::take(&mut logic.world.cine.director_commands) {
            native.command(&input, &param);
            if matches!(
                input.to_ascii_lowercase().as_str(),
                "panicevent" | "forcepanicevent" | "createmob" | "spawnmob"
            ) && native.take_horde_alert()
            {
                audio.play("MegaMobIncoming", None);
            }
        }
    }
    flush_vscript_commands(native, cinematic.as_deref_mut(), &mut audio);
    // A dead infected player (e.g. an exploded Boomer awaiting respawn) does
    // not stop the survivors' Director.
    drop(_dir0);
    let _dir1 = crate::performance::scope("director::samples");
    if s.paused || (s.dead && !s.infected) || cinematic.as_ref().is_some_and(|c| c.active) {
        return;
    }
    let alive = enemies.iter().filter(|(_, _, e, _, _)| e.health > 0.).count();
    let mut survivors = vec![(0, body.feet)];
    survivors.extend(
        squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.state.dead)
            .map(|(i, m)| (i + 1, m.position)),
    );
    native.observe_survivors(time.delta_secs(), &survivors);
    let nearby = enemies
        .iter()
        .filter(|(_, t, e, _, _)| e.health > 0. && t.translation.distance(body.feet) < 500. * UNIT)
        .count();
    // Retail survivor intensity: damage, incapacitation and ledge grabs
    // (observe_intensity) and infected deaths near each survivor.
    let mut samples = vec![];
    if !s.infected {
        samples.push(crate::source_director::SurvivorSample {
            id: 0,
            position: body.feet,
            health: s.health + s.temporary,
            incap: s.incap,
            incap_health: s.incap_health,
            ledge: s.ledge.is_some(),
            dead: s.dead,
        });
    }
    samples.extend(squad.0.iter().enumerate().map(|(i, m)| crate::source_director::SurvivorSample {
        id: i + 1,
        position: m.position,
        health: m.state.health + m.state.temporary,
        incap: m.state.incap,
        incap_health: m.state.incap_health,
        ledge: m.state.ledge.is_some(),
        dead: m.state.dead,
    }));
    native.observe_intensity(&samples);
    for (entity, transform, enemy, special, witch) in &enemies {
        if enemy.health > 0. {
            living.insert(entity, transform.translation);
        } else if living.remove(&entity).is_some() {
            use crate::source_director::Killed;
            let killed = if witch.is_some()
                || special.is_some_and(|a| a.class == crate::special_infected::SpecialClass::Tank)
            {
                Killed::Boss
            } else if special.is_some() {
                Killed::Special
            } else {
                Killed::Common { fast: enemy.movement_speed > 150. * UNIT }
            };
            native.enemy_killed(transform.translation, killed);
        }
    }
    // Despawned without dying (corpse cleanup, map change): forget them.
    if living.len() > alive + 64 {
        living.retain(|e, _| enemies.get(*e).is_ok_and(|(_, _, en, _, _)| en.health > 0.));
    }
    drop(_dir1);
    let _dir2 = crate::performance::scope("director::tick");
    let count = native.tick(time.delta_secs(), s.health, nearby, alive, body.feet);
    // A panic's MegaMobIncoming plays as STAGE_INITIAL_DELAY ends, whether
    // or not the common limit leaves room to spawn.
    if count == 0 && native.take_horde_alert() {
        audio.play("MegaMobIncoming", None);
    }
    // Finale outputs the Director decided (FinaleEscapeStarted) fire on the
    // started trigger_finale.
    let finale_events = native.take_finale_events();
    if !finale_events.is_empty() {
        if let Some(logic) = cinematic.as_deref_mut() {
            let finale = logic.world.ents.iter().position(|e| {
                e.alive && matches!(e.kind, l4d2r_game_core::entities::Kind::Finale { started: true, .. })
            });
            if let Some(id) = finale {
                for event in finale_events {
                    println!("Finale output: {event}");
                    logic.world.fire_output_pub(id, &event);
                }
            }
        }
    }
    flush_vscript_commands(native, cinematic.as_deref_mut(), &mut audio);
    // Online guests spawn nothing: the host's game runs the horde
    // (net_infected).
    drop(_dir2);
    let _dir3 = crate::performance::scope("director::spawn");
    if crate::net_play::guest() || crate::game_state::director_stopped() {
        return;
    }
    let mut fallen_count = enemies
        .iter()
        .filter(|(_, _, e, _, _)| {
            e.health > 0.
                && crate::infected_population::Kind::model(&e.model)
                    == crate::infected_population::Kind::Fallen
        })
        .count();
    for _ in 0..count {
        let Some(position) = hidden_spawn(native, &c, body.feet, &squad, 500. * UNIT, None, &[]) else {
            continue;
        };
        let mob = native.pending > 0;
        if spawn_common(&mut commands, &models, &mut campaign, native, &mut fallen_count, s.elapsed, position, mob)
            .is_some()
        {
            native.spawned();
            if native.take_horde_alert() {
                audio.play("MegaMobIncoming", None);
            }
        }
    }
    // Wandering population (retail MaintainWanderingPopulation): idle
    // commons placed in hidden NAV areas near the survivors. Areas within
    // director_threat_clear_radius (500) of a survivor are cleared instead.
    let mut living = vec![body.feet];
    living.extend(squad.0.iter().filter(|m| !m.state.dead).map(|m| m.position));
    let mut population = alive + count;
    drop(_dir3);
    let _dir4 = crate::performance::scope("director::wanderers");
    for (spot, center) in native.wanderer_spots(time.delta_secs(), &living) {
        if living.iter().any(|p| p.distance(center) < THREAT_CLEAR_RADIUS * UNIT) {
            while native.wanderers_at(spot) > 0 {
                native.wanderer_spawned(spot);
            }
            continue;
        }
        while native.wanderers_at(spot) > 0 && population < native.limit() {
            let mut placed = None;
            for _ in 0..3 {
                let offset = native.wander_offset() * UNIT;
                if let Some(p) = hidden_point(&c, body.feet, &squad, center + offset) {
                    placed = Some(p);
                    break;
                }
            }
            let Some(position) = placed else {
                break;
            };
            let seed = campaign.total_spawned as u64 * 0x9E37_79B9 + spot as u64;
            if let Some(actor) = spawn_common(
                &mut commands, &models, &mut campaign, native, &mut fallen_count, s.elapsed, position, false,
            ) {
                // InfectedWander (104195d0): no sitting or lying down when the
                // map's worldspawn "timeofday" (sv_force_time_of_day -1) is
                // 2-4 (daytime); retail c1m1_hotel shows only standing commons.
                let time_of_day = cinematic.as_ref().and_then(|l| {
                    l.world.ents.iter().find(|e| e.me.classname == "worldspawn")
                        .and_then(|e| e.me.kv("timeofday"))
                        .and_then(|v| v.trim().parse::<i32>().ok())
                }).unwrap_or(0);
                let posture = (2..=4).contains(&time_of_day).then_some(mind::Posture::Stagger);
                commands.entity(actor).insert(mind::Mind::wanderer(seed, posture));
                population += 1;
            }
            native.wanderer_spawned(spot);
        }
    }
    // Special infected: the Director's special schedule (MaxSpecials,
    // per-class limits, DominatorLimit, respawn interval) once the survivors
    // have left the safe area, placed out of every survivor's sight between
    // z_spawn_safety_range and z_spawn_range. Not while the player is an
    // infected (the player's own team is the infected team then).
    drop(_dir4);
    let _dir5 = crate::performance::scope("director::rest");
    if !s.infected {
        let live = crate::special_infected::ai::live_counts(specials.iter());
        // `--ai-specials-now` (playtests/captures): ignore the safe-area
        // gate and shorten the first special delay to 3-5 seconds.
        let now = std::env::args().any(|a| a == "--ai-specials-now");
        if now && !*quick_specials {
            *quick_specials = true;
            native.convar("director_special_initial_spawn_delay_min", "3");
            native.convar("director_special_initial_spawn_delay_max", "5");
        }
        let outside = native.flow > 100. || now;
        let wave = native.special_wave(live, outside);
        let requested = wave.len();
        let mut spawned = 0;
        let mut taken = vec![];
        for class in wave {
            let Some(position) = hidden_spawn(
                native,
                &c,
                body.feet,
                &squad,
                crate::special_infected::ai::SPAWN_SAFETY_RANGE * UNIT,
                Some(crate::special_infected::ai::SPAWN_FLOW_LIMIT),
                &taken,
            )
            .filter(|p| p.distance(body.feet) <= crate::special_infected::ai::SPAWN_RANGE * UNIT) else {
                continue;
            };
            taken.push(position);
            if crate::special_infected::ai::spawn(
                &mut commands,
                &models,
                class,
                position,
                body.feet,
            )
            .is_some()
            {
                spawned += 1;
            }
        }
        if requested > 0 {
            native.specials_spawned(spawned);
        }
        // Bosses: the co-op Tank schedule (source_director::tank_due).
        let tank_alive = specials
            .iter()
            .any(|(ai, e)| ai.class == crate::special_infected::SpecialClass::Tank && e.health > 0.);
        native.set_tank_alive(tank_alive);
        // Finale TANK stages request Tanks regardless of ProhibitBosses.
        // Prefer a hidden NAV spot; otherwise any allowed spot beyond the
        // safety range; unplaced requests are retried next tick.
        let requested = native.take_tank_requests();
        let mut unplaced = 0;
        for _ in 0..requested {
            let position = boss_spot(native, &c, body.feet, &squad);
            if position.is_none() {
                unplaced += 1;
            }
            if let Some(position) = position {
                println!("Director finale: Tank at {position:?}");
                crate::special_infected::ai::spawn(
                    &mut commands,
                    &models,
                    crate::special_infected::SpecialClass::Tank,
                    position,
                    body.feet,
                );
            }
        }
        native.return_tank_requests(unplaced);
        // Retail co-op bosses: the Witch/Tank/nothing threat order, spent as
        // survivors first enter the Director's threat areas.
        let survivors: Vec<Vec3> = std::iter::once(body.feet)
            .filter(|_| !s.dead)
            .chain(squad.0.iter().filter(|m| !m.state.dead).map(|m| m.position))
            .collect();
        if let Some((area, kind)) = native.threat_entered(&survivors) {
            let eyes: Vec<Vec3> = survivors.iter().map(|p| *p + Vec3::Y * EYE).collect();
            let visible = |p: Vec3| {
                eyes.iter().any(|eye| {
                    let t = c.sight_trace(*eye, p + Vec3::Y);
                    !t.start_solid && t.fraction >= 1.
                })
            };
            let fits = |p: Vec3| {
                c.supported_spawn_near(p, HEIGHT).is_ok_and(|g| c.for_common().clear_hull(g, HEIGHT))
            };
            // Retail falls back to a random valid spawn area (FUN_1035eae0).
            let position = native
                .threat_position(area, fits, visible)
                .and_then(|p| c.supported_spawn_near(p, HEIGHT).ok())
                .or_else(|| boss_spot(native, &c, body.feet, &squad));
            let spawned = match (position, kind) {
                (Some(position), 7) => {
                    let yaw = native.threat_yaw();
                    match witches.as_mut() {
                        Some(w) => {
                            w.at.push((position, yaw));
                            println!("Director threat: Witch at {position:?}");
                            true
                        }
                        None => false,
                    }
                }
                (Some(position), 8) if !tank_alive => {
                    println!("Director threat: Tank at {position:?}");
                    crate::special_infected::ai::spawn(
                        &mut commands,
                        &models,
                        crate::special_infected::SpecialClass::Tank,
                        position,
                        body.feet,
                    )
                    .is_some()
                }
                _ => {
                    println!("Director threat: type {kind} not placed (position {position:?}, Tank alive {tank_alive})");
                    false
                }
            };
            native.threat_spawned(area, kind, spawned);
        }
        if native.tank_due(outside, tank_alive) {
            if let Some(position) = hidden_spawn(
                native,
                &c,
                body.feet,
                &squad,
                crate::special_infected::ai::SPAWN_SAFETY_RANGE * UNIT,
                Some(crate::special_infected::ai::SPAWN_FLOW_LIMIT),
                &[],
            )
            // Survival Tanks are never skipped: any boss spot will do.
            .or_else(|| native.survival.is_some().then(|| boss_spot(native, &c, body.feet, &squad)).flatten())
            {
                println!("Director: Tank at {position:?}");
                crate::special_infected::ai::spawn(
                    &mut commands,
                    &models,
                    crate::special_infected::SpecialClass::Tank,
                    position,
                    body.feet,
                );
            }
        }
    }
}
/// A boss spawn spot: a hidden NAV spot, else any allowed NAV spot beyond
/// the special-infected safety range on clear, supported ground.
fn boss_spot(
    native: &mut crate::source_director::Director,
    c: &Collision,
    feet: Vec3,
    squad: &Squad,
) -> Option<Vec3> {
    let safety = crate::special_infected::ai::SPAWN_SAFETY_RANGE * UNIT;
    hidden_spawn(native, c, feet, squad, safety, None, &[]).or_else(|| {
        (0..64).find_map(|_| {
            let p = native
                .candidate()
                .filter(|p| c.infected_spawn_allowed(*p) && p.distance(feet) >= safety)?;
            // Same ground snap and hull check as hidden_spawn.
            let ground = c.trace(p + Vec3::Y * 0.4, p - Vec3::Y * 1.5, Vec3::ZERO);
            if ground.fraction >= 1. || ground.normal.y < 0.7 {
                return None;
            }
            let p = p + Vec3::Y * 0.4 - Vec3::Y * 1.9 * ground.fraction + Vec3::Y * 0.004;
            c.for_common().clear_hull(p, HEIGHT).then_some(p)
        })
    })
}
/// A NAV spawn point no living survivor can see, between `min` and the
/// Director's spawn range, on clear ground the infected may use.
/// retail director_threat_clear_radius.
const THREAT_CLEAR_RADIUS: f32 = 500.;
/// Spawn one common of the population's choice at `position`.
#[allow(clippy::too_many_arguments)]
fn spawn_common(
    commands: &mut Commands,
    models: &viewer::RenderModels,
    campaign: &mut Campaign,
    native: &mut crate::source_director::Director,
    fallen_count: &mut usize,
    elapsed: f32,
    position: Vec3,
    mob: bool,
) -> Option<Entity> {
    let fallen_allowed = *fallen_count == 0 && campaign.population.fallen_ready(elapsed);
    let model = campaign.population.choose_allowed(position, mob, fallen_allowed);
    let health = campaign.population.health(&model);
    let carried = campaign
        .population
        .fallen_items_allowed(&model, |class| native.allow_fallen_item(class));
    let draws = models.0.get(&(model.clone(), 0, 0))?;
    if crate::infected_population::Kind::model(&model) == crate::infected_population::Kind::Fallen {
        *fallen_count += 1;
    }
    let death_loot = campaign.population.loot(&model);
    let actor = commands
        .spawn((
            Transform::from_translation(position),
            Visibility::default(),
            Enemy {
                model,
                death_loot,
                loot_dropped: false,
                reaction_serial: 0,
                attack_serial: 0,
                health,
                attack: 0.,
                vertical: 0.,
                reaction: 0,
                corpse_time: 0.,
                death: Default::default(),
                movement_speed: 0.,
                burning: 0.,
            },
        ))
        .id();
    commands
        .entity(actor)
        .insert(crate::infected_population::CarriedLoot(carried));
    campaign.total_spawned += 1;
    for (mesh, material) in draws {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            crate::viewer::DynamicModelLighting::default(),
            Transform::default(),
            RenderLayers::layer(1),
            ChildOf(actor),
        ));
    }
    Some(actor)
}
/// `p` settled on the floor, out of every living survivor's sight, with room
/// for a common.
fn hidden_point(c: &Collision, player: Vec3, squad: &Squad, p: Vec3) -> Option<Vec3> {
    if !c.infected_spawn_allowed(p) {
        return None;
    }
    if c.trace(player + Vec3::Y * EYE, p + Vec3::Y * 1.2, Vec3::ZERO).fraction >= 0.99
        || squad.0.iter().any(|b| {
            !b.state.dead
                && c.trace(b.position + Vec3::Y * EYE, p + Vec3::Y * 1.2, Vec3::ZERO).fraction >= 0.99
        })
    {
        return None;
    }
    let ground = c.trace(p + Vec3::Y * 0.4, p - Vec3::Y * 1.5, Vec3::ZERO);
    if ground.fraction >= 1. || ground.normal.y < 0.7 {
        return None;
    }
    let p = p + Vec3::Y * 0.4 - Vec3::Y * 1.9 * ground.fraction + Vec3::Y * 0.004;
    (c.infected_spawn_allowed(p) && c.for_common().clear_hull(p, HEIGHT)).then_some(p)
}
fn hidden_spawn(
    native: &mut crate::source_director::Director,
    c: &Collision,
    player: Vec3,
    squad: &Squad,
    min: f32,
    flow_limit: Option<f32>,
    taken: &[Vec3],
) -> Option<Vec3> {
    for _ in 0..48 {
        let p = native.candidate()?;
        // Never stack two spawns on one spot (a wave drew the same NAV
        // point for a Smoker and a Jockey).
        if !c.infected_spawn_allowed(p) || taken.iter().any(|t| t.distance(p) < 1.) {
            continue;
        }
        // Specials: within z_spawn_flow_limit of the lead survivor's flow, so
        // a spot near in a straight line but far along the route is skipped.
        if let Some(limit) = flow_limit {
            if native
                .flow_at(p)
                .is_none_or(|f| (f - native.flow).abs() > limit)
            {
                continue;
            }
        }
        let distance = p.distance(player);
        if distance < min
            || distance > native.spawn_range()
            || squad.0.iter().any(|b| p.distance(b.position) < 300. * UNIT)
        {
            continue;
        }
        // Reject positions with line of sight to any living survivor, including behind the player.
        if c.trace(player + Vec3::Y * EYE, p + Vec3::Y * 1.2, Vec3::ZERO)
            .fraction
            >= 0.99
            || squad.0.iter().any(|b| {
                !b.state.dead
                    && c.trace(b.position + Vec3::Y * EYE, p + Vec3::Y * 1.2, Vec3::ZERO)
                        .fraction
                        >= 0.99
            })
        {
            continue;
        }
        let ground = c.trace(p + Vec3::Y * 0.4, p - Vec3::Y * 1.5, Vec3::ZERO);
        if ground.fraction >= 1. || ground.normal.y < 0.7 {
            continue;
        }
        let p = p + Vec3::Y * 0.4 - Vec3::Y * 1.9 * ground.fraction + Vec3::Y * 0.004;
        if !c.infected_spawn_allowed(p) || !c.for_common().clear_hull(p, HEIGHT) {
            continue;
        }
        return Some(p);
    }
    None
}

pub(crate) fn flush_vscript_commands(
    director: &mut crate::source_director::Director,
    mut logic: Option<&mut crate::l4d2r_cinematic::MapLogic>,
    audio: &mut crate::game_audio::Queue,
) {
    // Bound reentrant script -> I/O -> script chains per simulation tick.
    for _ in 0..4 {
        let requests = std::mem::take(&mut director.script_commands);
        if requests.is_empty() {
            break;
        }
        for request in requests {
            match request.target.as_str() {
                "!navmesh" if request.input.eq_ignore_ascii_case("UnblockRescueVehicleNav") => {
                    if let Some(logic)=logic.as_mut() { logic.world.cine.rescue_vehicle_nav_unblocked=true; }
                }
                "!director" => director.command(&request.input, &request.param),
                // SpawnEntityFromTable: "id" 0x1D, then key 0x1F value records
                // separated by 0x1E.
                "!spawn" => {
                    if let Some(logic) = logic.as_mut() {
                        let (reserved, payload) = request.param.split_once('\u{1d}').unwrap_or(("", &request.param));
                        let kvs: Vec<(String, String)> = payload
                            .split('\u{1e}')
                            .filter_map(|r| r.split_once('\u{1f}'))
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect();
                        let id = logic.world.spawn_from_keyvalues(&request.input, kvs);
                        if reserved.parse::<usize>().ok() != Some(id) {
                            eprintln!("SpawnEntityFromTable: {} reserved {reserved}, created {id}", request.input);
                        }
                    }
                }
                // Director.UserDefinedEventN: the info_director's output.
                "!director_event" => {
                    if let Some(logic) = logic.as_mut() {
                        let ids: Vec<usize> = (0..logic.world.ents.len())
                            .filter(|&i| logic.world.ents[i].alive && logic.world.ents[i].me.classname == "info_director")
                            .collect();
                        for id in ids {
                            logic.world.fire_output_pub(id, &request.input);
                        }
                    }
                }
                "!convar" => director.convar(&request.input, &request.param),
                "!sound" => audio.play(request.input, None),
                "!think" => {
                    if let Ok(id) = request.input.parse() {
                        director.configure_think(id, &request.param);
                    }
                }
                "!connect" | "!disconnect" => {
                    if let (Some((id, output)), Some(logic)) =
                        (request.input.split_once(' '), logic.as_mut())
                    {
                        if let Ok(id) = id.parse() {
                            logic.world.connect_script_output(
                                id,
                                output,
                                &request.param,
                                request.target == "!connect",
                            );
                        }
                    }
                }
                target => {
                    if let Some(logic) = logic.as_mut() {
                        logic.world.queue_input_context(
                            target,
                            &request.input,
                            &request.param,
                            request.delay,
                            request.activator,
                            request.caller,
                        );
                    }
                }
            }
        }
    }
}

/// Follow distance. Retail bots keep near sb_separation_range (200) and
/// settle 170-215 units from the human after friction coasting (bots-1027);
/// they stop pushing at 240 units and coast about 45 units from run speed.
const BOT_SEPARATION_RANGE: f32 = 240. * UNIT;
/// Survivor bots run at the survivor run speed (220 units/s, recorded on
/// c2m1), or the limp speed (150) below survivor_limp_health (40).
/// Personal space survivor bots keep from teammates: hull width (32 units)
/// plus a small margin.
const SURVIVOR_SPACING: f32 = 48. * UNIT;
/// Upright teammates a bot must not walk into: other living, standing bots
/// and the survivor player.
fn survivor_obstacles(me: usize, positions: &[(Vec3, bool)], player: Option<Vec3>) -> Vec<(usize, Vec3)> {
    positions
        .iter()
        .enumerate()
        .filter(|(i, (_, standing))| *i != me && *standing)
        .map(|(i, (p, _))| (i, *p))
        .chain(player.map(|p| (usize::MAX, p)))
        .collect()
}
/// Steering away from teammates inside SURVIVOR_SPACING, scaled by how deep
/// they are inside it (0 outside, ~1 when touching).
/// Fixed separating direction for two survivors on the same spot: opposite
/// for the two members of a pair.
fn pair_direction(me: usize, other: usize) -> Vec3 {
    let (lo, hi) = (me.min(other) as u64, me.max(other) as u64);
    let base = Quat::from_rotation_y((lo.wrapping_mul(7).wrapping_add(hi) % 1000) as f32 * 2.4) * Vec3::X;
    if me < other { base } else { -base }
}
fn survivor_spacing(me: usize, at: Vec3, others: &[(usize, Vec3)]) -> Vec3 {
    let mut push = Vec3::ZERO;
    for (key, p) in others {
        if (p.y - at.y).abs() > HEIGHT {
            continue;
        }
        let away = (at - *p) * Vec3::new(1., 0., 1.);
        let d = away.length();
        if d >= SURVIVOR_SPACING {
            continue;
        }
        // Exactly coincident: separate along a fixed per-pair direction.
        let dir = if d > 1e-4 { away / d } else { pair_direction(me, *key) };
        push += dir * (1. - d / SURVIVOR_SPACING);
    }
    push
}
/// Horizontal move that takes a bot out of any teammate hull it overlaps.
fn survivor_overlap(me: usize, at: Vec3, others: &[(usize, Vec3)]) -> Option<Vec3> {
    let mut push = Vec3::ZERO;
    for (key, p) in others {
        if (p.y - at.y).abs() > HEIGHT {
            continue;
        }
        let away = (at - *p) * Vec3::new(1., 0., 1.);
        let d = away.length();
        let min = 2. * RADIUS;
        if d >= min {
            continue;
        }
        let dir = if d > 1e-4 { away / d } else { pair_direction(me, *key) };
        push += dir * (min - d);
    }
    (push.length_squared() > 1e-8).then_some(push)
}
/// A direction near `dir` a bot can step in: `dir` itself when its hull has
/// room, else the closest of +-45 and +-90 degrees that does (an idle bot
/// pushed against a door jamb slides along the doorway instead).
fn clear_direction(c: &Collision, feet: Vec3, dir: Vec3) -> Vec3 {
    if dir == Vec3::ZERO {
        return dir;
    }
    let lift = 18. * UNIT;
    let center = feet + Vec3::Y * (HEIGHT * 0.5 + lift * 0.5);
    let extents = Vec3::new(RADIUS, (HEIGHT - lift) * 0.5, RADIUS);
    let free = |d: Vec3| c.trace(center, center + d * 8. * UNIT, extents).fraction > 0.99;
    [0f32, 45., -45., 90., -90.]
        .into_iter()
        .map(|a| Quat::from_rotation_y(a.to_radians()) * dir)
        .find(|&d| free(d))
        .unwrap_or(dir)
}
/// Deepest drop a bot will walk off: fall_speed_fatal (720 u/s) is reached
/// after 720^2 / (2 x 800) = 324 units, so retail bots never take this step
/// (their nav paths keep them on walkable ground).
const BOT_LETHAL_DROP: f32 = 300. * UNIT;
/// True when moving `velocity` for `dt` would put a grounded bot over a drop
/// deeper than BOT_LETHAL_DROP (a ledge, roof edge or window sill).
fn bot_lethal_drop_ahead(c: &Collision, feet: Vec3, velocity: Vec3, dt: f32) -> bool {
    let flat = velocity * Vec3::new(1., 0., 1.);
    let Some(direction) = flat.try_normalize() else { return false };
    let step = 18. * UNIT;
    let probe = feet + direction * (RADIUS + flat.length() * dt + 2. * UNIT) + Vec3::Y * step;
    // A wall ahead stops the bot anyway; that is no edge.
    if c.trace(feet + Vec3::Y * step, probe, Vec3::ZERO).fraction < 1. {
        return false;
    }
    let foot = Vec3::new(4. * UNIT, 0.01, 4. * UNIT);
    c.trace(probe, probe - Vec3::Y * (BOT_LETHAL_DROP + step), foot).fraction >= 1.
}
/// Bullet holes are not placed on surfaces that can move (doors, moving
/// brushes, breakables, props): the decal would not follow them. Only
/// these classes count; trigger, fog and other invisible brush volumes
/// cover whole areas and must not hide holes on the world behind them.
pub(crate) fn decal_on_movable(logic: Option<&crate::l4d2r_cinematic::MapLogic>, p: Vec3) -> bool {
    let Some(l) = logic else { return false };
    l.world.ents.iter().enumerate().any(|(i, e)| {
        let c = e.me.classname.as_str();
        let movable = c.starts_with("prop_")
            || c.starts_with("func_door")
            || matches!(c, "func_movelinear" | "func_tracktrain" | "func_rotating" | "func_breakable" | "func_breakable_surf" | "func_physbox" | "func_elevator" | "func_button");
        movable
            && e.alive
            && l.bounds(i).is_some_and(|(lo, hi)| (p.cmpge(lo - Vec3::splat(0.02)) & p.cmple(hi + Vec3::splat(0.02))).all())
    })
}
fn bot_speed(state: &Match) -> f32 {
    if state.health + state.temporary < 40. { 150. * UNIT } else { 220. * UNIT }
}
/// Retail bot movement type (server 1038ed10), the human-following part:
/// beyond sb_separation_danger_min_range (200) of the human a bot runs;
/// within sb_separation_range (200) it walks (z_walk_speed 85) while the
/// human's walk flag is set. Not reproduced: walking when moving away from
/// its view out of combat, and the threat-exposure walk of a leading bot.
fn bot_move_speed(state: &Match, human_walking: bool, human_distance: f32) -> f32 {
    let run = bot_speed(state);
    if human_walking && human_distance < BOT_WALK_RANGE { run.min(85. * UNIT) } else { run }
}
/// Retail 1038ed10: a bot that is not "still" (its still timer, 0x319c,
/// runs only while on the ground below still_vel_limit 5 units/s,
/// 102c9020) walks (85) when its movement points away from its view, so a
/// bot backing away from a threat it watches backpedals at walk speed.
fn backing_walk(wish: Vec3, aim: Vec3, velocity: Vec3) -> Vec3 {
    const STILL_VEL_LIMIT: f32 = 5.;
    let moving = velocity.xz().length() >= STILL_VEL_LIMIT * UNIT;
    let away = wish.xz().dot(aim.xz()) < 0.;
    if moving && away && wish.length() > 85. * UNIT {
        wish.normalize() * 85. * UNIT
    } else {
        wish
    }
}
/// sb_separation_range / sb_separation_danger_min_range (both 200).
const BOT_WALK_RANGE: f32 = 200. * UNIT;
fn squad_ai(
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    c: Res<Collision>,
    body: Single<&Body>,
    rules: Res<Rules>,
    mut s: ResMut<Match>,
    mut squad: ResMut<Squad>,
    navigation_state: (
        Option<ResMut<crate::l4d2r_navigation::Navigation>>,
        Option<Res<crate::campaign_flow::Checkpoint>>,
        Local<crate::bot_recovery::Recovery>,
        Option<Res<crate::special_infected::witch::Calm>>,
        Local<crate::ai_ladder::LadderClimbs>,
        Local<crate::bot_look::BotLook>,
        Local<Vec<(Vec3, Vec3)>>,
        Local<Vec<(crate::special_infected::ai::Victim, f32)>>,
        Local<crate::bot_retreat::Retreats>,
        Option<ResMut<crate::loot::scavenge::Scavenge>>,
        Local<crate::bot_look::Clearing>,
        Local<crate::bot_retreat::Assist>,
        (
            Local<crate::bot_retreat::BattleStations>,
            Local<f32>,
            Local<crate::bot_retreat::Regroups>,
            Option<Res<crate::l4d2r_cinematic::PlatformRiders>>,
            Local<Vec<Vec3>>,
        ),
    ),
    mut actors: Query<(&CompanionActor, &mut Transform), Without<Enemy>>,
    mut enemies: Query<(Entity, &mut Transform, &mut Enemy), Without<CompanionActor>>,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut impacts: Option<ResMut<crate::bullet_impacts::Impacts>>,
    glow_fixture: Option<Res<crate::survivor_glow::GlowFixture>>,
    native: Option<Res<crate::source_director::Director>>,
    mut sounds: Option<ResMut<game_audio::Queue>>,
    mut animation_state: (
        Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
        Option<Res<crate::l4d2r_animation::PlaybackState>>,
        Option<Res<crate::campaign_settings::SessionSettings>>,
        Option<ResMut<crate::upgrades::Effects>>,
        Commands,
        Local<crate::bot_combat::Aim>,
    ),
) {
    let _timing = crate::performance::scope("game::squad_ai");
    let (mut navigation, checkpoint, mut recovery, calm_witches, mut bot_ladders, mut bot_look, mut bot_velocity, mut held_time, mut retreats, mut scavenge, mut clearing, mut assist, (mut stations, mut human_held, mut regroups, riders_on, mut intent)) = navigation_state;
    bot_velocity.resize(squad.0.len(), (Vec3::ZERO, Vec3::NAN));
    let checkpoint_goal = checkpoint.as_ref().and_then(|c| c.goal);
    if s.infected
        || s.paused
        || glow_fixture.is_some()
        || cinematic.as_ref().is_some_and(|c| c.active)
    {
        return;
    }
    let dt = time.delta_secs();
    let mut positions: Vec<_> = squad
        .0
        .iter()
        .map(|m| (m.position, !m.state.dead && !m.state.incap))
        .collect();
    let friendly_positions = squad
        .0
        .iter()
        .map(|m| {
            (
                m.position,
                (!m.state.dead).then_some(if m.state.incap {
                    0.65
                } else {
                    crate::special_infected::pinned_height(&m.state).unwrap_or(HEIGHT)
                }),
            )
        })
        .collect::<Vec<_>>();
    let door_goal = checkpoint.as_ref().and_then(|c| c.door);
    // Retail SurvivorLegsCoverFriendInCombat: calm bots move to support a
    // teammate in combat until within ASSIST_RANGE with a clear view.
    let combat_now = time.elapsed_secs();
    assist.observe(crate::bot_look::PLAYER, s.shot_serial, s.damage_serial, combat_now);
    for (i, m) in squad.0.iter().enumerate() {
        assist.observe(i, m.state.shot_serial, m.state.damage_serial, combat_now);
    }
    let mut combatants: Vec<(usize, Vec3, bool)> = squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .map(|(i, m)| (i, m.position, assist.in_combat(i, m.state.pinned, combat_now)))
        .collect();
    if !s.dead && !s.infected {
        combatants.push((
            crate::bot_look::PLAYER,
            body.feet,
            assist.in_combat(crate::bot_look::PLAYER, s.pinned, combat_now),
        ));
    }
    let assist_goals: Vec<Option<Vec3>> = (0..squad.0.len())
        .map(|i| {
            let m = &squad.0[i];
            let calm = scavenge.as_ref().map_or(f32::INFINITY, |sc| sc.calm(i));
            if m.state.dead || m.state.incap || m.state.pinned || calm < crate::bot_retreat::ASSIST_CALM {
                assist.forget(i);
                return None;
            }
            let eye = m.position + Vec3::Y * EYE;
            let friends: Vec<_> = combatants
                .iter()
                .map(|(key, at, combat)| {
                    let in_position = *combat
                        && at.distance(m.position) <= crate::bot_retreat::ASSIST_RANGE
                        && {
                            let t = c.sight_trace(eye, *at + Vec3::Y * EYE);
                            !t.start_solid && t.fraction >= 1.
                        };
                    (*key, *at, *combat, in_position)
                })
                .collect();
            assist.choose(i, m.position, &friends)
        })
        .collect();
    // Retail SurvivorLegsBattleStations: in battlefield/finale areas, bots
    // take up the stations nearest a human who holds position.
    *human_held = if (body.velocity * Vec3::new(1., 0., 1.)).length() < 10. * UNIT { *human_held + dt } else { 0. };
    let tank_alive = enemies.iter().any(|(_, _, e)| {
        e.health > 0.
            && crate::special_infected::SpecialClass::from_model(&e.model)
                == Some(crate::special_infected::SpecialClass::Tank)
    });
    let mut survivor_feet: Vec<Vec3> = squad.0.iter().filter(|m| !m.state.dead).map(|m| m.position).collect();
    if !s.dead && !s.infected {
        survivor_feet.push(body.feet);
    }
    let station_world = crate::bot_retreat::StationWorld {
        now: combat_now,
        human: (!s.dead && !s.infected).then_some(body.feet),
        human_held: *human_held,
        tank_alive,
        survivors: &survivor_feet,
    };
    let station_goals: Vec<Option<Vec3>> = (0..squad.0.len())
        .map(|i| {
            let m = &squad.0[i];
            if m.state.dead || m.state.incap || m.state.pinned {
                stations.forget(i);
                return None;
            }
            navigation.as_deref().and_then(|nav| stations.goal(i, m.position, nav, &station_world))
        })
        .collect();
    // Retail SurvivorLegsRegroup: "I've lost sight of the team".
    let mut teammates: Vec<(usize, Vec3)> = squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .map(|(i, m)| (i, m.position))
        .collect();
    if !s.dead && !s.infected {
        teammates.push((crate::bot_look::PLAYER, body.feet));
    }
    let regroup_goals: Vec<Option<Vec3>> = (0..squad.0.len())
        .map(|i| {
            let m = &squad.0[i];
            if m.state.dead || m.state.incap || m.state.pinned {
                regroups.forget(i);
                return None;
            }
            let eye = m.position + Vec3::Y * EYE;
            let seen: Vec<(usize, Vec3, bool)> = teammates
                .iter()
                .map(|(k, at)| {
                    let t = c.sight_trace(eye, *at + Vec3::Y * EYE);
                    (*k, *at, *k != i && !t.start_solid && t.fraction >= 1.)
                })
                .collect();
            regroups.update(i, m.position, &seen, dt)
        })
        .collect();
    // Retail "Joining team leader on a platform" (FUN_103bf350): when the
    // human rides an elevator, a bot not on that same elevator goes to them.
    let platform_goals: Vec<Option<Vec3>> = (0..squad.0.len())
        .map(|i| {
            let on = riders_on.as_ref()?;
            let platform = on.player.filter(|_| !s.dead && !s.infected)?;
            let m = &squad.0[i];
            (!m.state.dead && !m.state.incap && !m.state.pinned && on.bots.get(i).copied().flatten() != Some(platform))
                .then_some(body.feet)
        })
        .collect();
    // A hanging player is helped up from the top of the ledge.
    let player_spot = s.ledge.map_or(body.feet, |h| h.stand);
    let goals: Vec<_> = (0..squad.0.len())
        .map(|i| {
            door_goal
                .filter(|(bot, _)| *bot == i)
                .map(|(_, at)| at)
                .or(checkpoint_goal)
                .unwrap_or_else(|| {
                    // Rescues and heals come first; otherwise a bot collecting
                    // an item (retail SurvivorCollectObject) heads for it.
                    let rescue = crate::survivor_items::rescue_goal(&s, player_spot, &squad, i);
                    if rescue == player_spot {
                        scavenge
                            .as_ref()
                            .and_then(|sc| sc.goal(i, squad.0[i].position))
                            .or(station_goals[i])
                            .or(assist_goals[i])
                            .or(regroup_goals[i])
                            .or(platform_goals[i])
                            .unwrap_or(rescue)
                    } else {
                        rescue
                    }
                })
        })
        .collect();
    // One reviver per downed player (retail m_reviveOwner): the bot already
    // reviving keeps it, otherwise the nearest bot in reach.
    // Nobody can revive a survivor a special infected is still holding.
    let player_reviver = (s.incap && !s.pinned).then(|| {
        let eligible = |m: &crate::game_state::Companion| {
            !m.state.dead && !m.state.incap && !m.state.pinned && m.position.distance(player_spot) < 2.
        };
        squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| eligible(m))
            .min_by(|a, b| {
                (a.1.revive_time <= 0.)
                    .cmp(&(b.1.revive_time <= 0.))
                    .then(a.1.position.distance(player_spot).total_cmp(&b.1.position.distance(player_spot)))
            })
            .map(|(i, _)| i)
    }).flatten();
    s.reviving = None;
    // Teammates held by a special: bots close in to rescue them, and a
    // Smoker's victim is freed by shoving the victim (tongue break).
    use crate::special_infected::ai::Victim;
    let player_held = !s.infected && !s.dead && s.pinned;
    let mut held_spots: Vec<(Victim, Vec3, bool)> = squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead && m.state.pinned)
        .map(|(i, m)| (Victim::Bot(i), m.position, m.state.pinned_by == Some(crate::special_infected::SpecialClass::Smoker)))
        .collect();
    if player_held {
        held_spots.push((Victim::Player, body.feet, s.pinned_by == Some(crate::special_infected::SpecialClass::Smoker)));
    }
    // Bots notice an immobilized teammate after
    // sb_friend_immobilized_reaction_time_{normal 2.0, hard 1.0, expert 0.5}.
    let reaction = match animation_state.2.as_ref().map(|s| s.difficulty) {
        Some(crate::campaign_settings::Difficulty::Expert) => 0.5,
        Some(crate::campaign_settings::Difficulty::Advanced) => 1.0,
        _ => 2.0,
    };
    held_time.retain(|(v, _)| held_spots.iter().any(|(h, _, _)| h == v));
    for (v, _, _) in &held_spots {
        match held_time.iter_mut().find(|(h, _)| h == v) {
            Some((_, t)) => *t += dt,
            None => held_time.push((*v, dt)),
        }
    }
    held_spots.retain(|(v, _, _)| held_time.iter().any(|(h, t)| h == v && *t >= reaction));
    let holders = if held_spots.is_empty() { vec![] } else { crate::special_infected::ai::holders() };
    // Threats bots back away from (retail SurvivorLegsRetreat).
    let pinning = crate::special_infected::ai::holders();
    let threats: Vec<crate::bot_retreat::Threat> = enemies
        .iter()
        .filter(|(_, _, e)| e.health > 0.)
        .filter_map(|(id, t, e)| {
            let calm = calm_witches.as_ref().is_some_and(|w| w.0.contains(&id));
            crate::bot_retreat::danger_range(&e.model, pinning.contains(&id), calm)
                .map(|range| crate::bot_retreat::Threat { id, position: t.translation, range })
        })
        .collect();
    if let Some(logic) = cinematic.as_ref() { bot_ladders.bind_map(&logic.world.bsp); }
    // Each bot's travel direction (zero when standing): this tick's for
    // lower indices, which are decided first, the last tick's for the rest.
    intent.resize(squad.0.len(), Vec3::ZERO);
    for (index, member) in squad.0.iter_mut().enumerate() {
        // Played by someone else (multiplayer): no bot thinking at all.
        if crate::game_state::remote(index) {
            recovery.reset(index);
            bot_ladders.cancel(index as u64);
            animation_state.5.forget(index);
            member.state.ladder_pose = None;
            positions[index].0 = member.position;
            // A downed remote player is revived here like a bot (bleeding
            // out stays with its owner); net_play sends the revive to them.
            let player_help = !s.incap
                && keys.pressed(KeyCode::KeyE)
                && body.feet.distance(member.position) < 2.;
            let help = positions
                .iter()
                .enumerate()
                .any(|(i, (p, alive))| i != index && *alive && p.distance(member.position) < 2.);
            if member.state.incap && !member.state.dead && !member.state.pinned && (help || player_help) {
                if player_help {
                    s.reviving = Some(index);
                }
                member.revive_time += dt;
                if member.revive_time >= if s.adrenaline_time > 0. { 2.5 } else { 5. } {
                    member.state.revive();
                    member.revive_time = 0.;
                }
            } else {
                member.revive_time = 0.;
            }
            continue;
        }
        if index == 0 && member.state.elapsed == 0.
            && cinematic.as_ref().is_some_and(|l| !l.world.find("lab_ladder_team_0").is_empty()) {
            crate::ai_ladder::capture_bot(&mut bot_ladders, &c, member, index);
        }
        member.state.ladder_pose = None;
        {
            // kick <survivor>: the bot leaves the game.
            let mut kicks = crate::game_state::KICKS.lock().unwrap();
            let character = |n: &str| match n {
                "rochelle" => "producer", "coach" => "coach", "nick" => "gambler", "ellis" => "mechanic",
                "bill" => "namvet", "zoey" => "teenangst", "louis" => "manager", "francis" => "biker", _ => "",
            };
            if let Some(i) = kicks.iter().position(|n| !character(n).is_empty() && member.model.contains(character(n))) {
                kicks.remove(i);
                member.state.dead = true;
                member.state.incap = false;
                member.state.health = 0.;
                crate::game_state::set_kicked(index);
            }
        }
        if member.state.dead || member.state.pinned || member.state.incap {
            bot_ladders.cancel(index as u64);
        }
        let held = rules.weapons.get(&member.state.gun().class).unwrap_or(&rules.pistol);
        animation_state.5.advance(index, dt, held);
        member.state.temporary = (member.state.temporary
            - dt * native.as_ref().map_or(0.27, |d| d.temporary_decay))
        .max(0.);
        member.state.adrenaline_time = (member.state.adrenaline_time - dt).max(0.);
        if member.state.dead {
            animation_state.5.forget(index);
            recovery.reset(index);
            continue;
        }
        if member.state.pinned {
            animation_state.5.forget(index);
            // The special's carry/drag/pummel controller owns this origin.
            // Following, recovery teleports, sb_stop gravity and reviving
            // must not compete with it (including incapacitated victims).
            recovery.reset(index);
            member.vertical = 0.;
            member.revive_time = 0.;
            if member.state.incap {
                member.state.incap_health = (member.state.incap_health - dt * 3.).max(0.);
                member.state.dead = member.state.incap_health <= 0.;
            }
            continue;
        }
        if member.state.incap {
            recovery.reset(index);
            member.state.incap_health = (member.state.incap_health - dt * 3.).max(0.);
            member.state.dead = member.state.incap_health <= 0.;
            let help = positions
                .iter()
                .enumerate()
                .any(|(i, (p, alive))| i != index && *alive && p.distance(member.position) < 2.);
            // `--capture-hold-use`: captures hold use (revive views).
            static HOLD_USE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            let hold = *HOLD_USE.get_or_init(|| std::env::args().any(|a| a == "--capture-hold-use"));
            let player_help = !s.incap
                && !member.state.pinned
                && (keys.pressed(KeyCode::KeyE) || hold)
                && body.feet.distance(member.position) < 2.;
            if player_help {
                s.reviving = Some(index);
            }
            if (help || player_help) && !member.state.pinned {
                member.revive_time += dt;
                if member.revive_time >= if s.adrenaline_time > 0. { 2.5 } else { 5. } {
                    member.state.revive();
                    member.revive_time = 0.;
                }
            } else {
                member.revive_time = 0.;
            }
            continue;
        }
        {
            // bot_setpos: survivor name -> model character.
            let mut placements = crate::game_state::BOT_PLACEMENTS.lock().unwrap();
            let character = |n: &str| match n {
                "rochelle" => "producer", "coach" => "coach", "nick" => "gambler", "ellis" => "mechanic",
                "bill" => "namvet", "zoey" => "teenangst", "louis" => "manager", "francis" => "biker", _ => "",
            };
            if let Some(i) = placements.iter().position(|(n, _, _)| !character(n).is_empty() && member.model.contains(character(n))) {
                let (_, p, yaw) = placements.remove(i);
                member.position = Vec3::from_array(binary::source_position(p));
                if yaw.is_finite() {
                    let r = yaw.to_radians();
                    member.aim = Vec3::from_array(binary::source_position([r.cos(), r.sin(), 0.])).normalize_or(member.aim);
                }
                positions[index].0 = member.position;
                println!("bot_setpos applied: {} at {:?}", member.model, member.position / 0.0254);
            }
        }
        // L4D2R_BOT_POS: debug, each bot's position (Source units) once a second.
        if std::env::var_os("L4D2R_BOT_POS").is_some() && (time.elapsed_secs() * 60.) as u32 % 60 == 0 {
            let p = member.position / 0.0254;
            println!("L4D2R bot {} at ({:.0} {:.0} {:.0})", member.model, p.x, -p.z, p.y);
        }
        if member.state.action > 0. {
            recovery.reset(index);
            continue;
        }
        if crate::game_state::bots_stopped() {
            // sb_stop: stand in place under gravity; no moves, revives or attacks.
            recovery.reset(index);
            member.revive_time = 0.;
            let damage=crate::fall_damage::move_bot(member,&c,Vec3::ZERO,dt,native.as_ref().map_or_else(Default::default,|d|d.fall_damage));
            if damage>0. { if let Some(sounds)=sounds.as_mut() {sounds.play("Player.FallDamage",Some(member.position));} }
            positions[index].0 = member.position;
            continue;
        }
        if !s.dead
            && !s.incap
            && body.grounded
            && recovery.due(index, s.elapsed, member.position, body.feet)
        {
            if let Some(point) =
                crate::bot_recovery::landing(&c, body.feet, body.yaw, index, &positions)
            {
                member.position = point;
                positions[index].0 = point;
                member.vertical = 0.;
                member.revive_time = 0.;
                recovery.reset(index);
                if let Some(nav) = navigation.as_mut() {
                    nav.reset_survivor(index);
                }
            }
        }
        member.state.elapsed += dt;
        member.state.shove = (member.state.shove - dt).max(0.);
        let held = rules
            .weapons
            .get(&member.state.gun().class)
            .unwrap_or(&rules.pistol);
        member.state.advance_reload(held, dt);
        let goal = goals[index];
        let distance = member.position.distance(goal);
        if s.incap && distance < 2. && player_reviver == Some(index) {
            member.revive_time += dt;
            if member.revive_time >= 5. {
                s.revive();
                member.revive_time = 0.;
            }
        } else {
            member.revive_time = 0.;
        }
        // On a NAV ladder: climb at the survivor ladder speed (retail
        // 100 units/s, ladders-retail-1019); no shooting while climbing.
        if member.state.incap || member.state.dead {
            bot_ladders.cancel(index as u64);
        } else if let Some(position) =
            bot_ladders.advance(index as u64, &c, member.position, 100. * UNIT, dt)
        {
            member.position = position;
            member.state.ladder_pose = bot_ladders.pose(index as u64);
            member.vertical = 0.;
            positions[index].0 = position;
            continue;
        }
        let toward = navigation.as_mut().map_or(goal - member.position, |nav| {
            nav.toward(index, member.position, goal, dt)
        }) * Vec3::new(1., 0., 1.);
        if let Some(step) = navigation
            .as_mut()
            .and_then(|nav| nav.ladder_survivor(index, member.position, goal))
        {
            bot_ladders.begin(index as u64, &c, member.position, step);
        }
        // Rescuing a held teammate: come within shove reach (1.3 m).
        let rescuing = held_spots.iter().any(|(_, p, _)| p.distance(goal) < 0.05);
        let wish = if distance
            > if rescuing {
                0.9
            } else if s.incap {
                1.4
            } else {
                if checkpoint_goal.is_some()
                    || assist.target(index).is_some()
                    || station_goals[index].is_some()
                    || regroups.active(index)
                    || platform_goals[index].is_some()
                    || goal.distance(body.feet) > 0.1
                    || s.dead
                    || member.state.medkit && (s.health < 35. || s.black_white)
                    || crate::survivor_items::wants_pills(&member.state, &s)
                {
                    1.4
                } else {
                    // Retail bots regroup whenever they are farther than
                    // sb_separation_range (200) from the human and settle
                    // 170-210 units away (c2m1 recordings, bots-1027).
                    BOT_SEPARATION_RANGE
                }
            } {
            navigation.as_mut().map_or_else(
                || c.steer(member.position, toward, HEIGHT),
                |nav| {
                    nav.steer(
                        index,
                        &c,
                        member.position,
                        toward,
                        HEIGHT,
                        time.elapsed_secs(),
                    )
                },
            ) * bot_move_speed(&member.state, s.walk_flag, member.position.distance(body.feet))
        } else {
            Vec3::ZERO
        };
        // Back away from a dangerous threat in view (Tank, enraged Witch,
        // Boomer, a close special), still shooting it; not mid-revive.
        let eye = member.position + Vec3::Y * EYE;
        let retreat = if member.revive_time > 0. || threats.is_empty() {
            retreats.reset(index);
            None
        } else {
            retreats.update(index, member.position, &threats, dt, |p| {
                let t = c.sight_trace(eye, p + Vec3::Y);
                !t.start_solid && t.fraction >= 1.
            })
        };
        let wish = match retreat {
            Some(away) => {
                let mates: Vec<Vec3> = survivor_obstacles(index, &positions, (!s.dead && !s.incap).then_some(body.feet))
                    .into_iter()
                    .map(|(_, p)| p)
                    .collect();
                let away = crate::bot_retreat::around(away, member.position, &mates);
                c.steer(member.position, away, HEIGHT) * bot_move_speed(&member.state, s.walk_flag, member.position.distance(body.feet))
            }
            None => wish,
        };
        // Retail survivors are solid to each other and bots keep out of a
        // teammate's way: steer off anyone inside SURVIVOR_SPACING, and step
        // aside while idle instead of standing inside them.
        let player_solid = !s.infected && !s.dead && !s.incap;
        let others = survivor_obstacles(index, &positions, player_solid.then_some(body.feet));
        let crowd = survivor_spacing(index, member.position, &others);
        intent[index] = Vec3::ZERO;
        let wish = if wish.length_squared() > 1e-6 {
            let speed = wish.length();
            let travel = wish / speed;
            // Taking turns at narrow spots: a moving bot gives way to a
            // moving bot of lower index just ahead or beside it, so two bots
            // never wedge each other in a doorway. Otherwise it keeps clear
            // only of the player and lower-index bots; higher-index bots in
            // its way give way or step aside themselves.
            intent[index] = travel;
            let moving = |k: usize| intent.get(k).is_some_and(|t| *t != Vec3::ZERO);
            let ahead: Vec<_> = others
                .iter()
                .filter(|(k, p)| {
                    let d = (*p - member.position) * Vec3::new(1., 0., 1.);
                    *k < index
                        && moving(*k)
                        && (p.y - member.position.y).abs() < HEIGHT
                        && d.length() < SURVIVOR_SPACING
                        && d.normalize_or_zero().dot(travel) > -0.2
                })
                .copied()
                .collect();
            if ahead.is_empty() {
                let leaders: Vec<_> = others.iter().filter(|(k, _)| *k < index || *k == usize::MAX).copied().collect();
                let crowd = survivor_spacing(index, member.position, &leaders);
                (travel + crowd).normalize_or(travel) * speed
            } else {
                let away = survivor_spacing(index, member.position, &ahead).normalize_or_zero();
                clear_direction(&c, member.position, away) * bot_move_speed(&member.state, s.walk_flag, member.position.distance(body.feet)) * 0.5
            }
        } else if crowd.length() > 0.25 {
            // Standing in a moving teammate's way: step out of its line
            // rather than straight ahead of it.
            let mut away = crowd;
            for (k, p) in &others {
                let Some(t) = intent.get(*k).copied().filter(|t| *t != Vec3::ZERO) else { continue };
                let rel = (member.position - *p) * Vec3::new(1., 0., 1.);
                if rel.length() >= SURVIVOR_SPACING || rel.dot(t) < 0. {
                    continue;
                }
                let side = rel - t * rel.dot(t);
                let side = side.try_normalize().unwrap_or_else(|| pair_direction(index, *k));
                away += side - t * crowd.dot(t).max(0.);
            }
            clear_direction(&c, member.position, away.normalize_or(crowd.normalize())) * bot_move_speed(&member.state, s.walk_flag, member.position.distance(body.feet)) * 0.5
        } else {
            wish
        };
        if distance > 3. {
            if let Some(logic) = cinematic.as_mut() {
                logic.open_near(
                    member.position + Vec3::Y * EYE,
                    toward.normalize_or_zero(),
                    &c,
                );
            }
        }
        // Retail bots are players: ground friction and acceleration
        // (player_movement::horizontal) ease every start, stop and turn.
        let grounded = c.grounded(member.position, HEIGHT);
        // (velocity, where this step left the bot): anything else that moved
        // it since (ladders, pins, revives, teleports) starts it from rest.
        if member.position != bot_velocity[index].1 {
            bot_velocity[index].0 = Vec3::ZERO;
        }
        let capped = backing_walk(wish, member.aim, bot_velocity[index].0);
        // L4D2R_BOT_SPEED_LOG: debug, each bot's speed and the walk cap about once a second.
        if std::env::var_os("L4D2R_BOT_SPEED_LOG").is_some() && (time.elapsed_secs() * 60.) as u32 % 60 == index as u32 {
            println!("Bot {index}: speed {:.0} wish {:.0} backpedal {}", bot_velocity[index].0.length() / UNIT,
                wish.length() / UNIT, capped.length() + 1e-4 < wish.length());
        }
        let wish = capped;
        let mut velocity = crate::player_movement::horizontal(
            bot_velocity[index].0,
            wish,
            wish.length(),
            grounded,
            dt,
        );
        // Never step off a lethal ledge (Hotel's window sills and roof edges):
        // crowd spacing, giving way or backing off a threat must not push a
        // bot over.
        if grounded && bot_lethal_drop_ahead(&c, member.position, velocity, dt) {
            velocity = Vec3::ZERO;
        }
        let before = member.position;
        let damage=crate::fall_damage::move_bot(member,&c,velocity,dt,native.as_ref().map_or_else(Default::default,|d|d.fall_damage));
        // Never end a step inside a teammate's hull.
        if let Some(push) = survivor_overlap(index, member.position, &others) {
            if !(grounded && bot_lethal_drop_ahead(&c, member.position, push, 1.)) {
                member.position = c.walk(member.position, push, HEIGHT, grounded);
            }
        }
        // Keep what collision actually allowed, so walls absorb velocity.
        if dt > 0. {
            let moved = (member.position - before) * Vec3::new(1., 0., 1.) / dt;
            bot_velocity[index].0 = if moved.length() > velocity.length() + 0.01 { velocity } else { moved };
        }
        bot_velocity[index].1 = member.position;
        if damage>0. { if let Some(sounds)=sounds.as_mut() {sounds.play("Player.FallDamage",Some(member.position));} }
        positions[index].0=member.position;
        if member.state.incap || member.state.dead {continue;}
        member.cooldown = (member.cooldown - dt).max(0.);
        if member.cooldown <= 0. {
            let weapon = rules
                .weapons
                .get(&member.state.gun().class)
                .unwrap_or(&rules.pistol);
            if member.state.gun().ammo == 0 && member.state.reload <= 0. {
                if weapon.infinite() || member.state.gun().reserve > 0 {
                    member.state.begin_reload(weapon);
                } else if member.state.slot == 1 {
                    member.state.slot = 2;
                    continue;
                }
            }
            let origin = member.position + Vec3::Y * EYE;
            // A teammate on a Smoker's tongue within reach: shove them free.
            if member.state.shove <= 0. {
                if let Some((victim, at, _)) = held_spots.iter().find(|(v, at, smoked)| {
                    *smoked && *v != Victim::Bot(index) && at.distance(member.position) < 1.3
                }) {
                    member.state.shove = 0.7;
                    member.state.shove_serial = member.state.shove_serial.wrapping_add(1);
                    member.aim = ((*at - member.position) * Vec3::new(1., 0., 1.)).normalize_or(member.aim);
                    crate::special_infected::ai::shove_victim(*victim);
                    continue;
                }
            }
            // Test near candidates first: selecting the nearest visible enemy
            // does not require tracing every enemy after a visible one is found.
            let mut candidates = enemies
                .iter()
                .filter(|(id, t, e)| {
                    e.health > 0.
                        // A special holding a teammate is a known threat
                        // wherever the bot happens to be looking.
                        && (crate::bot_combat::in_view(t.translation + Vec3::Y - origin, member.aim, member.state.vomited > 0.)
                            || holders.contains(id)
                            // A close infected the bot can hear (running or
                            // attacking) is a threat even behind it.
                            || ((e.attack > 0. || e.movement_speed > 1.)
                                && t.translation.distance(member.position) <= crate::bot_combat::CLOSE_THREAT_RANGE))
                        // Bots hold fire on a witch that has not been startled.
                        && !calm_witches.as_ref().is_some_and(|w| w.0.contains(id))
                })
                .map(|(entity, t, _)| (entity, t.translation))
                .collect::<Vec<_>>();
            // Specials holding a teammate first, then nearest.
            candidates.sort_by(|a, b| {
                (!holders.contains(&a.0))
                    .cmp(&!holders.contains(&b.0))
                    .then(a.1.distance_squared(origin).total_cmp(&b.1.distance_squared(origin)))
            });
            let target = candidates.into_iter().find(|(_, position)| {
                let sight = c.sight_trace(origin, *position + Vec3::Y);
                !sight.start_solid && sight.fraction >= 1.
            });
            if let Some((entity, position)) = target {
                // "Situation changed - giving up": a bot in a fight drops
                // the item it was collecting.
                if let Some(scavenge) = scavenge.as_mut() {
                    scavenge.fighting(index);
                }
                let ready = animation_state.5.observe(index, entity.to_bits(), position + Vec3::Y - origin, &mut member.aim, dt);
                if let Ok((_, mut transform, mut enemy)) = enemies.get_mut(entity) {
                    let incoming = (position + Vec3::Y - origin).normalize_or_zero();
                    if member.position.distance(position) < 1.3 && member.state.shove <= 0. {
                        member.state.shove = 0.7;
                        member.state.shove_serial = member.state.shove_serial.wrapping_add(1);
                        enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
                        enemy.reaction = 1;
                        enemy.attack = animation_state.0.as_ref().map_or(0.7, |a| {
                            a.infected_reaction_duration(
                                &enemy.model,
                                "ACT_TERROR_SHOVED_BACKWARD",
                                entity.to_bits(),
                            )
                        });
                        if crate::infected_population::Kind::model(&enemy.model)
                            == crate::infected_population::Kind::Riot
                        {
                            transform.rotation *= Quat::from_rotation_y(std::f32::consts::PI);
                        } else {
                            enemy.health -= 5.;
                            crate::game_ui::kill_feed::hit(entity, crate::game_ui::kill_feed::Killer::Bot(index));
                        }
                        transform.translation = c.walk(
                            transform.translation,
                            incoming * Vec3::new(1., 0., 1.) * 0.55,
                            HEIGHT,
                            c.grounded(transform.translation, HEIGHT),
                        );
                        member.cooldown = 0.7;
                        if let Some(queue) = sounds.as_mut() {
                            queue.play(
                                if crate::infected_population::Kind::model(&enemy.model)
                                    == crate::infected_population::Kind::Clown
                                {
                                    "Weapon.HitClown"
                                } else {
                                    "Zombie.Shoved"
                                },
                                Some(position),
                            );
                        }
                        if enemy.health <= 0. {
                            enemy.record_death(
                                crate::infected_death::Cause::Blunt,
                                &transform,
                                incoming,
                                1.,
                            );
                            enemy.corpse_time = 12.;
                            member.state.kills += 1;
                        }
                        continue;
                    }
                }
                if !ready || member.state.reload > 0. || member.state.gun().ammo == 0 {
                    continue;
                }
                let incoming = member.aim;
                let serial = member.state.shot_serial.wrapping_add(1);
                // Evaluate each live actor's posed boxes once for this shot, not per pellet.
                let targets = enemies
                    .iter()
                    .filter(|(_, _, e)| e.health > 0.)
                    .map(|(id, t, e)| {
                        (
                            id,
                            *t,
                            crate::infected_population::Kind::model(&e.model),
                            animation_state.0.as_ref().and_then(|a| {
                                a.hitboxes(&e.model, id, t, animation_state.1.as_deref())
                            }),
                            e.model.contains("hulk"),
                        )
                    })
                    .collect::<Vec<_>>();
                let mut marks = Vec::new();
                let mut hits = Vec::new();
                let mut rock_shots = Vec::new();
                let mut obstructed_by_survivor = false;
                for ray in animation_state.5.rays(index, weapon, incoming, serial, wish.length() / bot_speed(&member.state), member.state.gun().laser) {
                    // Retail penetration (bullet_penetration).
                    let candidates: Vec<_> = targets
                        .iter()
                        .filter_map(|(id, t, kind, boxes, tank)| {
                            let hit = match boxes {
                                Some(boxes) => boxes
                                    .trace(origin, ray, weapon.range)
                                    .map(|h| (h.distance, h.group, Some(h.index))),
                                None => crate::ray_box(
                                    origin,
                                    ray,
                                    t.translation + Vec3::new(-0.35, 0., -0.35),
                                    t.translation + Vec3::new(0.35, 1.8, 0.35),
                                )
                                .map(|d| (d, 2, None)),
                            }?;
                            Some(crate::bullet_penetration::Target {
                                distance: hit.0,
                                tank: *tank,
                                absorbs: kind.blocks_front(t, ray),
                                value: (*id, *t, *kind, hit.0, hit.1, ray, hit.2),
                            })
                        })
                        .collect();
                    let path = crate::bullet_penetration::walk(
                        weapon.penetration,
                        weapon.range,
                        candidates,
                        crate::bullet_penetration::world_wall(&c, origin, ray, weapon.range),
                    );
                    let limit = path.hits.first().map_or(path.end, |h| h.0);
                    rock_shots.push((ray, path.end));
                    let blocks = |p: Vec3, height: f32| {
                        crate::ray_box(
                            origin,
                            ray,
                            p + Vec3::new(-RADIUS, 0., -RADIUS),
                            p + Vec3::new(RADIUS, height, RADIUS),
                        )
                        .is_some_and(|d| d <= limit)
                    };
                    // Conservative shot veto, including incapacitated teammates.
                    if (!s.dead
                        && blocks(
                            body.feet,
                            if s.incap {
                                0.65
                            } else if let Some(h) = crate::special_infected::pinned_height(&s) {
                                h
                            } else if body.crouched {
                                crate::collision::DUCK_HEIGHT
                            } else {
                                HEIGHT
                            },
                        ))
                        || friendly_positions
                            .iter()
                            .enumerate()
                            .any(|(i, (p, height))| {
                                i != index && height.is_some_and(|h| blocks(*p, h))
                            })
                    {
                        obstructed_by_survivor = true;
                        break;
                    }
                    hits.extend(path.hits.into_iter().map(|h| h.1));
                    marks.extend(path.marks.iter().map(|&(distance, normal, material)| (origin + ray * distance, normal, material)));
                }
                if obstructed_by_survivor {
                    continue;
                }
                if let Some(impacts) = impacts.as_mut() {
                    impacts.hits.extend(marks.into_iter().filter(|(p, _, _)| !decal_on_movable(cinematic.as_deref(), *p)));
                }
                member.state.gun_mut().ammo -= 1;
                animation_state.5.fired(index, weapon);
                // Bot bullets also hit thrown Tank rocks (z_tank_throw_health).
                for (ray, reach) in rock_shots {
                    crate::special_infected::tank::shot_at_rocks(origin, ray, reach, weapon.damage_at(reach));
                }
                let upgraded = if member.state.slot == 1 {
                    crate::upgrades::take_round(member.state.gun_mut())
                } else {
                    None
                };
                member.state.shot_serial = serial;
                member.cooldown = weapon.shot_cycle(member.state.gun().ammo, false);
                if let Some(queue) = sounds.as_mut() {
                    queue.play(&weapon.fire_sound, Some(origin));
                }
                // All rays are resolved against the pre-shot snapshot. A lethal pellet
                // does not allow later pellets in the same shot through that actor.
                let mut damage = HashMap::<Entity, (f32, Vec3, Vec3, bool)>::new();
                for (id, transform, kind, distance, group, ray, box_index) in hits {
                    if kind.blocks_front(&transform, ray) {
                        continue;
                    }
                    let point = origin + ray * distance;
                    if let (Some(impacts), Some(box_index)) = (impacts.as_mut(), box_index) {
                        impacts.gore.push(crate::infected_gore::Hit {
                            damage_force_source: if upgraded.is_some() {
                                None
                            } else {
                                crate::infected_gore::bullet_damage_force_source(&weapon.class, ray)
                            },
                            entity: id, position: point, direction: ray, box_index, distance,
                            damage_type: if upgraded == Some(crate::upgrades::Kind::Explosive) {
                                crate::infected_gore::DMG_BLAST
                            } else {
                                crate::infected_gore::Hit::bullet_type(&weapon.class, Some(group as i32))
                            },
                            sniper: crate::infected_gore::Hit::is_sniper(&weapon.class),
                            swing: None, back: false, decapitates: false, seed: serial,
                            cause: if upgraded == Some(crate::upgrades::Kind::Explosive) { crate::infected_death::Cause::Explosion }
                                else if weapon.pellets > 1 { crate::infected_death::Cause::Shotgun }
                                else { crate::infected_death::Cause::Bullet },
                        });
                    }
                    let multiplier = if group == 1 {
                        1.
                    } else {
                        animation_state.2.as_ref().map_or(1., |s| {
                            crate::studio_hitboxes::body_damage_factor(s.difficulty)
                        })
                    };
                    let hit = damage.entry(id).or_insert((0., point, ray, false));
                    hit.0 += weapon.damage_at(distance) * multiplier;
                    hit.3 |= group == 1;
                }
                for (id, (damage, point, ray, head)) in damage {
                    if damage <= 0. {
                        continue;
                    }
                    if let Ok((_, transform, mut enemy)) = enemies.get_mut(id) {
                        enemy.health -= damage;
                        crate::game_ui::kill_feed::hit(id, crate::game_ui::kill_feed::Killer::Bot(index));
                        if upgraded == Some(crate::upgrades::Kind::Incendiary)
                            && !crate::infected_population::Kind::model(&enemy.model).fireproof()
                        {
                            enemy.burning = 4.;
                            animation_state
                                .4
                                .entity(id)
                                .insert(crate::upgrades::Burn(4.));
                        }
                        if let Some(kind) = upgraded {
                            if let Some(fx) = animation_state.3.as_mut() {
                                fx.hits.push((kind, point));
                            }
                        }

                        enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
                        // Commons flinch as a gesture and keep moving.
                        if !enemy.model.contains("/common_") {
                            enemy.reaction = 2;
                            enemy.attack = 0.5;
                        }
                        if let Some(impacts) = impacts.as_mut() {
                            impacts.blood.push(crate::bullet_impacts::BloodHit {
                                position: point,
                                direction: ray,
                                shotgun: weapon.pellets > 1,
                            });
                        }
                        if enemy.health <= 0. {
                            enemy.record_death(
                                if upgraded == Some(crate::upgrades::Kind::Explosive) {
                                    crate::infected_death::Cause::Explosion
                                } else if weapon.pellets > 1 {
                                    crate::infected_death::Cause::Shotgun
                                } else {
                                    crate::infected_death::Cause::Bullet
                                },
                                &transform,
                                ray,
                                if head { 1.5 } else { 1. },
                            );
                            enemy.death.far = point.distance(origin) / UNIT > 500.;
                            member.state.kills += 1;
                            enemy.corpse_time = 12.;
                        }
                    }
                }
            } else {
                animation_state.5.forget(index);
            }
        }
    }
    // Living survivors other bots may glance at (bot_look friends).
    let mut friends: Vec<(usize, Vec3)> = squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.state.dead)
        .map(|(i, m)| (i, m.position + Vec3::Y * if m.state.incap { 0.5 } else { EYE }))
        .collect();
    if !s.dead {
        friends.push((crate::bot_look::PLAYER, body.feet + Vec3::Y * if s.incap { s.downed_eye() } else { EYE }));
    }
    let now = time.elapsed_secs();
    // Area clearing: bots queue the areas a new step reveals, then every
    // survivor clears what it can see completely.
    if let Some(nav) = navigation.as_deref() {
        clearing.sync(cinematic.as_ref().map_or(0, |l| l.generation), crate::bot_look::Areas::count(nav));
        for (index, member) in squad.0.iter().enumerate() {
            if member.state.dead || member.state.incap || member.state.pinned {
                clearing.forget(index);
            } else {
                clearing.enter(index, nav, member.position);
            }
        }
        let clear = !native.as_ref().is_some_and(|d| d.dont_clear());
        for member in squad.0.iter().filter(|m| !m.state.dead && clear) {
            clearing.mark(nav, member.position);
        }
        if !s.dead {
            if clear {
                clearing.mark(nav, body.feet);
            }
            if !s.infected {
                clearing.mark_player(nav, body.feet, clear);
                crate::survivor_music::area_revealed(clearing.take_revealed());
            }
        }
    }
    let squad_view: Vec<(Vec3, Vec3)> = squad.0.iter().map(|m| (m.position, m.aim)).collect();
    for (actor, mut transform) in &mut actors {
        let member = &mut squad.0[actor.0];
        // The articulated corpse owns its world pose after death. Rotating
        // this root toward the player would move all of its resting bones.
        if member.state.dead || member.state.pinned {
            bot_look.forget(actor.0);
            continue;
        }
        transform.translation = member.position;
        if crate::game_state::remote(actor.0) {
            bot_look.forget(actor.0);
            let flat = member.aim * Vec3::new(1., 0., 1.);
            if flat.length_squared() > 1e-4 {
                transform.rotation = Quat::from_rotation_y((-flat.z).atan2(flat.x));
            }
            continue;
        }
        if let Some(pose) = member.state.ladder_pose {
            transform.rotation = pose.rotation();
            member.aim = pose.facing;
            continue;
        }
        let toward = if member.cooldown > 0. || member.state.reload > 0. || animation_state.5.tracking(actor.0) {
            member.aim
        } else {
            // Retail idle looking (bot_look): ahead while moving, otherwise
            // glancing around at friends and nearby spots, at saccade speed.
            let eye = member.position + Vec3::Y * EYE;
            let others: Vec<_> = friends.iter().copied().filter(|(k, _)| *k != actor.0).collect();
            // "Supporting friend in combat": while moving to assist a
            // teammate it can see, look where that teammate looks.
            let support = assist.target(actor.0).and_then(|key| {
                let (friend_eye, forward) = if key == crate::bot_look::PLAYER {
                    (
                        body.feet + Vec3::Y * EYE,
                        Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z,
                    )
                } else {
                    let f = squad_view.get(key)?;
                    (f.0 + Vec3::Y * EYE, f.1)
                };
                let t = c.sight_trace(eye, friend_eye);
                (!t.start_solid && t.fraction >= 1.)
                    .then(|| friend_eye + forward * crate::bot_retreat::SUPPORT_LOOK)
            });
            // "Clearing an area" comes before looking around.
            let clear = support.or_else(|| {
                navigation
                    .as_deref()
                    .and_then(|nav| clearing.target(actor.0, nav, now, dt, eye, member.aim))
            });
            let wanted = match clear {
                Some(point) => (point - eye).normalize_or(member.aim),
                None => bot_look.wanted(actor.0, now, member.position, eye, member.aim, &others, |a, b| {
                    let t = c.sight_trace(a, b);
                    !t.start_solid && t.fraction >= 1.
                }),
            };
            crate::bot_look::turn(member.aim, wanted, crate::bot_look::SACCADE_SPEED, dt)
        };
        if toward.length_squared() > 0.1 {
            // Keep visual perception aligned with the direction actually
            // rendered when idle, rather than the last combat aim.
            member.aim = toward.normalize();
            transform.rotation = Quat::from_rotation_y((-toward.z).atan2(toward.x));
        }
        // Native collapse/incap sequences already put the survivor on the
        // floor; rotating the root as well tips that pose onto its side.
    }
}
/// Survivor bots simulate at the 30 Hz fixed tick; like retail's
/// interpolated players, draw them between their last two tick positions
/// so walking (stairs especially) does not step at the tick rate.
fn smooth_companions(
    squad: Res<Squad>,
    fixed: Res<Time<Fixed>>,
    mut actors: Query<(&CompanionActor, &mut Transform), Without<Enemy>>,
    mut track: Local<Vec<(Vec3, Vec3)>>,
    mut last_tick: Local<f64>,
) {
    let alpha = fixed.overstep_fraction();
    // A new fixed step that left a bot where it was must also settle the
    // interpolation; otherwise the drawn bot keeps sliding between its last
    // two positions every frame, and its walk cycle plays on the spot.
    let tick = fixed.elapsed_secs_f64();
    let new_tick = tick != *last_tick;
    *last_tick = tick;
    for (actor, mut transform) in &mut actors {
        let Some(member) = squad.0.get(actor.0) else { continue };
        if track.len() <= actor.0 {
            track.resize(actor.0 + 1, (member.position, member.position));
        }
        let (previous, current) = &mut track[actor.0];
        if member.state.dead || member.state.pinned {
            *previous = member.position;
            *current = member.position;
            continue;
        }
        if member.position != *current {
            // Teleports (rescues, recovery) jump straight to the new spot.
            *previous = if member.position.distance(*current) > 1. { member.position } else { *current };
            *current = member.position;
        } else if new_tick {
            *previous = *current;
        }
        let drawn = previous.lerp(*current, alpha);
        if transform.translation != drawn {
            transform.translation = drawn;
        }
    }
}
/// A shambling common blocked while walking along `wish`: the horizontal
/// normal of a world wall it can lean on (retail StaggerAround::OnContact
/// needs normal z under 0.1; LeanOnWall::OnStart then needs the wall to
/// support it at lean height, else "No support for lean").
fn lean_wall(c: &Collision, feet: Vec3, wish: Vec3) -> Option<Vec3> {
    let ahead = (wish * Vec3::new(1., 0., 1.)).normalize_or_zero();
    let chest = feet + Vec3::Y * (40. * UNIT);
    let hit = c.trace(chest, chest + ahead * (24. * UNIT), Vec3::ZERO);
    if hit.fraction >= 1. || hit.normal.y.abs() >= mind::WALL_NORMAL_MAX_UP {
        return None;
    }
    let normal = (hit.normal * Vec3::new(1., 0., 1.)).normalize_or_zero();
    let shoulder = feet + Vec3::Y * (0.75 * crate::collision::HEIGHT);
    let support = c.trace(shoulder, shoulder - normal * (32. * UNIT), Vec3::ZERO);
    (support.fraction < 1.).then_some(normal)
}
/// Most a chasing common's facing turns per second (degrees).
const COMMON_TURN_RATE: f32 = 250.;
/// A common in reach keeps its ACT_TERROR_ATTACK_CONTINUOUSLY clip this long
/// after its target last was (refreshed every tick while in reach).
const MELEE_HOLD: f32 = 0.25;
/// Longest a common in reach goes without a blow when its clip reports none
/// (the longest gap between Melee_01's AE_ATTACK_HIT events is 1.36 s).
const MELEE_FALLBACK: f32 = 2.;
/// No two AE_ATTACK_HIT events in Melee_01..04 are under 0.6 s apart. The clip
/// clock follows frame time while enemy_ai follows fixed time, which falls
/// behind on slow frames: events closer than this in game time are dropped.
const MELEE_MIN_GAP: f32 = 0.5;
/// Retail commons swing on their melee clip's AE_ATTACK_HIT events: the
/// looping Melee_01..04 clips (7 to 21 s long) were recorded playing straight
/// through on c1m1_hotel, never restarting per blow. The animation system
/// counts the events it crosses; enemy_ai lands one blow per event.
#[derive(Resource, Default)]
pub(crate) struct MeleeHits {
    pub hits: HashMap<Entity, u32>,
    since: HashMap<Entity, f32>,
}
fn enemy_ai(
    projectiles: Option<Res<crate::projectiles::Projectiles>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    time: Res<Time<Fixed>>,
    c: Res<Collision>,
    body: Single<&Body>,
    mut s: ResMut<Match>,
    // The Witch has her own brain (special_infected/witch.rs).
    mut enemies: Query<
        (
            Entity,
            &mut Transform,
            &mut Enemy,
            Option<&crate::test_lab::ImpactTarget>,
            Has<crate::special_infected::ai::AiSpecial>,
            Option<&mut mind::Mind>,
        ),
        // Online guests' puppets of the host's infected run no AI.
        (Without<crate::special_infected::witch::Witch>, Without<crate::net_infected::Puppet>),
    >,
    mut commands: Commands,
    mut squad: ResMut<Squad>,
    mut cinematic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut navigation: Option<ResMut<crate::l4d2r_navigation::Navigation>>,
    mut crowd: Local<crate::infected_movement::Crowd>,
    clown_signals: Option<Res<crate::uncommon_behaviour::ClownSignals>>,
    (director, mut audio, mut noise, mut melee, library): (
        Option<Res<crate::source_director::Director>>,
        Option<ResMut<crate::game_audio::Queue>>,
        Local<SurvivorNoise>,
        Option<ResMut<MeleeHits>>,
        Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    ),
) {
    let _timing = crate::performance::scope("game::enemy_ai");
    let c = c.for_common();
    if s.paused || cinematic.as_ref().is_some_and(|c| c.active) {
        return;
    }
    let dt = time.delta_secs();
    crowd.rebuild(
        enemies
            .iter()
            .filter(|(_, _, e, _, _, _)| e.health > 0.)
            .map(|(id, t, _, _, _, _)| (id.to_bits(), t.translation)),
        dt,
    );
    // What unalerted commons can perceive of each survivor this tick.
    noise.clock += dt;
    let clock = noise.clock;
    let mut heard = vec![];
    if !s.infected && !s.dead {
        heard.push((body.feet, Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z, s.shot_serial));
    } else {
        heard.push((Vec3::splat(f32::INFINITY), Vec3::ZERO, 0));
    }
    for m in &squad.0 {
        let point = if m.state.dead { Vec3::splat(f32::INFINITY) } else { m.position };
        heard.push((point, m.aim * Vec3::new(1., 0., 1.), m.state.shot_serial));
    }
    noise.running.resize(heard.len(), 0.);
    noise.shots.resize(heard.len(), 0);
    noise.positions.resize(heard.len(), Vec3::ZERO);
    let mut threats = vec![];
    for (i, (point, facing, shots)) in heard.into_iter().enumerate() {
        let moved = (point - noise.positions[i]) * Vec3::new(1., 0., 1.);
        noise.positions[i] = point;
        // Faster than a walk (85) counts as running.
        let running = point.is_finite() && moved.length() / dt.max(1e-3) > 150. * UNIT;
        noise.running[i] = if running { noise.running[i] + dt } else { 0. };
        let fired = shots != noise.shots[i];
        noise.shots[i] = shots;
        if point.is_finite() {
            threats.push(mind::Threat {
                position: point,
                forward: facing.normalize_or_zero(),
                visible: false,
                running: noise.running[i],
                fired,
            });
        }
    }
    let mob = director.as_ref().is_some_and(|d| d.mob_active());
    if let Some(nav) = navigation.as_mut() {
        nav.begin_infected_tick(&crowd.alive);
    }
    // `L4D2R_COMMON_LOG=1`: one line per common per second from the brain
    // (the animation log skips commons nobody can see).
    let log_second = std::env::var_os("L4D2R_COMMON_LOG").is_some()
        && (s.elapsed * 1.).floor() != ((s.elapsed - dt) * 1.).floor();
    for (entity, mut transform, mut enemy, impact_target, special, mut mind) in &mut enemies {
        // AI specials stay in the crowd (routes, separation) but
        // special_infected::ai drives them.
        if special {
            continue;
        }
        let swings = melee.as_mut().and_then(|m| m.hits.remove(&entity)).unwrap_or(0);
        if log_second && enemy.health > 0. {
            let p = transform.translation / UNIT;
            let to_player = (body.feet - transform.translation).length() / UNIT;
            println!(
                "L4D2RBRAIN t={:.0} ent={} pos={:.0},{:.0},{:.0} player_dist={:.0} spd={:.0} reaction={} attack={:.2} mind={} clip={}",
                s.elapsed, entity.index(), p.x, p.y, p.z, to_player, enemy.movement_speed / UNIT, enemy.reaction, enemy.attack,
                mind.as_ref().map_or("none", |m| if m.attacking() { "attacking" } else { "wander" }),
                mind.as_ref().map_or("none", |m| m.activity)
            );
        }
        if enemy.health <= 0. {
            let start = transform.translation + Vec3::Y * 0.1;
            enemy.vertical -= 800. * UNIT * dt;
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
        enemy.burning = (enemy.burning - dt).max(0.);
        if matches!(enemy.reaction, 1..=3) && enemy.attack > 0. {
            crowd.cancel_climb(entity.to_bits());
        } else if let Some((position, reaction, remaining)) =
            crowd.advance_climb(entity.to_bits(), &c, transform.translation, dt)
        {
            transform.translation = position;
            enemy.reaction = reaction;
            enemy.attack = remaining;
            enemy.vertical = 0.;
            enemy.movement_speed = 0.;
            continue;
        } else if let Some((position, code)) = crate::infected_movement::advance_ladder(
            &mut crowd, entity.to_bits(), &c, transform.translation, dt,
        ) {
            // NAV ladder traversal (crate::ai_ladder).
            transform.translation = position;
            if let Some(pose) = crowd.ladders.pose(entity.to_bits()) {
                transform.rotation = pose.rotation();
            }
            enemy.reaction = code;
            enemy.attack = if code != 0 { 0.1 } else { 0. };
            enemy.vertical = 0.;
            enemy.movement_speed = 0.;
            continue;
        }
        if matches!(enemy.reaction, crate::infected_movement::LADDER_UP | crate::infected_movement::LADDER_DOWN) {
            enemy.reaction = 0;
            enemy.attack = 0.;
        }
        enemy.movement_speed = 0.;
        enemy.attack = (enemy.attack - dt).max(0.);
        if enemy.reaction != 0 && enemy.attack > 0. {
            // A hit reaction pauses intent, not gravity. In particular a shove
            // can interrupt a climb after the feet have left their support.
            let (point, vertical) = crate::infected_movement::walk(
                &c, transform.translation, Vec3::ZERO, enemy.vertical, dt,
            );
            transform.translation = point;
            enemy.vertical = vertical;
            continue;
        }
        enemy.reaction = 0;
        if impact_target.is_some() {
            continue;
        }
        let bot = squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.state.dead)
            .min_by(|a, b| {
                a.1.position
                    .distance_squared(transform.translation)
                    .total_cmp(&b.1.position.distance_squared(transform.translation))
            })
            .filter(|(_, m)| {
                s.dead
                    || s.infected
                    || m.position.distance_squared(transform.translation)
                        < body.feet.distance_squared(transform.translation)
            })
            .map(|(i, m)| (i, m.position));
        // Boomer-vomited (IT) survivors within z_notice_it_range draw commons.
        // A vomited survivor player draws commons the same way.
        let player_it = !s.infected
            && !s.dead
            && s.vomited > 0.
            && body.feet.distance(transform.translation)
                <= crate::special_infected::boomer::NOTICE_IT_RANGE * UNIT;
        let bot = if player_it {
            None
        } else {
            crate::special_infected::boomer::it_target(&squad, transform.translation).or(bot)
        };
        let lure = projectiles.as_ref().and_then(|p| {
            p.attraction(
                transform.translation,
                entity,
                matches!(
                    crate::infected_population::Kind::model(&enemy.model),
                    crate::infected_population::Kind::Roadcrew
                        | crate::infected_population::Kind::Jimmy
                ),
                crate::infected_population::Kind::model(&enemy.model)
                    == crate::infected_population::Kind::Jimmy,
            )
        });
        let kind = crate::infected_population::Kind::model(&enemy.model);
        let clown_target = if kind == crate::infected_population::Kind::Common {
            clown_signals
                .as_ref()
                .and_then(|signals| signals.target(transform.translation))
        } else {
            None
        };
        if let Some(mind) = mind.as_mut().filter(|m| !m.attacking()) {
            let seen = (enemy.health, enemy.reaction_serial);
            let injured = mind.last.is_some_and(|(health, serial)| {
                seen.0 < health || seen.1 != serial
            }) || enemy.burning > 0.;
            mind.last = Some(seen);
            // Lures, a clown's squeak and bile reach unalerted commons too.
            if lure.is_some() || clown_target.is_some() || player_it
                || crate::special_infected::boomer::it_target(&squad, transform.translation).is_some()
            {
                mind.provoke();
            }
            let eye = transform.translation + Vec3::Y * 1.5;
            let sight = mind.sight_range() * UNIT;
            let mut sensed = threats.clone();
            for t in &mut sensed {
                t.visible = t.position.distance(transform.translation) <= sight
                    && c.trace(eye, t.position + Vec3::Y * 1.5, Vec3::ZERO).fraction >= 1.;
            }
            let senses = mind::Senses {
                clock,
                position: transform.translation,
                threats: &sensed,
                mob,
                injured,
            };
            let mut voice = None;
            let intent = mind.think(&senses, dt, &mut voice);
            if let (Some(voice), Some(audio)) = (voice, audio.as_mut()) {
                audio.play(voice, Some(transform.translation));
            }
            let wish = match intent {
                mind::Intent::Attack => None,
                mind::Intent::Idle { yaw, .. } => {
                    transform.rotation = Quat::from_rotation_y(yaw);
                    Some(Vec3::ZERO)
                }
                mind::Intent::Clip { label, start } => {
                    // Retail moves a wandering common by its clip's extracted
                    // root motion (walks, turn-arounds), at playback rate 1.
                    let motion = library.as_ref().and_then(|l| {
                        l.clip_motion(&enemy.model, label, (clock - dt - start).max(0.), (clock - start).max(0.))
                    });
                    let (step, turn) = motion.unwrap_or((Vec2::ZERO, 0.));
                    mind.yaw += turn.to_radians();
                    let yaw = mind.yaw;
                    transform.rotation = Quat::from_rotation_y(yaw);
                    let left = mind::forward(yaw + std::f32::consts::FRAC_PI_2);
                    let velocity = (mind::forward(yaw) * step.x + left * step.y) * UNIT / dt.max(1e-4);
                    if velocity.length_squared() > 1e-8 {
                        let ahead = transform.translation + velocity.normalize() * 0.6;
                        let floor = c.trace(ahead + Vec3::Y * 0.5, ahead - Vec3::Y * 1.2, Vec3::ZERO);
                        if floor.fraction >= 1. {
                            // Never shamble off a drop: an obstacle, not a wall.
                            mind.bump(clock, None);
                            Some(Vec3::ZERO)
                        } else if crowd.touching_ahead(entity.to_bits(), transform.translation, velocity) {
                            // Walked into another actor: StaggerAround::OnContact
                            // with a non-world entity, "Bumped into something".
                            mind.bump(clock, None);
                            Some(Vec3::ZERO)
                        } else {
                            Some(velocity)
                        }
                    } else {
                        Some(Vec3::ZERO)
                    }
                }
                mind::Intent::Walk { yaw } => {
                    transform.rotation = Quat::from_rotation_y(yaw);
                    let ahead = transform.translation + mind::forward(yaw) * 0.6;
                    let floor = c.trace(ahead + Vec3::Y * 0.5, ahead - Vec3::Y * 1.2, Vec3::ZERO);
                    if floor.fraction >= 1. {
                        // Never shamble off a drop: an obstacle, not a wall.
                        mind.bump(clock, None);
                        Some(Vec3::ZERO)
                    } else {
                        Some(mind::forward(yaw) * mind::SHAMBLE_SPEED * UNIT)
                    }
                }
            };
            if let Some(wish) = wish {
                let before = transform.translation;
                let (point, vertical) = crate::infected_movement::walk(
                    &c, before, wish, enemy.vertical, dt,
                );
                transform.translation = point;
                enemy.vertical = vertical;
                let moved = (point - before) * Vec3::new(1., 0., 1.);
                enemy.movement_speed = moved.length() / dt.max(0.001);
                if wish != Vec3::ZERO && moved.length() < wish.length() * dt * 0.3 {
                    mind.bump(clock, lean_wall(&c, before, wish));
                }
                continue;
            }
        }
        let Some(target) = lure
            .map(|a| a.0)
            .or(clown_target)
            .or_else(|| bot.map(|(_, p)| p))
            .or_else(|| (!s.infected && !s.dead).then_some(body.feet))
        else {
            enemy.attack = 0.;
            crowd.cancel_climb(entity.to_bits());
            continue;
        };
        let target = if kind == crate::infected_population::Kind::Fallen && enemy.health < 1000. {
            transform.translation + (transform.translation - target).normalize_or_zero() * 12.7
        } else {
            target
        };
        let delta = (target - transform.translation) * Vec3::new(1., 0., 1.);
        let close = delta.length() <= 1.2 && (target.y - transform.translation.y).abs() < 1.5;
        let visible = close
            && c.trace(
                transform.translation + Vec3::Y,
                target + Vec3::Y,
                Vec3::ZERO,
            )
            .fraction
                >= 1.;
        let chase = !close || !visible;
        let toward = if chase {
            navigation
                .as_mut()
                .map_or(target - transform.translation, |nav| {
                    nav.toward_infected(entity.to_bits(), transform.translation, target, dt)
                })
        } else {
            Vec3::ZERO
        };
        if chase {
            if let Some(step) = navigation
                .as_mut()
                .and_then(|nav| nav.ladder_infected(entity.to_bits(), transform.translation, target))
            {
                crowd.ladders.begin(entity.to_bits(), &c, transform.translation, step);
                continue;
            }
        }
        let before = transform.translation;
        let can_pound = enemy.attack <= 0.;
        let (point, vertical, outcome) = crate::infected_movement::chase(
            &c,
            &mut crowd,
            entity.to_bits(),
            before,
            if chase { toward } else { Vec3::ZERO },
            enemy.vertical,
            dt,
            |from, direction| {
                can_pound
                    && cinematic
                        .as_mut()
                        .is_some_and(|logic| logic.pound_obstacle(from, direction, &c))
            },
        );
        transform.translation = point;
        enemy.vertical = vertical;
        if outcome == crate::infected_movement::Chase::Stuck {
            if let Some(nav) = navigation.as_mut() {
                nav.reroute_infected(entity.to_bits(), transform.translation, target);
            }
        }
        if outcome == crate::infected_movement::Chase::Pounded {
            enemy.reaction = 3;
            enemy.reaction_serial = enemy.reaction_serial.wrapping_add(1);
            enemy.attack = 0.8;
        }
        let moved = (transform.translation - before) * Vec3::new(1., 0., 1.);
        // In reach the common is not walking: crowd separation still nudges
        // it, but that is no locomotion speed. Counting it sped the melee
        // clip (Melee_01 moves 0.25 u/s) up to the 12x playback cap, so a
        // surrounding crowd flailed; retail's attackers play it at rate 1.
        enemy.movement_speed = if chase { moved.length() / dt.max(0.001) } else { 0. };
        // Retail commons turn at most 250 degrees a second (the yaw of every
        // recorded c1m1_hotel chaser stepped 25 degrees per 0.1 s sample
        // while turning), toward where they move or, in reach, the target.
        let face = if moved.length_squared() > 1e-6 {
            Some(moved)
        } else if close {
            Some(delta)
        } else {
            None
        };
        if let Some(face) = face.filter(|f| f.length_squared() > 1e-6) {
            let current = transform.rotation.to_euler(EulerRot::YXZ).0;
            let yaw = mind::turn_toward(current, (-face.z).atan2(face.x), COMMON_TURN_RATE.to_radians() * dt);
            transform.rotation = Quat::from_rotation_y(yaw);
        }
        if close && lure.is_some() {
            enemy.attack = 0.2;
        } else if visible && lure.is_none() && clown_target.is_none() {
            let blow = if let Some(m) = melee.as_mut() {
                enemy.attack = enemy.attack.max(MELEE_HOLD);
                let since = m.since.entry(entity).or_insert(0.);
                *since += dt;
                let blow = (swings > 0 && *since >= MELEE_MIN_GAP) || *since >= MELEE_FALLBACK;
                if blow {
                    *since = 0.;
                }
                blow
            } else {
                enemy.attack <= 0.
            };
            let origin = transform.translation + Vec3::Y;
            let end = target + Vec3::Y;
            if blow && c.trace(origin, end, Vec3::ZERO).fraction >= 1. {
                if let Some((index, _)) = bot {
                    squad.0[index]
                        .state
                        .damage(session.as_ref().map_or(2., |s| s.difficulty.claw()));
                } else if !s.infected && !s.dead {
                    s.damage(session.as_ref().map_or(2., |s| s.difficulty.claw()));
                    if kind == crate::infected_population::Kind::Mud {
                        s.mud_serial = s.mud_serial.wrapping_add(1);
                    }
                }
                enemy.attack_serial = enemy.attack_serial.wrapping_add(1);
                if melee.is_none() {
                    enemy.attack = 0.8;
                }
            }
        } else if let Some(m) = melee.as_mut() {
            m.since.remove(&entity);
        }
    }
    if let Some(m) = melee.as_mut() {
        m.since.retain(|e, _| crowd.alive.contains(&e.to_bits()));
        m.hits.retain(|e, _| crowd.alive.contains(&e.to_bits()));
    }
}
fn verify_native(c: &Collision, spawn: Vec3, pistol: &Weapon) -> io::Result<()> {
    let mut app = App::new();
    let mut state = Match::new(pistol);
    state.paused = false;
    app.insert_resource(Time::<Fixed>::default())
        .insert_resource(ButtonInput::<KeyCode>::default())
        .insert_resource(Input::default())
        .insert_resource(c.clone())
        .insert_resource(state)
        .insert_resource(Rules {
            weapons: HashMap::from([(pistol.class.clone(), pistol.clone())]),
            pistol: pistol.clone(),
        })
        .add_systems(FixedUpdate, move_player);
    let player = app
        .world_mut()
        .spawn(Body {
            feet: spawn,
            velocity: Vec3::ZERO,
            grounded: false,
            crouched: false,
            stance: default(),
            previous_feet: spawn,
            previous_eye: EYE,
            contact_normal: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
        })
        .id();
    for _ in 0..180 {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
        app.world_mut().run_schedule(FixedUpdate);
    }
    let position = app.world().get::<Body>(player).unwrap().feet;
    if !c.grounded(position, HEIGHT) || !c.clear_hull(position, HEIGHT) {
        let center = position + Vec3::Y * HEIGHT * 0.5;
        eprintln!(
            "Spawn ground trace: {:?}; standing clear={}",
            c.trace(
                center,
                center - Vec3::Y * (2. * UNIT),
                Vec3::new(
                    crate::collision::RADIUS,
                    HEIGHT * 0.5,
                    crate::collision::RADIUS
                )
            ),
            c.clear_hull(position, HEIGHT)
        );
        return Err(binary::invalid(format!(
            "Survivor spawn did not settle on clear ground: {position:?}"
        )));
    }
    for key in [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD] {
        {
            let mut body = app.world_mut().get_mut::<Body>(player).unwrap();
            body.feet = position;
            body.velocity = Vec3::ZERO;
            body.grounded = true;
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        for _ in 0..18 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            app.world_mut().run_schedule(FixedUpdate);
            let feet = app.world().get::<Body>(player).unwrap().feet;
            if feet.y < position.y - 0.2 || !c.clear_hull(feet, HEIGHT) {
                return Err(binary::invalid(format!(
                    "Hotel spawn walking failed for {key:?}: {feet:?}"
                )));
            }
        }
    }
    assert_eq!(pistol.clip, 15);
    assert_eq!(pistol.damage, 36.);
    let mut state = Match::new(pistol);
    state.pistol.ammo = 0;
    state.reload_complete(pistol);
    assert_eq!(state.pistol.ammo, pistol.clip);
    println!(
        "Native gameplay verification passed: spawn settled at {position:?}, standing and four-direction walking clear, pistol script and reload validated."
    );
    Ok(())
}
/// `--capture-finale` scaffolding only: keeps the idle capture player alive
/// and kills each Tank after 15 s so the Director's finale stages can be
/// observed end to end. Normal play never runs this.
fn finale_capture_assist(
    time: Res<Time>,
    mut s: ResMut<Match>,
    mut tanks: Query<(Entity, &mut Enemy, &crate::special_infected::ai::AiSpecial)>,
    mut ages: Local<HashMap<Entity, f32>>,
) {
    if !std::env::args().any(|a| a == "--capture-finale") {
        return;
    }
    if !s.dead {
        s.health = s.health.max(90.);
        s.incap = false;
    }
    for (entity, mut enemy, ai) in &mut tanks {
        if ai.class != crate::special_infected::SpecialClass::Tank || enemy.health <= 0. {
            continue;
        }
        let age = ages.entry(entity).or_insert(0.);
        *age += time.delta_secs();
        if *age > 15. {
            println!("Capture finale: Tank killed by fixture");
            enemy.health = 0.;
        }
    }
}
fn capture_telemetry(
    mut telemetry: ResMut<CaptureTelemetry>,
    collision: Res<Collision>,
    mut s: ResMut<Match>,
    campaign: Res<Campaign>,
    squad: Res<Squad>,
    enemies: Query<&Enemy>,
    mut frontend: ResMut<game_menu::Frontend>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    warmup: Option<Res<crate::render_warmup::Warmup>>,
    mut resize_fixture: (
        Query<&bevy::camera::RenderTarget, With<viewer::GameCamera>>,
        ResMut<Assets<Image>>,
        Query<&mut Window>,
    ),
) {
    if warmup.is_some() {
        return;
    }
    telemetry.0 += 1;
    // `--capture-finale`: force-start the map's trigger_finale (Director stage log).
    if telemetry.0 == 60 && std::env::args().any(|a| a == "--capture-finale") {
        if let Some(id) = logic.world.ents.iter().position(|e| e.alive && matches!(e.kind, l4d2r_game_core::entities::Kind::Finale { .. })) {
            println!("Capture finale: ForceFinaleStart on {}", logic.world.ents[id].me.targetname);
            logic.world.input(id, "ForceFinaleStart", "", None, None);
        } else {
            for e in logic.world.ents.iter().filter(|e| e.me.classname == "trigger_finale") {
                println!("Capture finale: trigger_finale {} alive={} kind={:?}", e.me.targetname, e.alive, std::mem::discriminant(&e.kind));
            }
        }
    }
    if std::env::args().any(|a|a=="--capture-fuel-pour") {
        if telemetry.0==60 {
            s.carried=Some("weapon_gascan".into()); s.slot=6;
            logic.world.queue_input("relay_explain_finale","Trigger","",0.,None);
            logic.world.queue_input("trigger_finale","UseFinale","",0.,None);
        }
        if (90..240).contains(&telemetry.0) { mouse.press(MouseButton::Left); }
        else { mouse.release(MouseButton::Left); }
        if telemetry.0==300 { println!("Fuel capture: progress={:?}, carried={:?}",logic.world.scavenge_progress(),s.carried); }
    }
    // Whitaker fixture: carry cola and hold left click at the receiving hatch.
    if std::env::args().any(|a| a == "--capture-cola-deliver") {
        if telemetry.0 == 60 {
            s.carried = Some("weapon_cola_bottles".into());
            s.slot = 6;
        }
        if (90..240).contains(&telemetry.0) {
            mouse.press(MouseButton::Left);
        } else {
            mouse.release(MouseButton::Left);
        }
        if telemetry.0 == 120 || telemetry.0 == 300 {
            println!("Cola capture: frame={} carried={:?} hint={:?}", telemetry.0, s.carried, s.hint);
        }
    }

    if std::env::args().any(|a| a == "--capture-glow-resize") {
        let size = match telemetry.0 {
            75 => Some((1024, 576)),
            105 => Some((1, 1)),
            135 => Some((1280, 720)),
            _ => None,
        };
        if let Some((width, height)) = size {
            for target in &resize_fixture.0 {
                if let bevy::camera::RenderTarget::Image(target) = target {
                    if let Some(mut image) = resize_fixture.1.get_mut(&target.handle) {
                        image.resize(bevy::render::render_resource::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        });
                    }
                }
            }
            for mut window in &mut resize_fixture.2 {
                window.focused = telemetry.0 != 105;
            }
            if telemetry.0 == 135 {
                s.paused = false;
                frontend.page = game_menu::Page::Game;
            }
            println!("Glow resize/focus fixture: {width}x{height}");
        }
    }
    if std::env::args().any(|a| a == "--capture-movement-audio") {
        if telemetry.0 == 60 {
            keys.press(KeyCode::KeyW);
        }
        if telemetry.0 == 100 {
            keys.press(KeyCode::ControlLeft);
        }
        if telemetry.0 == 150 {
            keys.release(KeyCode::KeyW);
            keys.release(KeyCode::ControlLeft);
        }
        if telemetry.0 == 180 {
            keys.press(KeyCode::Space);
        }
        if telemetry.0 == 181 {
            keys.release(KeyCode::Space);
        }
        if telemetry.0 == 70 {
            if let Some(id) = logic
                .world
                .ents
                .iter()
                .position(|e| e.me.kv("hammerid") == Some("330642"))
            {
                logic.world.input(id, "Open", "", None, None);
            }
        }
        if telemetry.0 == 130 {
            if let Some(id) = logic
                .world
                .ents
                .iter()
                .position(|e| e.me.kv("hammerid") == Some("330642"))
            {
                logic.world.input(id, "Close", "", None, None);
            }
        }
    }
    if telemetry.0 == 60 {
        let args: Vec<_> = std::env::args().collect();
        if let Some(name) = args
            .iter()
            .position(|a| a == "--capture-fire-relay")
            .and_then(|i| args.get(i + 1))
        {
            let ids = logic.world.find(name);
            for id in ids {
                logic.world.input(id, "Trigger", "", None, None);
            }
            println!("Capture triggered native fire relay: {name}");
        }
        if let Some(hammer) = args
            .iter()
            .position(|a| a == "--capture-break-entity")
            .and_then(|i| args.get(i + 1))
        {
            if let Some(id) = logic
                .world
                .ents
                .iter()
                .position(|e| e.me.kv("hammerid") == Some(hammer.as_str()))
            {
                let broken = if let Some(value)=args.windows(2).find(|p|p[0]=="--capture-surface-shatter") {
                    logic.world.input(id,"Shatter",&value[1],None,None); false
                } else {logic.world.damage_entity(
                    id,
                    args.windows(2).find(|p| p[0] == "--capture-break-damage")
                        .and_then(|p| p[1].parse::<f32>().ok()).unwrap_or(10000.),
                    l4d2r_game_core::entities::DamageKind::Bullet,
                )};
                crate::source_breakables::sync_doors(&mut logic);
                collision.sync_entities(&logic);
                println!("Capture native breakable {hammer}: broken={broken}");
            }
        }
    }
    if telemetry.0 == 60 && std::env::args().any(|a| a == "--capture-return-menu") {
        frontend.page = if std::env::args().any(|a| a == "--capture-return-campaign") {
            game_menu::Page::Campaign
        } else {
            game_menu::Page::Home
        };
        frontend.started = false;
        s.paused = true;
        println!(
            "Capture transition: gameplay -> {:?}",
            if frontend.page == game_menu::Page::Home {
                "Home"
            } else {
                "Campaign"
            }
        );
    }
    if telemetry.0 == 170 {
        println!("Authored break debris: {} active physical pieces", logic.break_debris.active.len());
        for (id,surface) in logic.break_surfaces.iter().filter(|(_,s)|s.initialized) {
            println!("Fractured surface {}: {}x{} grid, {} solid panes",logic.world.ents[*id].me.kv("hammerid").unwrap_or("?"),surface.wide,surface.high,surface.shapes.len());
        }
        println!(
            "Gameplay capture: elapsed={:.2}s health={:.0} spawned={} alive={} corpses={} squad_kills={}",
            s.elapsed,
            s.health,
            campaign.total_spawned,
            enemies.iter().filter(|e| e.health > 0.).count(),
            enemies.iter().filter(|e| e.health <= 0.).count(),
            squad.0.iter().map(|m| m.state.kills).sum::<u32>()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c4m2_bare_landmark_spawn_moves_into_the_player_start_area() {
        // Retail's direct-load spawn on c4m2 is (3754 -1598), in the start
        // saferoom; landmark_c4m1 (3776 -1728) sits inside a wall.
        let map = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/left4dead2/maps/c4m2_sugarmill_a.bsp");
        if !map.exists() {
            return;
        }
        let nav = l4d2_formats::nav::parse_nav(&std::fs::read(map.with_extension("nav")).unwrap()).unwrap();
        let starts: Vec<_> = nav.areas.iter().filter(|a| a.spawn_attributes & 0x80 != 0).map(|a| (a.nw, a.se)).collect();
        let found = player_start_near(map, [3776., -1728., 296.5]);
        println!("c4m2 PLAYER_START areas {} (checkpoint fallback): {:?}; nearest centre {found:?}", starts.len(), &starts[..starts.len().min(8)]);
        let p = found.expect("a start or checkpoint area");
        assert!((p[0] - 3776.).abs() > 1. || (p[1] + 1728.).abs() > 1., "{p:?}");
        // In the start saferoom, near retail's spawn.
        assert!(((p[0] - 3754.).powi(2) + (p[1] + 1598.).powi(2)).sqrt() < 250., "{p:?}");
    }
    #[test]
    fn menu_map_replacement_registers_both_player_ladder_types() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = Path::new("maps/rewrite_lab.bsp");
        let mut app = App::new();
        let weapon = pistol();
        let mut state = Match::new(&weapon);
        state.paused = false;
        app.insert_resource(Time::<Fixed>::default())
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(Input::default())
            .insert_resource(state)
            .insert_resource(Rules {
                weapons: HashMap::from([(weapon.class.clone(), weapon.clone())]),
                pistol: weapon,
            })
            .add_systems(FixedUpdate, move_player);
        let player = app.world_mut().spawn(Body {
            feet: Vec3::ZERO, velocity: Vec3::ZERO, grounded: false,
            crouched: false, stance: default(), previous_feet: Vec3::ZERO,
            previous_eye: EYE, contact_normal: Vec3::ZERO,
            yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.,
        }).id();
        // Reproduce the menu's ladder-free bootstrap before loading gameplay.
        let mut bootstrap = crate::l4d2r_cinematic::MapLogic::load(root, path, true).unwrap();
        for e in &mut bootstrap.world.ents {
            if e.me.classname == "func_simpleladder" { e.me.classname = "info_null".into(); }
        }
        install_map_logic(app.world_mut(), bootstrap);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
        // Reuse this scheduled system and its Local attachment across loads.
        // Shared, infected-only, survivor-only, then wrong-team rejection.
        for (infected, phase, y, allowed) in [
            (false, 0, 282., true), (true, 5, 482., true),
            (true, 0, 482., true), (false, 0, 682., true),
            (false, 0, 482., false), (true, 0, 682., false),
        ] {
            let logic = crate::l4d2r_cinematic::MapLogic::load(root, path, true).unwrap();
            let mut collision = Collision::load(path).unwrap();
            collision.enable_l4d2r(root, &logic).unwrap();
            app.insert_resource(collision);
            install_map_logic(app.world_mut(), logic);
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.infected = infected;
                state.special_phase = phase;
            }
            let start = Vec3::new(3579.95, 24., -y) * UNIT;
            {
                let mut body = app.world_mut().get_mut::<Body>(player).unwrap();
                body.feet = start;
                body.velocity = Vec3::ZERO;
                body.stance = default();
                body.crouched = false;
            }
            for _ in 0..12 {
                app.world_mut().resource_mut::<Time<Fixed>>()
                    .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut().run_schedule(FixedUpdate);
            }
            let gain = (app.world().get::<Body>(player).unwrap().feet.y - start.y) / UNIT;
            if allowed {
                let expected = if phase == 5 { 80. } else { 40. };
                assert!((gain - expected).abs() < 0.02,
                    "infected={infected} phase={phase} ladder_y={y}: gain={gain}");
            } else {
                assert!(gain < 1., "wrong-team ladder was climbable: {gain}");
            }
        }
    }
    #[test]
    fn survivor_primary_uses_its_damage_clip_and_reserve_reload() {
        let (mut app, enemy) = game_app();
        let mut rifle = pistol();
        rifle.class = "weapon_rifle".into();
        rifle.clip = 50;
        rifle.damage = 33.;
        rifle.cycle = 0.0875;
        rifle.reload_times = [2.2, 0.5, 0.5];
        rifle.fire_sound = "Rifle.Fire".into();
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(rifle.class.clone(), rifle.clone());
        let mut state = Match::new(&pistol());
        state.paused = false;
        state.slot = 1;
        state.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: rifle.class.clone(),
            ammo: 1,
            reserve: 60,
        });
        app.world_mut().resource_mut::<Squad>().0.push(Companion {
            state,
            position: Vec3::X * 0.7,
            spawn: Vec3::X * 0.7,
            model: "models/survivors/survivor_gambler.mdl".into(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_Z,
        });
        settled_bot_shot(&mut app, 0);
        assert!((app.world().get::<Enemy>(enemy).unwrap().health - 17.).abs() < 0.001);
        assert_eq!(
            app.world().resource::<Squad>().0[0]
                .state
                .primary
                .as_ref()
                .unwrap()
                .ammo,
            0
        );
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 1);
        app.world_mut().get_mut::<Enemy>(enemy).unwrap().health = 0.;
        for _ in 0..20 {
            tick(&mut app);
        }
        assert!(app.world().resource::<Squad>().0[0].state.reload > 0.);
        for _ in 0..140 {
            tick(&mut app);
        }
        let state = &app.world().resource::<Squad>().0[0].state;
        assert_eq!(state.primary.as_ref().unwrap().ammo, 50);
        assert_eq!(state.primary.as_ref().unwrap().reserve, 10);
    }
    #[test]
    fn survivor_can_shove_while_reloading_without_spending_a_round() {
        let (mut app, enemy) = game_app();
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = Vec3::new(0., 0., -0.8);
        let mut state = Match::new(&pistol());
        state.paused = false;
        state.reload = 1.;
        state.reload_total = 1.;
        app.world_mut().resource_mut::<Squad>().0.push(Companion {
            state,
            // Clear of the player (survivors are solid and keep their
            // distance), facing the infected within shove reach.
            position: Vec3::new(0.95, 0., -0.2),
            spawn: Vec3::new(0.95, 0., -0.2),
            model: "models/survivors/survivor_gambler.mdl".into(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::new(-0.95, 0., -0.6).normalize(),
        });
        tick(&mut app);
        let state = &app.world().resource::<Squad>().0[0].state;
        assert_eq!(state.shove_serial, 1);
        assert_eq!(state.pistol.ammo, 15);
        assert!(state.reload > 0.);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().reaction, 1);
    }
    #[test]
    fn installed_pistol_barrel_aligns_with_camera_forward() {
        let forward = held_transform([0., 0., 1.]).rotation * Vec3::Z;
        assert!(forward.dot(Vec3::NEG_Z) > 0.999);
        let other = Vec3::new(1., 0.2, 0.).normalize();
        assert!((held_transform(other.to_array()).rotation * other).dot(Vec3::NEG_Z) > 0.999);
    }
    fn pistol() -> Weapon {
        Weapon {
            penetration: Default::default(),
            pellets: 1,
            range_modifier: 1.,
            scatter: Vec2::ZERO,
            accuracy: Default::default(),
            muzzle_effect: String::new(),
            class: "weapon_pistol".into(),
            clip: 15,
            damage: 36.,
            cycle: 0.175,
            range: 2500. * UNIT,
            speed: 250. * UNIT,
            punch: 2.,
            model: String::new(),
            player_model: String::new(),
            view_model: String::new(),
            reload_times: [1.7, 0.5, 0.5],
            reload_empty_time: 0.,
            damage_flags: 2,
            decapitates: false,
            muzzle_forward: [0., 0., 1.],
            fire_sound: "Pistol.Fire".into(),
            melee_hit_sound: String::new(),
            melee_world_sound: String::new(),
            melee_attacks: vec![],
            icon: String::new(),
            icon_rect: Rect::default(),
        }
    }
    #[test]
    fn empty_guns_auto_reload_without_input_and_preserve_ammo() {
        use bevy::ecs::system::RunSystemOnce;
        for (class, clip, reserve) in [
            ("weapon_pistol", 15, 0),
            ("weapon_pistol_dual", 30, 0),
            ("weapon_pistol_magnum", 8, 0),
            ("weapon_rifle", 50, 87),
            ("weapon_smg", 50, 87),
            ("weapon_hunting_rifle", 15, 30),
            ("weapon_pumpshotgun", 8, 12),
        ] {
            let (mut app, _) = game_app();
            let mut weapon = pistol();
            weapon.class = class.into();
            weapon.clip = clip;
            app.world_mut()
                .resource_mut::<Rules>()
                .weapons
                .insert(class.into(), weapon.clone());
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.pistol.class = class.into();
                state.pistol.ammo = 0;
                state.pistol.reserve = reserve;
                if !weapon.infinite() {
                    state.primary = Some(state.pistol.clone());
                    state.slot = 1;
                }
            }
            app.world_mut().run_system_once(combat).unwrap();
            let mut state = app.world_mut().resource_mut::<Match>();
            assert!(state.reload > 0., "{class}");
            for _ in 0..400 {
                state.advance_reload(&weapon, 0.05);
            }
            assert_eq!(state.gun().ammo, clip, "{class}");
            if !weapon.infinite() {
                assert_eq!(state.gun().reserve, reserve - clip);
            }
        }
    }
    #[test]
    fn auto_reload_waits_for_actions_and_does_not_reload_items_or_empty_reserves() {
        use bevy::ecs::system::RunSystemOnce;
        for mode in 0..7 {
            let (mut app, _) = game_app();
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.pistol.ammo = 0;
                match mode {
                    0 => state.cooldown = 0.2,
                    1 => state.action = 0.2,
                    2 => state.shove = 0.2,
                    3 => state.throw_stage = 1,
                    4 => state.slot = 4,
                    5 => state.pistol.class = "weapon_rifle".into(),
                    _ => {
                        state.pistol.class = "weapon_melee_fireaxe".into();
                        state.pistol.reserve = 10;
                    }
                }
            }
            if mode >= 5 {
                let mut w = pistol();
                w.class = if mode == 5 {
                    "weapon_rifle"
                } else {
                    "weapon_melee_fireaxe"
                }
                .into();
                app.world_mut()
                    .resource_mut::<Rules>()
                    .weapons
                    .insert(w.class.clone(), w);
            }
            app.world_mut().run_system_once(combat).unwrap();
            assert_eq!(app.world().resource::<Match>().reload, 0., "mode {mode}");
        }
    }
    #[test]
    fn carryable_pickup_preserves_inventory_and_switching_drops_it() {
        use bevy::ecs::system::RunSystemOnce;
        for class in ["weapon_gascan", "weapon_cola_bottles"] {
            let (mut app, enemy) = game_app();
            app.world_mut().despawn(enemy);
            let original = app.world().resource::<Match>().pistol.ammo;
            let pickup = app
                .world_mut()
                .spawn((
                    Pickup {
                        class: class.into(),
                        origin: Vec3::new(0., EYE, -1.),
                    },
                    crate::loot::stock(1),
                ))
                .id();
            app.world_mut().resource_mut::<Input>().use_item = true;
            app.world_mut().run_system_once(interact).unwrap();
            let s = app.world().resource::<Match>();
            assert_eq!(s.carried.as_deref(), Some(class));
            assert_eq!(s.held_class(), class);
            assert_eq!(s.slot, 6);
            assert_eq!(s.pistol.ammo, original);
            assert!(app.world().get::<Taken>(pickup).is_some());
            app.world_mut()
                .resource_mut::<Match>()
                .select_inventory_slot(2);
            app.world_mut()
                .run_system_once(crate::carryables::update)
                .unwrap();
            assert!(app.world().resource::<Match>().carried.is_none());
            assert_eq!(
                app.world_mut()
                    .query_filtered::<&Pickup, Without<Taken>>()
                    .iter(app.world())
                    .count(),
                1
            );
        }
    }
    #[test]
    fn hotel_plank_real_player_ticks_do_not_latch_on_edges() {
        use bevy::ecs::system::RunSystemOnce;
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let center = Vec3::from_array(crate::binary::source_position([554.4, 5636.4, 2849.8]));
        for angle in 0..8 {
            for height in [HEIGHT, DUCK_HEIGHT] {
                let direction =
                    Quat::from_rotation_y(angle as f32 * std::f32::consts::FRAC_PI_4) * Vec3::NEG_Z;
                let start = collision.snap_ground(center - direction * 1.2, height);
                let (mut app, enemy) = game_app();
                app.world_mut().despawn(enemy);
                app.insert_resource(collision.clone());
                app.world_mut()
                    .resource_mut::<Time<Fixed>>()
                    .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(KeyCode::KeyW);
                if height == DUCK_HEIGHT {
                    app.world_mut()
                        .resource_mut::<ButtonInput<KeyCode>>()
                        .press(KeyCode::ControlLeft);
                }
                {
                    let mut q = app.world_mut().query::<&mut Body>();
                    let mut b = q.single_mut(app.world_mut()).unwrap();
                    b.feet = start;
                    b.yaw = angle as f32 * std::f32::consts::FRAC_PI_4;
                    b.grounded = true;
                }
                for _ in 0..100 {
                    app.world_mut().run_system_once(move_player).unwrap();
                }
                let end = app
                    .world_mut()
                    .query::<&Body>()
                    .single(app.world())
                    .unwrap()
                    .feet;
                println!(
                    "PLAYER_PLANK {angle} h={height} travel={} start={start:?} end={end:?}",
                    (end - start).xz().length()
                );
                if (end - start).xz().length() < 1.8 {
                    let b = app
                        .world_mut()
                        .query::<&Body>()
                        .single(app.world())
                        .unwrap();
                    println!(
                        "SNAG velocity={:?} grounded={} fraction={} clear={}",
                        b.velocity,
                        b.grounded,
                        b.stance.fraction,
                        collision.clear_hull(end, height)
                    );
                    for lift in [0., 0.25, 0.5, 1., 2., 3., 4., 8., 18.] {
                        let at = end + Vec3::Y * lift * UNIT;
                        let t = collision.trace(
                            at + Vec3::Y * height * 0.5,
                            at + Vec3::Y * height * 0.5 + direction * 0.1,
                            Vec3::new(
                                crate::collision::RADIUS,
                                height * 0.5,
                                crate::collision::RADIUS,
                            ),
                        );
                        println!(
                            "SNAG lift={lift} trace={t:?} grounded={}",
                            collision.grounded(at, height)
                        );
                    }
                }
                assert!(
                    (end - start).xz().length() > 1.8,
                    "player snagged angle={angle} height={height}"
                );
            }
        }
    }
    #[test]
    fn dark_carnival_native_slopes_and_dips_keep_the_player_moving() {
        use bevy::ecs::system::RunSystemOnce;
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m1_highway.bsp");
        if !path.exists() {
            return;
        }
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let reference = Vec3::from_array(crate::binary::source_position([2205.6, 288.4, -932.8]));
        let mut candidates: Vec<_> = logic
            .world
            .bsp
            .displacement_triangles()
            .into_iter()
            .filter_map(|(_, tri)| {
                let tri =
                    tri.map(|p| Vec3::from_array(crate::binary::source_position(p.to_array())));
                let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or_zero();
                let normal = if normal.y < 0. { -normal } else { normal };
                if !(0.75..0.995).contains(&normal.y) {
                    return None;
                }
                let center = (tri[0] + tri[1] + tri[2]) / 3.;
                Some((center.distance_squared(reference), center, normal))
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut checked = 0;
        for (_, center, normal) in candidates
            .into_iter()
            .take(3000)
            .flat_map(|(d, c, n)| [(d, c, n), (d, c, -n)])
        {
            let direction = Vec3::new(-normal.x, 0., -normal.z).normalize_or_zero();
            let Ok(start) = collision.supported_spawn(center, HEIGHT) else {
                continue;
            };
            let Ok(goal) = collision.supported_spawn(start + direction * 1.1, HEIGHT) else {
                continue;
            };
            if (goal.y - start.y).abs() > 0.35 {
                continue;
            }
            let raised = Vec3::Y * (18. * UNIT + 0.005);
            let offset = Vec3::Y * HEIGHT * 0.5;
            if collision
                .trace(
                    start + offset + raised,
                    goal + offset + raised,
                    Vec3::new(
                        crate::collision::RADIUS,
                        HEIGHT * 0.5,
                        crate::collision::RADIUS,
                    ),
                )
                .fraction
                < 0.99
            {
                continue;
            }
            for duck in [false, true] {
                let (mut app, enemy) = game_app();
                app.world_mut().despawn(enemy);
                app.insert_resource(collision.clone());
                app.world_mut()
                    .resource_mut::<Time<Fixed>>()
                    .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(KeyCode::KeyW);
                if duck {
                    app.world_mut()
                        .resource_mut::<ButtonInput<KeyCode>>()
                        .press(KeyCode::ControlLeft);
                }
                {
                    let mut q = app.world_mut().query::<&mut Body>();
                    let mut b = q.single_mut(app.world_mut()).unwrap();
                    b.feet = start;
                    b.yaw = (-direction.x).atan2(-direction.z);
                    b.grounded = true;
                }
                let mut progress = 0.;
                for _ in 0..80 {
                    app.world_mut().run_system_once(move_player).unwrap();
                    let feet = app
                        .world_mut()
                        .query::<&Body>()
                        .single(app.world())
                        .unwrap()
                        .feet;
                    progress = (feet - start).dot(direction);
                    if progress > 0.85 {
                        break;
                    }
                }
                println!(
                    "CARNIVAL_ROUTE {checked} duck={duck} start={start:?} goal={goal:?} progress={progress}"
                );
                assert!(
                    progress > 0.75,
                    "walkable terrain snag: route {checked}, duck={duck}"
                );
            }
            checked += 1;
            if checked >= 24 {
                break;
            }
        }
        assert!(
            checked >= 12,
            "too few unblocked terrain fixtures: {checked}"
        );
    }
    #[test]
    fn dark_carnival_steep_contact_allows_retreat() {
        use bevy::ecs::system::RunSystemOnce;
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m1_highway.bsp");
        if !path.exists() {
            return;
        }
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let start = Vec3::from_array(crate::binary::source_position([
            2010.3455, 231.054, -1270.6261,
        ]));
        let down = collision.trace(
            start + Vec3::Y * HEIGHT * 0.5,
            start + Vec3::Y * (HEIGHT * 0.5 - 0.06),
            Vec3::new(
                crate::collision::RADIUS,
                HEIGHT * 0.5,
                crate::collision::RADIUS,
            ),
        );
        println!(
            "STEEP_CONTACT ground={down:?} clear={}",
            collision.clear_hull(start, HEIGHT)
        );
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(collision);
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        {
            let mut q = app.world_mut().query::<&mut Body>();
            let mut b = q.single_mut(app.world_mut()).unwrap();
            b.feet = start;
            b.yaw = 105.6_f32.to_radians() - std::f32::consts::FRAC_PI_2;
        }
        for _ in 0..40 {
            app.world_mut().run_system_once(move_player).unwrap();
        }
        let end = app
            .world_mut()
            .query::<&Body>()
            .single(app.world())
            .unwrap()
            .feet;
        println!(
            "STEEP_RETREAT start={start:?} end={end:?} distance={}",
            (end - start).xz().length()
        );
        assert!(
            (end - start).xz().length() > 0.8,
            "steep contact locks movement in both directions"
        );
    }
    #[test]
    fn ammunition_pack_deploys_once_then_player_claims_one_clip() {
        use bevy::ecs::system::RunSystemOnce;
        for kind in [
            crate::upgrades::Kind::Incendiary,
            crate::upgrades::Kind::Explosive,
        ] {
            let (mut app, enemy) = game_app();
            app.world_mut().despawn(enemy);
            app.init_resource::<crate::upgrades::Effects>();
            let mut rifle = pistol();
            rifle.class = "weapon_rifle".into();
            rifle.clip = 50;
            app.world_mut()
                .resource_mut::<Rules>()
                .weapons
                .insert(rifle.class.clone(), rifle.clone());
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.primary = Some(Gun {
                    class: rifle.class,
                    ammo: 9,
                    reserve: 200,
                    laser: false,
                    ammo_upgrade: None,
                    upgraded_rounds: 0,
                });
                state.upgrade_pack = Some(kind);
                state.slot = 4;
            }
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            app.world_mut().resource_mut::<Input>().fire_held = true;
            for _ in 0..30 {
                app.world_mut()
                    .run_system_once(crate::upgrades::update)
                    .unwrap();
            }
            assert!(app.world().resource::<Match>().upgrade_pack.is_some());
            for _ in 0..35 {
                app.world_mut()
                    .run_system_once(crate::upgrades::update)
                    .unwrap();
            }
            assert!(app.world().resource::<Match>().upgrade_pack.is_none());
            let count = app
                .world_mut()
                .query::<&crate::upgrades::Claims>()
                .iter(app.world())
                .count();
            assert_eq!(count, 1);
            app.world_mut()
                .query::<&mut Body>()
                .single_mut(app.world_mut())
                .unwrap()
                .pitch = -1.2;
            app.world_mut().resource_mut::<Input>().use_item = true;
            app.world_mut().run_system_once(interact).unwrap();
            let gun = app.world().resource::<Match>().primary.as_ref().unwrap();
            assert_eq!(gun.ammo, 50);
            assert_eq!(gun.ammo_upgrade, Some(kind));
            assert_eq!(gun.upgraded_rounds, 50);
            app.world_mut()
                .resource_mut::<Match>()
                .primary
                .as_mut()
                .unwrap()
                .ammo = 10;
            app.world_mut().resource_mut::<Input>().use_item = true;
            app.world_mut().run_system_once(interact).unwrap();
            assert_eq!(
                app.world()
                    .resource::<Match>()
                    .primary
                    .as_ref()
                    .unwrap()
                    .ammo,
                10
            );
        }
    }
    #[test]
    fn laser_box_is_reusable_and_upgrades_the_primary_only() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.world_mut().resource_mut::<Match>().primary = Some(Gun {
            class: "weapon_rifle".into(),
            ammo: 50,
            reserve: 100,
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
        });
        let entity = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "upgrade_laser_sight".into(),
                    origin: Vec3::new(0., EYE, -1.),
                },
                crate::loot::stock(-1),
            ))
            .id();
        app.world_mut().resource_mut::<Input>().use_item = true;
        app.world_mut().run_system_once(interact).unwrap();
        assert!(
            app.world()
                .resource::<Match>()
                .primary
                .as_ref()
                .unwrap()
                .laser
        );
        assert!(!app.world().resource::<Match>().pistol.laser);
        assert!(app.world().get::<Taken>(entity).is_none());
    }
    #[test]
    fn commons_never_target_or_damage_an_infected_player_without_survivors() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.insert_resource(Collision::test_solid_floor());
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.infected = true;
            s.health = 250.;
        }
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = Vec3::new(0., 0., -0.3);
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        for _ in 0..60 {
            app.world_mut().run_system_once(enemy_ai).unwrap();
        }
        assert_eq!(app.world().resource::<Match>().health, 250.);
        assert_eq!(
            app.world().get::<Transform>(enemy).unwrap().translation,
            Vec3::new(0., 0., -0.3)
        );
    }
    #[test]
    fn hunter_relaunches_outward_from_wall_and_ceiling_contacts() {
        use bevy::ecs::system::RunSystemOnce;
        for ceiling in [false, true] {
            let (mut app, enemy) = game_app();
            app.world_mut().despawn(enemy);
            app.init_resource::<crate::special_infected::Hunter>();
            let mut collision = Collision::test_solid_floor();
            if ceiling {
                collision.test_add_box(Vec3::new(-2., 2., -2.), Vec3::new(2., 2.1, 2.));
            } else {
                collision.test_add_box(Vec3::new(1., -1., -2.), Vec3::new(1.1, 4., 2.));
            }
            app.insert_resource(collision);
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            {
                let mut s = app.world_mut().resource_mut::<Match>();
                s.infected = true;
                s.health = 250.;
                s.special_phase = 2;
            }
            let start = if ceiling {
                Vec3::Y * 0.58
            } else {
                Vec3::new(0.58, 1., 0.)
            };
            app.world_mut()
                .resource_mut::<crate::special_infected::Hunter>()
                .test_start_pounce(start);
            {
                let mut q = app.world_mut().query::<&mut Body>();
                let mut b = q.single_mut(app.world_mut()).unwrap();
                b.feet = start;
                b.grounded = false;
                b.crouched = true;
                b.stance.fraction = 1.;
                b.velocity = if ceiling {
                    Vec3::Y * 15.
                } else {
                    Vec3::new(15., 3., 0.)
                };
            }
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ControlLeft);
            app.world_mut().run_system_once(move_player).unwrap();
            let contact_feet = {
                let mut q = app.world_mut().query::<&mut Body>();
                let mut b = q.single_mut(app.world_mut()).unwrap();
                b.yaw = std::f32::consts::FRAC_PI_2;
                b.pitch = if ceiling {
                    -std::f32::consts::FRAC_PI_2
                } else {
                    0.
                };
                assert!(b.contact_normal.length_squared() > 0.5);
                b.feet
            };
            // A held attack already down before turning must also rebound.
            app.world_mut().resource_mut::<Input>().fire_held = true;
            app.world_mut()
                .run_system_once(crate::special_infected::tick)
                .unwrap();
            let b = app
                .world_mut()
                .query::<&Body>()
                .single(app.world())
                .unwrap();
            assert!(!b.grounded);
            if ceiling {
                assert!(b.velocity.y < -400. * UNIT);
                assert_eq!(b.feet, contact_feet);
            } else {
                assert!(b.velocity.x < -599. * UNIT);
                assert!((b.velocity.y / UNIT - 150.).abs() < 0.01);
                assert!((b.feet.y - contact_feet.y - 20. * UNIT).abs() < 0.00001);
            }
            assert_eq!(app.world().resource::<Match>().special_phase, 2);
            assert_eq!(app.world().resource::<Match>().shot_serial, 1);
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(0.2));
            {
                let mut q = app.world_mut().query::<&mut Body>();
                q.single_mut(app.world_mut()).unwrap().feet = contact_feet;
            }
            app.world_mut()
                .run_system_once(crate::special_infected::tick)
                .unwrap();
            assert_eq!(
                app.world().resource::<Match>().shot_serial,
                2,
                "each rebound must restart the native viewmodel lunge"
            );
            assert_eq!(
                app.world()
                    .resource::<game_audio::Queue>()
                    .0
                    .iter()
                    .filter(|sound| sound.name == "HunterZombie.Pounce")
                    .count(),
                1,
                "retail FUN_1031e000 skips the pounce yell within 1.5 s (rebound)"
            );
        }
    }
    #[test]
    fn crouched_hunter_can_pounce_again_on_landing_without_recharging() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(crate::special_infected::Hunter::default());
        {
            let mut state = app.world_mut().resource_mut::<Match>();
            state.infected = true;
            state.health = 250.;
        }
        app.world_mut()
            .resource_mut::<crate::special_infected::Hunter>()
            .charge = 1.;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        app.world_mut().resource_mut::<Input>().fire_held = true;
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(0.3));
            {
                let mut q = app.world_mut().query::<&mut Body>();
                let mut body = q.single_mut(app.world_mut()).unwrap();
                body.grounded = true;
                body.feet = Vec3::ZERO;
            }
            app.world_mut()
                .run_system_once(crate::special_infected::tick)
                .unwrap();
            assert!(
                !app.world_mut()
                    .query::<&Body>()
                    .single(app.world())
                    .unwrap()
                    .grounded
            );
        }
        assert_eq!(app.world().resource::<Match>().shot_serial, 2);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ControlLeft);
        app.world_mut()
            .run_system_once(crate::special_infected::tick)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<crate::special_infected::Hunter>()
                .charge,
            0.
        );
    }
    #[test]
    fn infected_spawn_click_stays_consumed_until_release_for_every_ghost_class() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::special_infected::{Hunter, SpecialClass};
        for class in SpecialClass::ALL.into_iter().filter(|c| *c != SpecialClass::Tank) {
            let (mut app, _) = game_app();
            app.insert_resource(Collision::test_solid_floor());
            let mut hunter = Hunter::default();
            hunter.ghost = true;
            app.insert_resource(hunter);
            {
                let mut state = app.world_mut().resource_mut::<Match>();
                state.infected = true;
                state.special_class = class;
                state.health = class.health();
            }
            app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().grounded = true;
            app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::ControlLeft);
            app.world_mut().resource_mut::<Input>().sample_primary(true, true);
            app.world_mut().run_system_once(crate::special_infected::tick).unwrap();
            assert!(!app.world().resource::<Hunter>().ghost, "{class:?} did not spawn");
            // Keep holding for longer than Hunter's charge. The same physical
            // click must not turn into any class's held primary ability.
            for _ in 0..45 {
                app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut().resource_mut::<Input>().sample_primary(true, false);
                app.world_mut().run_system_once(crate::special_infected::tick).unwrap();
                let input = app.world().resource::<Input>();
                assert!(!input.fire && !input.fire_held && !input.throw_held, "{class:?} leaked spawn attack");
                assert_eq!(app.world().resource::<Match>().shot_serial, 0, "{class:?} attacked on spawn");
            }
            app.world_mut().resource_mut::<Input>().sample_primary(false, false);
            app.world_mut().resource_mut::<Input>().sample_primary(true, true);
            assert!(app.world().resource::<Input>().fire && app.world().resource::<Input>().fire_held);
            if class == SpecialClass::Hunter {
                app.world_mut().run_system_once(crate::special_infected::tick).unwrap();
                assert_eq!(app.world().resource::<Match>().shot_serial, 1, "fresh press should pounce without an added cooldown");
            }
        }
    }
    #[test]
    fn primary_sampling_preserves_normal_tap_buffer_and_held_attack_after_spawn_release() {
        let mut input = Input::default();
        input.sample_primary(true, true);
        input.sample_primary(false, false);
        assert!(input.fire, "a tap must remain buffered until the server tick consumes it");
        input.consume_spawn_attack();
        for _ in 0..5 { input.sample_primary(true, false); }
        assert!(!input.fire && !input.fire_held);
        input.sample_primary(false, false);
        input.sample_primary(true, true);
        assert!(std::mem::take(&mut input.fire));
        input.sample_primary(true, false);
        assert!(!input.fire && input.fire_held, "normal held abilities must still work after the new press");
    }
    fn game_app() -> (App, Entity) {
        let mut app = App::new();
        let weapon = pistol();
        let mut state = Match::new(&weapon);
        state.paused = false;
        app.insert_resource(Time::<Fixed>::default())
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(Input::default())
            .insert_resource(Collision::default())
            .insert_resource(state)
            .insert_resource(Rules {
                weapons: HashMap::from([(weapon.class.clone(), weapon.clone())]),
                pistol: weapon,
            })
            .insert_resource(Squad(vec![]))
            .insert_resource(crate::campaign_settings::SessionSettings {
                map: std::path::PathBuf::new(),
                survivor: crate::campaign_settings::Survivor::Coach,
                difficulty: crate::campaign_settings::Difficulty::Normal,
                original: false,
            })
            .init_resource::<game_audio::Queue>()
            .insert_resource(viewer::RenderModels(HashMap::new()))
            .insert_resource(Campaign {
                spawn: Vec3::ZERO,
                angles: [0.; 3],
                pickups: vec![],
                enemy_model: String::new(),
                population: Default::default(),
                total_spawned: 0,
            })
            .add_systems(
                FixedUpdate,
                (
                    move_player,
                    timers,
                    interact,
                    crate::survivor_items::update,
                    combat,
                    melee::swing,
                    director,
                    squad_ai,
                    enemy_ai,
                )
                    .chain(),
            );
        app.world_mut().spawn((
            Body {
                feet: Vec3::ZERO,
                velocity: Vec3::ZERO,
                grounded: false,
                crouched: false,
                stance: default(),
                previous_feet: Vec3::ZERO,
                previous_eye: EYE,
                contact_normal: Vec3::ZERO,
                yaw: 0.,
                pitch: 0.,
            },
            Transform::default(),
            viewer::GameCamera,
        ));
        let enemy = app
            .world_mut()
            .spawn((
                Enemy {
                    model: "models/infected/common_male01.mdl".into(),
                    death_loot: None,
                    loot_dropped: false,
                    reaction_serial: 0,
                    attack_serial: 0,
                    health: 50.,
                    attack: 0.,
                    vertical: 0.,
                    reaction: 0,
                    corpse_time: 0.,
                    death: Default::default(),
                    movement_speed: 0.,
                    burning: 0.,
                },
                Transform::from_xyz(0., 0., -4.),
            ))
            .id();
        (app, enemy)
    }
    #[test]
    fn phone_drag_rotates_camera_with_move_fire_cancel_and_reload() {
        use bevy::input::{InputPlugin, touch::{TouchInput, TouchPhase}};
        fn frame(app: &mut App) {
            // Emulate iOS restoring None after an unsupported cursor grab.
            for mut cursor in app.world_mut().query::<&mut CursorOptions>().iter_mut(app.world_mut()) {
                cursor.grab_mode = CursorGrabMode::None;
            }
            app.world_mut().run_schedule(PreUpdate);
            app.world_mut().run_schedule(Update);
        }
        fn finger(app: &mut App, window: Entity, id: u64, phase: TouchPhase, x: f32, y: f32) {
            app.world_mut().write_message(TouchInput { phase, position: Vec2::new(x, y), window, force: None, id });
        }
        fn angles(app: &mut App) -> (f32, f32) {
            let body = app.world_mut().query::<&Body>().single(app.world()).unwrap();
            (body.yaw, body.pitch)
        }
        let (mut app, _) = game_app();
        app.add_plugins(InputPlugin)
            .insert_resource(Time::<()>::default())
            .insert_resource(game_menu::Frontend::playing_for_test())
            .add_systems(PreUpdate, touch::inject.after(bevy::input::InputSystems))
            .add_systems(Update, controls);
        let window = app.world_mut().spawn((Window {
            resolution: bevy::window::WindowResolution::new(2000, 1000), ..default()
        }, CursorOptions::default())).id();
        touch::rebuild(app.world_mut());
        app.world_mut().resource_mut::<touch::TouchControls>().enabled = true;
        frame(&mut app);
        let fire = touch::layout(2000., 1000.)[0].1;
        finger(&mut app, window, 1, TouchPhase::Started, 300., 700.);
        finger(&mut app, window, 2, TouchPhase::Started, fire.x, fire.y);
        finger(&mut app, window, 3, TouchPhase::Started, 1300., 300.);
        frame(&mut app);
        finger(&mut app, window, 1, TouchPhase::Moved, 300., 560.);
        finger(&mut app, window, 3, TouchPhase::Moved, 1320., 320.);
        frame(&mut app);
        let first = angles(&mut app);
        // 20 px x LOOK_SCALE 1.6 x default look speed 1.5 x 0.002 rad.
        assert!((first.0 + 0.096).abs() < 1e-5 && (first.1 + 0.096).abs() < 1e-5, "camera yaw/pitch={first:?}");
        assert!(app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
        assert!(app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
        frame(&mut app);
        assert_eq!(angles(&mut app), first, "stationary finger must not drift");
        finger(&mut app, window, 3, TouchPhase::Canceled, 1320., 320.);
        frame(&mut app);
        assert_eq!(angles(&mut app), first);
        // Reload drops old gesture ownership; a fresh drag must still work.
        touch::rebuild(app.world_mut());
        finger(&mut app, window, 4, TouchPhase::Started, 1300., 300.);
        frame(&mut app);
        finger(&mut app, window, 4, TouchPhase::Moved, 1280., 280.);
        frame(&mut app);
        let after = angles(&mut app);
        assert!(after.0.abs() < 1e-5 && after.1.abs() < 1e-5, "post-reload yaw/pitch={after:?}");
        println!("Phone look check: yaw/pitch rotate, move+fire coexist, stationary/canceled touches do not drift, reload restores look");
    }
    #[test]
    fn focus_loss_and_click_recovery_clear_input_without_stale_mouse_motion() {
        let (mut app, _) = game_app();
        app.insert_resource(ButtonInput::<MouseButton>::default())
            .insert_resource(AccumulatedMouseMotion::default())
            .add_message::<MouseWheel>()
            .insert_resource(game_menu::Frontend {
                page: game_menu::Page::Game,
                mode: 0,
                restart: false,
                started: true,
            })
            .init_resource::<crate::game_console::Console>()
            .add_systems(Update, controls);
        let window = app
            .world_mut()
            .spawn((Window::default(), CursorOptions::default()))
            .id();
        app.world_mut().run_schedule(Update);
        assert!(!app.world().get::<CursorOptions>(window).unwrap().visible);
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        app.world_mut().resource_mut::<Input>().fire = true;
        app.world_mut().run_schedule(Update);
        // Losing focus keeps the game running but drops input.
        assert!(!app.world().resource::<Match>().paused);
        assert!(!app.world().resource::<Input>().fire);
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None
        );
        app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        app.world_mut()
            .resource_mut::<AccumulatedMouseMotion>()
            .delta = Vec2::splat(1000.);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut().run_schedule(Update);
        assert!(!app.world().resource::<Match>().paused);
        assert!(!app.world().resource::<Input>().fire);
        assert!(!app.world().get::<CursorOptions>(window).unwrap().visible);
        assert_eq!(
            app.world_mut()
                .query::<&Body>()
                .single(app.world())
                .unwrap()
                .yaw,
            0.
        );
        app.world_mut()
            .resource_mut::<crate::game_console::Console>()
            .open = true;
        app.world_mut().run_schedule(Update);
        assert!(app.world().get::<CursorOptions>(window).unwrap().visible);
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None
        );
    }
    #[test]
    fn wheel_messages_select_inventory_and_do_not_replay_after_pause() {
        let (mut app, _) = game_app();
        app.insert_resource(ButtonInput::<MouseButton>::default())
            .insert_resource(AccumulatedMouseMotion::default())
            .add_message::<MouseWheel>()
            .insert_resource(game_menu::Frontend {
                page: game_menu::Page::Game,
                mode: 0,
                restart: false,
                started: true,
            })
            .init_resource::<crate::game_console::Console>()
            .add_systems(Update, controls);
        let window = app
            .world_mut()
            .spawn((Window::default(), CursorOptions::default()))
            .id();
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.primary = Some(s.pistol.clone());
            s.slot = 1;
            s.throwable = Some("weapon_pipe_bomb".into());
            s.medkit = true;
            s.pills = true;
        }
        app.world_mut().run_schedule(Update);
        for (unit, y, expected) in [
            (MouseScrollUnit::Line, -1., 2),
            (MouseScrollUnit::Line, -3., 5),
            (MouseScrollUnit::Line, -1., 1),
            (MouseScrollUnit::Line, 1., 5),
            (MouseScrollUnit::Pixel, 20., 5),
            (MouseScrollUnit::Pixel, 20., 4),
        ] {
            app.world_mut().write_message(MouseWheel {
                unit,
                x: 0.,
                y,
                window,
                phase: bevy::input::touch::TouchPhase::Moved,
            });
            app.world_mut().run_schedule(Update);
            assert_eq!(app.world().resource::<Match>().slot, expected);
        }
        app.world_mut().resource_mut::<Match>().paused = true;
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.,
            y: -1.,
            window,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<Match>().slot, 4);
        app.world_mut().resource_mut::<Match>().paused = false;
        app.world_mut().run_schedule(Update);
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<Match>().slot, 4);
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.,
            y: -1.,
            window,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<Match>().slot, 5);
    }
    #[test]
    fn mudman_claw_hit_triggers_screen_effect_but_not_again_during_attack_cooldown() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, id) = game_app();
        app.world_mut().get_mut::<Enemy>(id).unwrap().model =
            "models/infected/common_male_mud.mdl".into();
        app.world_mut()
            .get_mut::<Transform>(id)
            .unwrap()
            .translation = Vec3::new(0., 0., -0.8);
        app.world_mut().run_system_once(enemy_ai).unwrap();
        assert_eq!(app.world().resource::<Match>().mud_serial, 1);
        assert_eq!(app.world().resource::<Match>().health, 98.);
        app.world_mut().run_system_once(enemy_ai).unwrap();
        assert_eq!(app.world().resource::<Match>().mud_serial, 1);
    }
    #[test]
    fn infected_routes_around_the_lab_wall_and_reaches_the_survivor() {
        let (mut app, enemy) = game_app();
        let path = crate::test_lab::path();
        app.insert_resource(Collision::load(&path).unwrap());
        app.insert_resource(crate::l4d2r_navigation::Navigation::load(&path));
        let position = |p| Vec3::from_array(binary::source_position(p));
        let target = position([1920., 600., 4.]);
        let body = app
            .world_mut()
            .query_filtered::<Entity, With<Body>>()
            .single(app.world())
            .unwrap();
        app.world_mut().get_mut::<Body>(body).unwrap().feet = target;
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = position([1700., 600., 4.]);
        let mut detour = false;
        for _ in 0..900 {
            tick(&mut app);
            assert!(
                app.world().get::<Body>(body).unwrap().feet.y >= 0.,
                "player lost floor support at {:?}",
                app.world().get::<Body>(body).unwrap().feet
            );
            let p = app.world().get::<Transform>(enemy).unwrap().translation;
            let y = -p.z / UNIT;
            detour |= y < 180. || y > 1070.;
            assert!(p.is_finite());
            if p.distance(target) < 1.3 {
                break;
            }
        }
        let last = app.world().get::<Transform>(enemy).unwrap().translation;
        assert!(
            detour && last.distance(target) < 1.3,
            "infected failed the wall route: {last:?}, detour={detour}, player={:?}, {}",
            app.world().get::<Body>(body).unwrap().feet,
            app.world()
                .resource::<crate::l4d2r_navigation::Navigation>()
                .debug_infected(enemy.to_bits())
        );
    }
    #[test]
    fn overlapping_infected_separate_in_the_real_ai_schedule() {
        let (mut app, enemy) = game_app();
        app.insert_resource(Collision::test_floor());
        let start = Transform::from_xyz(0., 0., -5.);
        *app.world_mut().get_mut::<Transform>(enemy).unwrap() = start;
        let other = app
            .world_mut()
            .spawn((
                start,
                Enemy {
                    model: "models/infected/common_male01.mdl".into(),
                    death_loot: None,
                    loot_dropped: false,
                    health: 50.,
                    attack: 0.,
                    vertical: 0.,
                    reaction: 0,
                    corpse_time: 0.,
                    death: Default::default(),
                    movement_speed: 0.,
                    burning: 0.,
                    reaction_serial: 0,
                    attack_serial: 0,
                },
            ))
            .id();
        for _ in 0..180 {
            tick(&mut app);
        }
        let a = app.world().get::<Transform>(enemy).unwrap().translation;
        let b = app.world().get::<Transform>(other).unwrap().translation;
        assert!(
            a.is_finite() && b.is_finite() && a.distance(b) > 0.45,
            "crowd remained coincident: {a:?} {b:?}"
        );
    }
    #[test]
    fn console_commands_move_heal_give_and_reject_invalid_values() {
        let (mut app, _) = game_app();
        let rules = app.world_mut().remove_resource::<Rules>().unwrap();
        let mut game = Match::new(&rules.pistol);
        let settings = app
            .world()
            .resource::<crate::campaign_settings::SessionSettings>()
            .clone();
        let mut body = Body {
            feet: Vec3::ZERO,
            velocity: Vec3::ONE,
            grounded: true,
            crouched: false,
            stance: default(),
            previous_feet: Vec3::ZERO,
            previous_eye: EYE,
            contact_normal: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
        };
        let mut c = crate::game_console::Console::default();
        let mut requests = crate::map_transition::Requests::default();
        let mut frontend = game_menu::Frontend {
            page: game_menu::Page::Game,
            mode: 0,
            restart: false,
            started: true,
        };
        let mut command = |text: &str,
                           c: &mut crate::game_console::Console,
                           game: &mut Match,
                           body: &mut Body| {
            crate::game_console::execute(
                text,
                c,
                game,
                &rules,
                body,
                &Collision::default(),
                &mut requests,
                &settings,
                Path::new("."),
                &mut frontend,
            )
        };
        assert_eq!(
            command("noclip", &mut c, &mut game, &mut body),
            "noclip true"
        );
        assert!(c.noclip);
        command("setpos 100 200 300", &mut c, &mut game, &mut body);
        assert!(body.feet.distance(Vec3::new(2.54, 7.62, -5.08)) < 0.0001);
        let position = body.feet;
        assert!(command("setpos NaN 0 0", &mut c, &mut game, &mut body).starts_with("Usage"));
        assert_eq!(body.feet, position);
        command("hurt 25", &mut c, &mut game, &mut body);
        assert_eq!(game.health, 75.);
        command("heal", &mut c, &mut game, &mut body);
        assert_eq!(game.health, 100.);
        command("give first_aid_kit", &mut c, &mut game, &mut body);
        assert!(game.medkit);
        command("flashlight", &mut c, &mut game, &mut body);
        assert!(c.flashlight);
        assert!(command("map ../outside", &mut c, &mut game, &mut body).contains("only letters"));
        assert!(requests.0.is_none());
    }
    #[test]
    fn bots_walk_beside_a_walking_human_and_run_to_catch_up() {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.health = 100.;
        let near = 150. * UNIT;
        let far = 260. * UNIT;
        assert!((bot_move_speed(&state, true, near) - 85. * UNIT).abs() < 1e-6);
        assert!((bot_move_speed(&state, true, far) - 220. * UNIT).abs() < 1e-6);
        assert!((bot_move_speed(&state, false, near) - 220. * UNIT).abs() < 1e-6);
        state.health = 30.;
        state.temporary = 0.;
        assert!((bot_move_speed(&state, true, near) - 85. * UNIT).abs() < 1e-6);
        assert!((bot_move_speed(&state, false, far) - 150. * UNIT).abs() < 1e-6);
        // Backpedalling once moving walks; from a standstill the first step
        // is not capped; moving toward the view is not capped.
        let back = Vec3::new(0., 0., 220. * UNIT);
        let aim = Vec3::new(0., 0., -1.);
        assert!((backing_walk(back, aim, back).length() - 85. * UNIT).abs() < 1e-5);
        assert_eq!(backing_walk(back, aim, Vec3::ZERO), back);
        assert_eq!(backing_walk(back, -aim, back), back);
    }
    #[test]
    fn walking_off_a_high_ledge_grabs_it_and_letting_go_falls() {
        use bevy::ecs::system::RunSystemOnce;
        // A 40 m (1575-unit) cliff: roof top at y 0, wall face at x 0.
        let mut collision = Collision::default();
        collision.test_add_box(Vec3::new(-20., -40., -20.), Vec3::new(0., 0., 20.));
        collision.test_add_box(Vec3::new(-20., -44., -20.), Vec3::new(40., -40., 20.));
        for (bots, grabs) in [(0, false), (1, true)] {
            let (mut app, enemy) = game_app();
            app.world_mut().despawn(enemy);
            app.insert_resource(collision.clone());
            let squad: Vec<_> = (0..bots).map(|_| bot(Vec3::new(-5., 0., 0.), &pistol())).collect();
            app.insert_resource(Squad(squad));
            app.init_resource::<crate::ledge_hang::Tracker>();
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            {
                let world = app.world_mut();
                let mut q = world.query::<&mut Body>();
                let mut b = q.single_mut(world).unwrap();
                b.feet = Vec3::new(-0.2, 0., 0.);
                b.velocity = Vec3::X * 250. * UNIT;
                b.grounded = true;
            }
            for _ in 0..30 {
                app.world_mut().run_system_once(move_player).unwrap();
                if app.world().resource::<Match>().ledge.is_some() {
                    break;
                }
            }
            let hang = app.world().resource::<Match>().ledge;
            assert_eq!(hang.is_some(), grabs, "last survivor standing never grabs");
            let Some(hang) = hang else { continue };
            assert!(app.world().resource::<Match>().incap);
            assert!((hang.hang.x - (RADIUS + UNIT)).abs() < 0.01, "{hang:?}");
            assert!((hang.hang.y + 80. * UNIT).abs() < 0.01, "{hang:?}");
            assert!(hang.stand.x < 0. && hang.stand.y.abs() < 0.05, "{hang:?}");
            // Hanging holds the player in place.
            for _ in 0..10 {
                app.world_mut().run_system_once(move_player).unwrap();
            }
            let feet = app.world_mut().query::<&Body>().single(app.world()).unwrap().feet;
            assert_eq!(feet, hang.hang);
            // Out of hang health: let go and fall to the floor 1495 units below.
            app.world_mut().resource_mut::<Match>().incap_health = 0.;
            for _ in 0..200 {
                app.world_mut().run_system_once(move_player).unwrap();
            }
            let state = app.world().resource::<Match>();
            assert!(state.ledge.is_none());
            assert!(state.dead, "a 1500-unit drop is fatal");
        }
    }
    #[test]
    fn fall_damage_native_hotel_landing_uses_real_support() {
        use bevy::ecs::system::RunSystemOnce;
        let root=Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {return;}
        let path=root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic=crate::l4d2r_cinematic::MapLogic::load(root,&path,true).unwrap();
        let mut collision=Collision::load(&path).unwrap();collision.enable_l4d2r(root,&logic).unwrap();
        let mut landing=None;
        for x in [248.,350.,450.,554.,650.,750.] {for y in [5500.,5600.,5700.,5800.,5900.] {
            let p=Vec3::new(x*UNIT,2848.*UNIT,-y*UNIT);
            let Ok(p)=collision.supported_spawn_near(p,HEIGHT) else {continue};
            let high=p+Vec3::Y*4.*UNIT;
            let ray=collision.trace(high+Vec3::Y*HEIGHT*0.5,p+Vec3::Y*(HEIGHT*0.5+0.5*UNIT),Vec3::new(RADIUS,HEIGHT*0.5,RADIUS));
            if collision.clear_hull(high,HEIGHT) && !ray.start_solid && ray.fraction>0.99 {landing=Some(p);break;}
        } if landing.is_some() {break;} }
        let p=landing.expect("Hotel roof must have a clear hull landing fixture");
        for hz in [30.,60.] {
            let (mut app,enemy)=game_app();app.world_mut().despawn(enemy);app.insert_resource(collision.clone());
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1./hz));
            {let world=app.world_mut();let mut q=world.query::<&mut Body>();let mut b=q.single_mut(world).unwrap();b.feet=p+Vec3::Y*4.*UNIT;b.velocity=Vec3::NEG_Y*640.*UNIT;b.grounded=false;}
            for _ in 0..(hz as usize*2) {app.world_mut().run_system_once(move_player).unwrap();}
            let state=app.world().resource::<Match>();
            println!("Native Hotel landing {hz} Hz: base {p:?}, health {}, landings {}",state.health,state.damage_serial);
            assert!((state.health-(100.-crate::fall_damage::Settings::default().damage(640.+400./hz as f32))).abs()<0.01);assert_eq!(state.damage_serial,1);
        }
    }
    #[test]
    fn fall_damage_lands_once_with_native_curve_and_infected_immunity() {
        use bevy::ecs::system::RunSystemOnce;
        for hz in [30.,60.] { for infected in [false,true] {
            let (mut app,enemy)=game_app();app.world_mut().despawn(enemy);
            app.insert_resource(Collision::test_solid_floor());
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1./hz));
            app.world_mut().resource_mut::<Match>().infected=infected;
            { let world=app.world_mut();let mut q=world.query::<&mut Body>();let mut body=q.single_mut(world).unwrap();
                body.feet=Vec3::Y*(4.*UNIT);body.velocity=Vec3::NEG_Y*(640.*UNIT);body.grounded=false;
            }
            app.world_mut().run_system_once(move_player).unwrap();
            let expected=if infected {100.} else {100.-crate::fall_damage::Settings::default().damage(640.+400./hz as f32)};
            assert!((app.world().resource::<Match>().health-expected).abs()<0.001,"{hz} Hz infected={infected}");
            for _ in 0..60 {app.world_mut().run_system_once(move_player).unwrap();}
            assert!((app.world().resource::<Match>().health-expected).abs()<0.001,"landing repeated");
            assert_eq!(app.world().resource::<game_audio::Queue>().0.iter().filter(|r|r.name=="Player.FallDamage").count(),usize::from(!infected));
        }}
    }
    #[test]
    fn fall_damage_distinguishes_incap_death_and_jumps_at_both_tick_rates() {
        use bevy::ecs::system::RunSystemOnce;
        for hz in [30.,60.] { for (speed,dead,incap) in [(720.,false,true),(800.,true,false)] {
            let (mut app,enemy)=game_app();app.world_mut().despawn(enemy);app.insert_resource(Collision::test_solid_floor());
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1./hz));
            {let world=app.world_mut();let mut q=world.query::<&mut Body>();let mut b=q.single_mut(world).unwrap();b.feet=Vec3::Y*4.*UNIT;b.velocity=Vec3::NEG_Y*speed*UNIT;b.grounded=false;}
            app.world_mut().run_system_once(move_player).unwrap();
            let state=app.world().resource::<Match>();assert_eq!((state.dead,state.incap),(dead,incap));
        }
            let (mut app,enemy)=game_app();app.world_mut().despawn(enemy);app.insert_resource(Collision::test_solid_floor());
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1./hz));
            app.world_mut().resource_mut::<Input>().jump=true;
            for _ in 0..120 {app.world_mut().run_system_once(move_player).unwrap();}
            assert_eq!(app.world().resource::<Match>().health,100.,"ordinary jump {hz}");
        }
    }
    #[test]
    fn survivor_bots_keep_out_of_each_others_hulls() {
        let positions = vec![(Vec3::ZERO, true), (Vec3::new(0.2, 0., 0.), true), (Vec3::new(0.1, 0., 0.), false)];
        let player = Vec3::new(5., 0., 0.);
        let others = survivor_obstacles(0, &positions, Some(player));
        // Downed teammates are not obstacles; the far player is listed but too far to push.
        assert_eq!(others.len(), 2);
        let push = survivor_spacing(0, Vec3::ZERO, &others);
        assert!(push.x < -0.5 && push.z.abs() < 1e-5, "{push:?}");
        // Overlap: pushed out to exactly two hull radii.
        let out = survivor_overlap(0, Vec3::ZERO, &others).unwrap();
        assert!(((Vec3::ZERO + out).distance(positions[1].0) - 2. * RADIUS).abs() < 1e-4);
        // Coincident bots separate in opposite directions.
        let same = vec![(Vec3::ZERO, true), (Vec3::ZERO, true)];
        let a = survivor_overlap(0, Vec3::ZERO, &survivor_obstacles(0, &same, None)).unwrap();
        let b = survivor_overlap(1, Vec3::ZERO, &survivor_obstacles(1, &same, None)).unwrap();
        assert!(a.normalize().dot(b.normalize()) < -0.99);
        // Outside the spacing nothing happens.
        let far = vec![(Vec3::ZERO, true), (Vec3::new(3., 0., 0.), true)];
        assert_eq!(survivor_spacing(0, Vec3::ZERO, &survivor_obstacles(0, &far, None)), Vec3::ZERO);
        assert!(survivor_overlap(0, Vec3::ZERO, &survivor_obstacles(0, &far, None)).is_none());
    }
    #[test]
    fn bots_walk_down_stairs_on_the_ground() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = crate::test_lab::path();
        if !root.exists() || !path.exists() {
            return;
        }
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut c = Collision::load(&path).unwrap();
        c.enable_l4d2r(root, &logic).unwrap();
        let top = c
            .supported_spawn_near(Vec3::from_array(binary::source_position([3120., 2330., 148.])), HEIGHT)
            .unwrap();
        let base = Vec3::from_array(binary::source_position([2600., 2330., 4.]));
        let mut member = bot(top, &pistol());
        let mut airborne = 0;
        for _ in 0..300 {
            let wish = ((base - member.position) * Vec3::new(1., 0., 1.)).normalize_or_zero() * 3.8;
            crate::fall_damage::move_bot(&mut member, &c, wish, 1. / 30., Default::default());
            if member.vertical.abs() > 1. {
                airborne += 1;
            }
            if (member.position - base).xz().length() < 0.5 {
                break;
            }
        }
        assert!((member.position - base).xz().length() < 0.5, "reached the base: {:?}", member.position);
        // The survivor animstate treats |vertical| > 1 m/s as a jump.
        assert_eq!(airborne, 0, "ticks spent falling off steps");
    }
    #[test]
    fn fall_damage_bots_land_once_even_with_sb_stop() {
        use bevy::ecs::system::RunSystemOnce;
        for stopped in [false,true] {for hz in [30.,60.] {
            let (mut app,enemy)=game_app();app.world_mut().despawn(enemy);app.insert_resource(Collision::test_solid_floor());
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1./hz));
            let mut member=bot(Vec3::Y*4.*UNIT,&pistol());member.vertical=-640.*UNIT;
            app.world_mut().resource_mut::<Squad>().0.push(member);
            crate::game_state::set_bots_stopped(stopped);
            app.world_mut().run_system_once(squad_ai).unwrap();
            let health=app.world().resource::<Squad>().0[0].state.health;
            assert!(health<80. && health>60.,"bot landing health {health}");
            for _ in 0..20 {app.world_mut().run_system_once(squad_ai).unwrap();}
            assert_eq!(app.world().resource::<Squad>().0[0].state.health,health,"repeated bot landing");
            crate::game_state::set_bots_stopped(false);
        }}
    }
    #[test]
    fn noclip_moves_without_gravity() {
        let (mut app, _) = game_app();
        app.init_resource::<crate::game_console::Console>();
        app.world_mut()
            .resource_mut::<crate::game_console::Console>()
            .noclip = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        tick(&mut app);
        let body = app
            .world_mut()
            .query::<&Body>()
            .single(app.world())
            .unwrap();
        assert!(body.feet.y > 0.);
        // The flight is kept as velocity (read by the viewmodel walk blend),
        // still without gravity.
        assert!(body.velocity.y > 0. && body.velocity.x == 0. && body.velocity.z == 0.);
    }
    fn medical_tick(app: &mut App) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
        app.world_mut()
            .run_system_once(crate::survivor_items::update)
            .unwrap();
    }
    fn bot(position: Vec3, w: &Weapon) -> Companion {
        Companion {
            state: Match::new(w),
            position,
            spawn: position,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_Z,
        }
    }
    #[test]
    fn pinned_bots_keep_the_specials_origin_and_facing_until_release() {
        use bevy::ecs::system::RunSystemOnce;
        for class in crate::special_infected::SpecialClass::MINIONS {
            for incap in [false, true] {
                let (mut app, _) = game_app();
                app.insert_resource(Collision::test_solid_floor());
                let position = Vec3::new(40., 0.1, 0.);
                let facing = Quat::from_rotation_y(0.73);
                let mut member = bot(position, &pistol());
                member.state.pinned = true;
                member.state.pinned_by = Some(class);
                member.state.incap = incap;
                member.vertical = -4.;
                member.revive_time = 4.9;
                member.state.stagger = 0.5;
                member.state.stagger_dir = Vec3::X;
                crate::special_infected::boomer::advance_survivor(
                    &mut member, &Collision::test_solid_floor(), 1. / 30.,
                );
                assert_eq!(member.position, position);
                app.insert_resource(Squad(vec![member]));
                let actor = app.world_mut().spawn((CompanionActor(0), Transform::from_translation(position).with_rotation(facing))).id();
                let system = app.world_mut().register_system(squad_ai);
                for elapsed in [0., 2.1, 4.2] {
                    app.world_mut().resource_mut::<Match>().elapsed = elapsed;
                    app.world_mut().run_system(system).unwrap();
                    let m = &app.world().resource::<Squad>().0[0];
                    assert_eq!(m.position, position, "{class:?}/{incap}");
                    assert_eq!(m.vertical, 0.);
                    assert_eq!(m.revive_time, 0.);
                    assert!(m.state.pinned);
                    let transform = app.world().get::<Transform>(actor).unwrap();
                    assert_eq!(transform.translation, position);
                    assert_eq!(transform.rotation, facing);
                }
                if !incap {
                    let mut squad = app.world_mut().resource_mut::<Squad>();
                    squad.0[0].state.pinned = false;
                    squad.0[0].state.pinned_by = None;
                    drop(squad);
                    app.world_mut().run_system(system).unwrap();
                    assert_ne!(app.world().resource::<Squad>().0[0].position, position);
                }
            }
        }
    }
    #[test]
    fn healthy_far_bots_recover_together_without_moving_downed_bots() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _) = game_app();
        app.insert_resource(Collision::test_solid_floor());
        let w = pistol();
        let mut members = vec![
            bot(Vec3::X * 40., &w),
            bot(Vec3::X * 42., &w),
            bot(Vec3::X * 44., &w),
        ];
        members[0].state.health = 63.;
        members[0].state.pistol.ammo = 7;
        members[2].state.incap = true;
        app.insert_resource(Squad(members));
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Body>();
            q.single_mut(world).unwrap().grounded = true;
        }
        // Reuse the system instance: Local recovery timers must survive ticks.
        let system = app.world_mut().register_system(squad_ai);
        app.world_mut().run_system(system).unwrap();
        app.world_mut().resource_mut::<Match>().elapsed = 2.1;
        app.world_mut().run_system(system).unwrap();
        let members = &app.world().resource::<Squad>().0;
        assert!(members[0].position.length() < 4. && members[1].position.length() < 4.);
        assert!(members[0].position.distance(members[1].position) >= 0.85);
        assert_eq!(members[0].state.health, 63.);
        assert_eq!(members[0].state.pistol.ammo, 7);
        assert_eq!(members[2].position, Vec3::X * 44.);
    }
    #[test]
    fn adrenaline_is_timed_consumed_once_and_keeps_its_use_model() {
        let (mut app, _) = game_app();
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.health = 50.;
            s.adrenaline = true;
            s.slot = 5;
        }
        app.world_mut().resource_mut::<Input>().fire = true;
        medical_tick(&mut app);
        assert_eq!(
            app.world().resource::<Match>().held_class(),
            "weapon_adrenaline"
        );
        assert!(app.world().resource::<Match>().action > 0.);
        for _ in 0..92 {
            medical_tick(&mut app);
        }
        let s = app.world().resource::<Match>();
        assert!(!s.adrenaline);
        assert_eq!(s.temporary, 25.);
        assert!(s.adrenaline_time > 14.9);
        assert_eq!(s.slot, 2);
        for _ in 0..100 {
            medical_tick(&mut app);
        }
        assert_eq!(app.world().resource::<Match>().temporary, 25.);
    }
    #[test]
    fn cancelled_healing_retains_the_kit_then_clears_third_strike_on_completion() {
        let (mut app, _) = game_app();
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.health = 25.;
            s.medkit = true;
            s.black_white = true;
            s.revives = 2;
            s.slot = 4;
        }
        app.world_mut().resource_mut::<Input>().fire_held = true;
        for _ in 0..30 {
            medical_tick(&mut app);
        }
        app.world_mut().resource_mut::<Input>().fire_held = false;
        medical_tick(&mut app);
        assert!(app.world().resource::<Match>().medkit);
        assert_eq!(app.world().resource::<Match>().health, 25.);
        app.world_mut().resource_mut::<Input>().fire_held = true;
        for _ in 0..302 {
            medical_tick(&mut app);
        }
        let s = app.world().resource::<Match>();
        assert_eq!(s.health, 85.);
        assert!(!s.medkit);
        assert!(!s.black_white);
        assert_eq!(s.revives, 0);
    }
    #[test]
    fn medkit_can_heal_a_teammate_without_consuming_their_inventory() {
        let (mut app, _) = game_app();
        let w = pistol();
        let mut teammate = bot(Vec3::new(0., 0., -1.), &w);
        teammate.state.health = 20.;
        teammate.state.medkit = true;
        app.world_mut().resource_mut::<Squad>().0.push(teammate);
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.medkit = true;
            s.slot = 4;
        }
        {
            let mut input = app.world_mut().resource_mut::<Input>();
            input.shove = true;
            input.shove_held = true;
        }
        for _ in 0..302 {
            medical_tick(&mut app);
        }
        assert_eq!(app.world().resource::<Squad>().0[0].state.health, 84.);
        assert!(app.world().resource::<Squad>().0[0].state.medkit);
        assert!(!app.world().resource::<Match>().medkit);
    }
    #[test]
    fn defibrillator_requires_a_dead_target_and_continuous_use() {
        let (mut app, _) = game_app();
        let mut teammate = bot(Vec3::new(0., 0., -1.2), &pistol());
        teammate.state.dead = true;
        teammate.state.health = 0.;
        app.world_mut().resource_mut::<Squad>().0.push(teammate);
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.defibrillator = true;
            s.slot = 4;
        }
        app.world_mut().resource_mut::<Input>().fire_held = true;
        for _ in 0..60 {
            medical_tick(&mut app);
        }
        app.world_mut().resource_mut::<Input>().fire_held = false;
        medical_tick(&mut app);
        assert!(app.world().resource::<Squad>().0[0].state.dead);
        assert!(app.world().resource::<Match>().defibrillator);
        app.world_mut().resource_mut::<Input>().fire_held = true;
        for _ in 0..182 {
            medical_tick(&mut app);
        }
        let b = &app.world().resource::<Squad>().0[0].state;
        assert!(!b.dead);
        assert_eq!(b.health, 50.);
        assert!(!app.world().resource::<Match>().defibrillator);
    }
    #[test]
    fn bot_can_defibrillate_the_dead_player() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let mut b = bot(Vec3::new(0., 0., -1.), &pistol());
        b.state.defibrillator = true;
        app.world_mut().resource_mut::<Squad>().0.push(b);
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.dead = true;
            s.health = 0.;
        }
        for _ in 0..182 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            app.world_mut()
                .run_system_once(crate::survivor_items::bot_care)
                .unwrap();
        }
        assert!(!app.world().resource::<Match>().dead);
        assert_eq!(app.world().resource::<Match>().health, 50.);
        assert!(!app.world().resource::<Squad>().0[0].state.defibrillator);
    }
    #[test]
    fn shotgun_fires_authored_pellets_for_one_shell() {
        let (mut app, enemy) = game_app();
        let mut weapon = pistol();
        weapon.class = "weapon_pumpshotgun".into();
        weapon.pellets = 10;
        weapon.clip = 8;
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(weapon.class.clone(), weapon.clone());
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.primary = Some(Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: weapon.class,
                ammo: 8,
                reserve: 72,
            });
            s.slot = 1;
        }
        app.world_mut().get_mut::<Enemy>(enemy).unwrap().health = 500.;
        app.world_mut().resource_mut::<Input>().fire = true;
        tick(&mut app);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 140.);
        assert_eq!(app.world().resource::<Match>().gun().ammo, 7);
    }
    #[test]
    fn desert_rifle_finishes_one_burst_after_trigger_release() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let mut w = pistol();
        w.class = "weapon_rifle_desert".into();
        w.clip = 30;
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(w.class.clone(), w.clone());
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.primary = Some(Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: w.class,
                ammo: 30,
                reserve: 360,
            });
            s.slot = 1;
        }
        app.world_mut().resource_mut::<Input>().fire = true;
        for _ in 0..60 {
            tick(&mut app);
        }
        assert_eq!(app.world().resource::<Match>().gun().ammo, 27);
        assert_eq!(app.world().resource::<Match>().burst_left, 0);
    }
    #[test]
    fn finite_source_pickups_remain_visible_until_the_last_pickup() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let pickup = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_first_aid_kit".into(),
                    origin: Vec3::new(0., 0., -1.2),
                },
                crate::loot::stock(2),
                Visibility::Inherited,
            ))
            .id();
        for round in 0..2 {
            {
                let mut s = app.world_mut().resource_mut::<Match>();
                s.medkit = false;
            }
            app.world_mut().resource_mut::<Input>().use_item = true;
            tick(&mut app);
            assert!(app.world().resource::<Match>().medkit);
            assert_eq!(app.world().get::<Taken>(pickup).is_some(), round == 1);
        }
    }
    #[test]
    fn swapping_medical_equipment_creates_a_collectible_drop() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.world_mut().resource_mut::<Match>().medkit = true;
        app.world_mut().spawn((
            Pickup {
                class: "weapon_defibrillator".into(),
                origin: Vec3::new(0., 0., -1.2),
            },
            crate::loot::stock(1),
        ));
        app.world_mut().resource_mut::<Input>().use_item = true;
        tick(&mut app);
        assert!(app.world().resource::<Match>().defibrillator);
        assert!(!app.world().resource::<Match>().medkit);
        let drops = app
            .world_mut()
            .query_filtered::<&Pickup, With<crate::loot::Dropped>>()
            .iter(app.world())
            .map(|p| p.class.clone())
            .collect::<Vec<_>>();
        assert_eq!(drops, vec!["weapon_first_aid_kit"]);
    }
    #[test]
    fn bots_collect_medical_supplies_and_can_use_them() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let mut b = bot(Vec3::new(0., 0., -1.), &pistol());
        b.state.health = 20.;
        app.world_mut().resource_mut::<Squad>().0.push(b);
        let pickup = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_adrenaline".into(),
                    origin: Vec3::new(0., 0., -1.1),
                },
                crate::loot::stock(1),
            ))
            .id();
        app.world_mut()
            .run_system_once(crate::loot::bot_pickups)
            .unwrap();
        assert!(app.world().get::<Taken>(pickup).is_some());
        assert!(app.world().resource::<Squad>().0[0].state.adrenaline);
        for _ in 0..94 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            app.world_mut()
                .run_system_once(crate::survivor_items::bot_care)
                .unwrap();
        }
        let b = &app.world().resource::<Squad>().0[0].state;
        assert!(!b.adrenaline);
        assert_eq!(b.temporary, 25.);
        assert_eq!(b.adrenaline_time, 15.);
    }
    #[test]
    fn zombie_hits_queue_blood_and_each_new_hit_restarts_reaction() {
        let (mut app, enemy) = game_app();
        app.init_resource::<crate::bullet_impacts::Impacts>();
        app.world_mut().get_mut::<Enemy>(enemy).unwrap().health = 200.;
        app.world_mut().resource_mut::<Input>().fire = true;
        tick(&mut app);
        assert_eq!(
            app.world()
                .resource::<crate::bullet_impacts::Impacts>()
                .blood
                .len(),
            1
        );
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().reaction_serial, 1);
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.cooldown = 0.;
        }
        app.world_mut().resource_mut::<Input>().fire = true;
        tick(&mut app);
        assert_eq!(
            app.world()
                .resource::<crate::bullet_impacts::Impacts>()
                .blood
                .len(),
            2
        );
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().reaction_serial, 2);
        // A common keeps going: its flinch is a gesture (retail AddGesture),
        // not a pause.
        let e = app.world().get::<Enemy>(enemy).unwrap();
        if e.model.contains("/common_") {
            assert_eq!(e.reaction, 0);
        } else {
            assert!(e.attack > 0.1);
        }
    }
    #[test]
    fn shotgun_groups_native_blood_burst_for_multiple_pellets_on_one_actor() {
        let (mut app, enemy) = game_app();
        app.init_resource::<crate::bullet_impacts::Impacts>();
        app.world_mut().get_mut::<Enemy>(enemy).unwrap().health = 1000.;
        let mut w = pistol();
        w.pellets = 10;
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(w.class.clone(), w);
        app.world_mut().resource_mut::<Input>().fire = true;
        tick(&mut app);
        let hits = &app
            .world()
            .resource::<crate::bullet_impacts::Impacts>()
            .blood;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].shotgun);
    }
    fn tick(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
        app.world_mut().run_schedule(FixedUpdate);
    }
    #[test]
    fn native_game_schedule_shoots_and_reloads_infinite_pistol() {
        let (mut app, enemy) = game_app();
        app.world_mut().resource_mut::<Input>().fire = true;
        tick(&mut app);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 14.);
        assert_eq!(app.world().resource::<Match>().pistol.ammo, 14);
        assert_eq!(
            app.world()
                .resource::<game_audio::Queue>()
                .0
                .front()
                .unwrap()
                .name,
            "Pistol.Fire"
        );
        app.world_mut().resource_mut::<Match>().pistol.ammo = 0;
        app.world_mut().resource_mut::<Input>().reload = true;
        for _ in 0..110 {
            tick(&mut app);
        }
        assert_eq!(
            app.world().resource::<Match>().pistol.ammo,
            0,
            "An empty pistol must not refill before the measured 2.1-second reload"
        );
        for _ in 0..20 {
            tick(&mut app);
        }
        assert_eq!(app.world().resource::<Match>().pistol.ammo, 15);
    }
    #[test]
    fn shove_staggers_and_pushes_without_spending_ammo_and_respects_cooldown() {
        let (mut app, enemy) = game_app();
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = Vec3::NEG_Z * 1.1;
        app.world_mut().resource_mut::<Input>().shove = true;
        tick(&mut app);
        assert_eq!(app.world().resource::<Match>().pistol.ammo, 15);
        assert_eq!(app.world().resource::<Match>().shove_serial, 1);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().reaction, 1);
        assert!(app.world().get::<Transform>(enemy).unwrap().translation.z < -1.4);
        app.world_mut().resource_mut::<Input>().shove = true;
        tick(&mut app);
        assert_eq!(app.world().resource::<Match>().shove_serial, 1);
    }
    #[test]
    fn throwable_is_consumed_once_at_release_and_returns_to_sidearm() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _) = game_app();
        let mut weapon = pistol();
        weapon.class = "weapon_pipe_bomb".into();
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(weapon.class.clone(), weapon);
        app.insert_resource(crate::l4d2r_animation::AnimationLibrary::default())
            .init_resource::<crate::projectiles::Projectiles>();
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.throwable = Some("weapon_pipe_bomb".into());
            s.slot = 3;
        }
        app.world_mut().resource_mut::<Input>().throw_held = true;
        for _ in 0..40 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f32(1. / 60.));
            app.world_mut()
                .run_system_once(crate::projectiles::controls)
                .unwrap();
        }
        assert_eq!(app.world().resource::<Match>().throw_stage, 2);
        app.world_mut().resource_mut::<Input>().throw_held = false;
        app.world_mut()
            .run_system_once(crate::projectiles::controls)
            .unwrap();
        assert!(app.world().resource::<Match>().throwable.is_none());
        for _ in 0..40 {
            app.world_mut()
                .run_system_once(crate::projectiles::controls)
                .unwrap();
        }
        assert_eq!(
            app.world()
                .resource::<crate::projectiles::Projectiles>()
                .thrown,
            1
        );
        assert_eq!(app.world().resource::<Match>().slot, 2);
    }
    #[test]
    fn use_key_picks_up_each_throwable_and_does_not_overwrite_an_owned_one() {
        for class in ["weapon_molotov", "weapon_pipe_bomb", "weapon_vomitjar"] {
            let (mut app, _) = game_app();
            let entity = app
                .world_mut()
                .spawn(Pickup {
                    class: class.into(),
                    origin: Vec3::new(0., 1.2, -0.8),
                })
                .id();
            app.world_mut().resource_mut::<Input>().use_item = true;
            tick(&mut app);
            assert_eq!(
                app.world().resource::<Match>().throwable.as_deref(),
                Some(class)
            );
            assert_eq!(app.world().resource::<Match>().slot, 3);
            assert!(app.world().get::<Taken>(entity).is_some());
        }
    }
    fn test_slash() -> crate::game_state::MeleeAttack {
        crate::game_state::MeleeAttack {
            start: 2,
            end: 3,
            duration: 1.23,
            start_time: 0.1,
            end_time: 0.33,
            activity: "ACT_VM_PRIMARYATTACK".into(),
        }
    }
    #[test]
    fn melee_world_hits_play_impact_but_misses_do_not() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let mut axe = pistol();
        axe.class = "weapon_melee_fireaxe".into();
        axe.range = 75. * UNIT;
        axe.melee_world_sound = "Axe.ImpactWorld".into();
        axe.melee_hit_sound = "Axe.ImpactFlesh".into();
        axe.melee_attacks = vec![test_slash()];
        app.world_mut().resource_mut::<Match>().pistol.class = axe.class.clone();
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(axe.class.clone(), axe);
        for wall in [false, true] {
            if wall {
                app.world_mut()
                    .resource_mut::<Collision>()
                    .test_add_box(Vec3::new(-2., -100., -1.2), Vec3::new(2., 100., -1.));
            }
            app.world_mut()
                .resource_mut::<game_audio::Queue>()
                .0
                .clear();
            app.world_mut().resource_mut::<Match>().cooldown = 0.;
            app.world_mut().resource_mut::<Input>().fire = true;
            // The swing sweeps between 0.1 and 0.33 s after the click.
            for _ in 0..30 {
                tick(&mut app);
            }
            let queue = &app.world().resource::<game_audio::Queue>().0;
            assert_eq!(
                queue.iter().filter(|r| r.name == "Axe.ImpactWorld").count(),
                usize::from(wall)
            );
            assert!(!queue.iter().any(|r| r.name == "Axe.ImpactFlesh"));
        }
    }
    #[test]
    fn melee_pickup_swaps_secondary_and_hits_without_ammunition() {
        let (mut app, enemy) = game_app();
        let mut axe = pistol();
        axe.class = "weapon_melee_fireaxe".into();
        axe.damage = 70.;
        axe.range = 75. * UNIT;
        axe.punch = 0.;
        axe.fire_sound = "Axe.Miss".into();
        axe.melee_hit_sound = "Axe.ImpactFlesh".into();
        axe.melee_world_sound = "Axe.ImpactWorld".into();
        axe.melee_attacks = vec![test_slash()];
        app.init_resource::<game_audio::Queue>();
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(axe.class.clone(), axe);
        app.world_mut().spawn((
            Pickup {
                class: "weapon_melee_fireaxe".into(),
                origin: Vec3::Y - Vec3::Z * 0.5,
            },
            Transform::from_translation(Vec3::Y - Vec3::Z * 0.5),
        ));
        let model = app
            .world_mut()
            .spawn((
                Name::new("weapon_melee_spawn / axe"),
                Transform::from_translation(Vec3::Y - Vec3::Z * 0.5),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().resource_mut::<Input>().use_item = true;
        tick(&mut app);
        assert_eq!(
            app.world().resource::<Match>().pistol.class,
            "weapon_melee_fireaxe"
        );
        assert_eq!(
            *app.world().get::<Visibility>(model).unwrap(),
            Visibility::Hidden
        );
        app.world_mut().resource_mut::<Match>().pistol.ammo = 0;
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation
            .z = -1.;
        app.world_mut().resource_mut::<Input>().fire = true;
        for _ in 0..30 {
            tick(&mut app);
        }
        assert!(app.world().get::<Enemy>(enemy).unwrap().health <= 0.);
        assert_eq!(app.world().resource::<Match>().kills, 1);
        assert_eq!(app.world().resource::<Match>().pistol.ammo, 0);
        let queue = &app.world().resource::<game_audio::Queue>().0;
        assert_eq!(
            queue.iter().filter(|r| r.name == "Axe.ImpactFlesh").count(),
            1
        );
        assert!(!queue.iter().any(|r| r.name == "Axe.ImpactWorld"));
    }
    #[test]
    fn native_game_pause_freezes_damage_movement_and_weapon_timers() {
        let (mut app, enemy) = game_app();
        {
            let mut state = app.world_mut().resource_mut::<Match>();
            state.paused = true;
            state.reload = 1.;
        }
        app.world_mut().resource_mut::<Input>().fire = true;
        for _ in 0..60 {
            tick(&mut app);
        }
        assert_eq!(app.world().resource::<Match>().reload, 1.);
        assert_eq!(app.world().resource::<Match>().health, 100.);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 50.);
    }
    #[test]
    fn native_game_pickup_updates_inventory_and_hides_placed_model() {
        let (mut app, _) = game_app();
        let pickup = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_first_aid_kit".into(),
                    origin: Vec3::Y - Vec3::Z * 0.5,
                },
                Transform::from_translation(Vec3::Y - Vec3::Z * 0.5),
            ))
            .id();
        let model = app
            .world_mut()
            .spawn((
                Name::new("weapon_first_aid_kit_spawn / model"),
                Transform::from_translation(Vec3::Y - Vec3::Z * 0.5),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut().resource_mut::<Input>().use_item = true;
        tick(&mut app);
        assert!(app.world().resource::<Match>().medkit);
        assert!(app.world().get::<Taken>(pickup).is_some());
        assert_eq!(
            *app.world().get::<Visibility>(model).unwrap(),
            Visibility::Hidden
        );
    }
    #[test]
    fn squad_uses_live_state_and_attacks_visible_infected() {
        let (mut app, enemy) = game_app();
        let member = Companion {
            state: Match::new(&pistol()),
            position: Vec3::X,
            spawn: Vec3::X,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_Z,
        };
        app.world_mut().resource_mut::<Squad>().0.push(member);
        settled_bot_shot(&mut app, 0);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 14.);
        assert!(app.world().resource::<Squad>().0[0].cooldown > 0.);
    }
    fn staged_bot(app: &mut App, position: Vec3) {
        app.world_mut().resource_mut::<Squad>().0.push(Companion {
            state: Match::new(&pistol()),
            position,
            spawn: position,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_Z,
        });
    }
    // Ballistic fixtures wait for actual perception/aim acquisition, then
    // stop on the first round so their damage/ammo assertions stay exact.
    fn settled_bot_shot(app: &mut App, index: usize) {
        app.insert_resource(Collision::test_solid_floor());
        let serial = app.world().resource::<Squad>().0[index].state.shot_serial;
        let system = app.world_mut().register_system(squad_ai);
        for _ in 0..60 {
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            app.world_mut().run_system(system).unwrap();
            if app.world().resource::<Squad>().0[index].state.shot_serial != serial { return; }
        }
        panic!("bot did not acquire a clear target and shoot");
    }
    #[test]
    fn survivor_holds_fire_for_player_then_resumes_when_lane_clears() {
        let (mut app, enemy) = game_app();
        staged_bot(&mut app, Vec3::Z * 2.);
        tick(&mut app);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 50.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.pistol.ammo, 15);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 0);
        let world = app.world_mut();
        let mut query = world.query::<&mut Body>();
        query.single_mut(world).unwrap().feet = Vec3::X * 4.;
        settled_bot_shot(&mut app, 0);
        assert!(app.world().get::<Enemy>(enemy).unwrap().health < 50.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.pistol.ammo, 14);
    }
    #[test]
    fn survivor_holds_fire_for_teammate_in_the_shot_lane() {
        let (mut app, enemy) = game_app();
        app.world_mut()
            .get_mut::<Transform>(enemy)
            .unwrap()
            .translation = Vec3::new(1., 0., -6.);
        staged_bot(&mut app, Vec3::X);
        staged_bot(&mut app, Vec3::new(1., 0., -2.));
        app.world_mut().resource_mut::<Squad>().0[1].cooldown = 10.;
        tick(&mut app);
        assert_eq!(app.world().get::<Enemy>(enemy).unwrap().health, 50.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.pistol.ammo, 15);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 0);
        app.world_mut().resource_mut::<Squad>().0[1].position.x = 4.;
        settled_bot_shot(&mut app, 0);
        assert!(app.world().get::<Enemy>(enemy).unwrap().health < 50.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.pistol.ammo, 14);
    }
    #[test]
    fn survivor_shotgun_hits_multiple_infected_for_one_shell() {
        let (mut app, target) = game_app();
        let extra = {
            let mut enemy = Enemy {
                model: "models/infected/common_male01.mdl".into(),
                death_loot: None,
                loot_dropped: false,
                reaction_serial: 0,
                attack_serial: 0,
                health: 50.,
                attack: 0.,
                vertical: 0.,
                reaction: 0,
                corpse_time: 0.,
                death: Default::default(),
                movement_speed: 0.,
                burning: 0.,
            };
            enemy.health = 1000.;
            app.world_mut()
                .spawn((enemy, Transform::from_xyz(2., 0., -6.)))
                .id()
        };
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(1., 0., -6.);
        app.world_mut().get_mut::<Enemy>(target).unwrap().health = 1000.;
        let mut weapon = pistol();
        weapon.class = "weapon_pumpshotgun".into();
        weapon.pellets = 48;
        weapon.scatter = Vec2::new(0., 12.);
        weapon.damage = 3.;
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .insert(weapon.class.clone(), weapon.clone());
        staged_bot(&mut app, Vec3::X);
        {
            let mut squad = app.world_mut().resource_mut::<Squad>();
            squad.0[0].state.primary = Some(Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: weapon.class,
                ammo: 8,
                reserve: 32,
            });
            squad.0[0].state.slot = 1;
        }
        settled_bot_shot(&mut app, 0);
        assert!(app.world().get::<Enemy>(target).unwrap().health < 1000.);
        assert!(
            app.world().get::<Enemy>(extra).unwrap().health < 1000.,
            "spread rays must hit other actors, not only the aim target"
        );
        assert_eq!(app.world().resource::<Squad>().0[0].state.gun().ammo, 7);
    }
    #[test]
    fn survivor_pellets_stop_at_first_actor_even_when_shot_is_lethal() {
        let (mut app, target) = game_app();
        let extra = app
            .world_mut()
            .spawn((
                Enemy {
                    model: "models/infected/common_male01.mdl".into(),
                    death_loot: None,
                    loot_dropped: false,
                    reaction_serial: 0,
                    attack_serial: 0,
                    health: 50.,
                    attack: 0.,
                    vertical: 0.,
                    reaction: 0,
                    corpse_time: 0.,
                    death: Default::default(),
                    movement_speed: 0.,
                    burning: 0.,
                },
                Transform::from_xyz(1., 0., -6.),
            ))
            .id();
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(1., 0., -3.);
        staged_bot(&mut app, Vec3::X);
        app.world_mut().resource_mut::<Rules>().pistol.pellets = 10;
        app.world_mut()
            .resource_mut::<Rules>()
            .weapons
            .get_mut("weapon_pistol")
            .unwrap()
            .pellets = 10;
        settled_bot_shot(&mut app, 0);
        assert!(app.world().get::<Enemy>(target).unwrap().health <= 0.);
        assert_eq!(app.world().get::<Enemy>(extra).unwrap().health, 50.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.kills, 1);
    }
    #[test]
    fn common_infected_chase_uses_retail_run_speed() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, entity) = game_app();
        app.insert_resource(Collision::test_solid_floor());
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f32(1. / 30.));
        let system = app.world_mut().register_system(enemy_ai);
        let mut step = || {
            let before = app.world().get::<Transform>(entity).unwrap().translation;
            app.world_mut().run_system(system).unwrap();
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f32(1. / 30.));
            let after = app.world().get::<Transform>(entity).unwrap().translation;
            (after - before).xz().length() / (1. / 30.) / UNIT
        };
        // From rest the common runs up at the recorded ~470 units/s^2.
        let speeds: Vec<f32> = (0..20).map(|_| step()).collect();
        assert!((speeds[0] - 470. / 30.).abs() < 0.5, "{speeds:?}");
        // Then, within about half a second, z_speed (250) x the common's own
        // RandomFloat(0.9, 1.1) factor.
        let expected = crate::infected_movement::run_speed(entity.to_bits()) / UNIT;
        let top = speeds.iter().position(|v| (v - expected).abs() < 0.01);
        assert!(top.is_some_and(|i| (15..=18).contains(&i)), "{speeds:?} vs {expected}");
        assert!((225. ..=275.).contains(&expected));
    }
    #[test]
    fn common_infected_chase_through_hotel_stairs_with_native_navigation() {
        let path = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() { return; }
        for (from, to) in [([248., 5660., 2848.03125], [248., 5910., 2752.03125]), ([248., 5910., 2752.03125], [248., 5660., 2848.03125])] {
            let (mut app, infected) = game_app();
            let bsp = l4d2_formats::bsp::Bsp::parse(std::fs::read(path).unwrap()).unwrap();
            let collision = Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(l4d2r_game_core::trace::Tracer::new(std::sync::Arc::new(bsp)))));
            app.insert_resource(collision);
            app.insert_resource(crate::l4d2r_navigation::Navigation::load(path));
            let start = Vec3::from_array(binary::source_position(from));
            let target = Vec3::from_array(binary::source_position(to));
            assert!(app.world().resource::<Collision>().clear_hull(start, HEIGHT), "invalid start {from:?}");
            assert!(app.world().resource::<Collision>().clear_hull(target, HEIGHT), "invalid goal {to:?}");
            app.world_mut().get_mut::<Transform>(infected).unwrap().translation = start;
            app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet = target;
            app.world_mut().resource_mut::<Match>().health = 1000.;
            let system = app.world_mut().register_system(enemy_ai);
            for _ in 0..150 {
                app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut().resource_mut::<Match>().elapsed += 1. / 30.;
                app.world_mut().run_system(system).unwrap();
            }
            let end = app.world().get::<Transform>(infected).unwrap().translation;
            assert!((end - target).xz().length() < 1.5, "stair chase stalled {from:?}->{to:?}: {end:?}, {}", app.world().resource::<crate::l4d2r_navigation::Navigation>().debug_infected(infected.to_bits()));
            assert!((end.y - target.y).abs() < 18. * UNIT, "wrong stair level: {end:?}");
        }
    }
    #[test]
    fn hotel_stair_rail_corner_routes() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() { return; }
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let nav = l4d2_formats::nav::parse_nav(&std::fs::read(path.with_extension("nav")).unwrap()).unwrap();
        for level in [2752., 2656.] {
            let target = nav.areas.iter().filter_map(|area| {
                let (lo, hi) = (area.min_xy(), area.max_xy());
                let x = (lo[0] + hi[0]) * 0.5;
                let y = (lo[1] + hi[1]) * 0.5;
                let z = area.z_at(x, y);
                if (z - level).abs() > 8. || !(0. ..600.).contains(&x) || !(5500. ..6100.).contains(&y) { return None; }
                let p = Vec3::from_array(binary::source_position([x, y, z + 0.03125]));
                (collision.clear_hull(p, HEIGHT) && collision.grounded(p, HEIGHT))
                    .then_some(((x - 248.).powi(2) + (y - 5660.).powi(2), p))
            }).min_by(|a, b| a.0.total_cmp(&b.0)).expect("missing supported stair landing").1;
            for reverse in [false, true] {
            let top = Vec3::from_array(binary::source_position([248., 5660., 2848.03125]));
            let (start, target) = if reverse { (target, top) } else { (top, target) };
            let (mut app, entity) = game_app();
            app.insert_resource(collision.clone());
            app.insert_resource(crate::l4d2r_navigation::Navigation::load(&path));
            app.world_mut().get_mut::<Transform>(entity).unwrap().translation = start;
            let mut group = vec![entity];
            for i in 1..8 {
                let offset = Vec3::new((i % 3) as f32 - 1., 0., (i / 3) as f32) * 0.25;
                let point = if collision.clear_hull(start + offset, HEIGHT) { start + offset } else { start };
                group.push(app.world_mut().spawn((Transform::from_translation(point), Enemy {
                    model: "models/infected/common_male01.mdl".into(), death_loot: None,
                    loot_dropped: false, death: Default::default(), movement_speed: 0.,
                    reaction_serial: 0, attack_serial: 0, health: 50., attack: 0., reaction: 0,
                    corpse_time: 0., burning: 0., vertical: 0.,
                })).id());
            }
            app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet = target;
            app.world_mut().resource_mut::<Match>().health = 10000.;
            let system = app.world_mut().register_system(enemy_ai);
            for _ in 0..600 {
                app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 30.));
                app.world_mut().run_system(system).unwrap();
            }
            for entity in group {
                let end = app.world().get::<Transform>(entity).unwrap().translation;
                println!("Hotel rail level={level} reverse={reverse} target={target:?} end={end:?}");
                assert!(end.distance(target) < 2., "rail corner stalled level={level} end={end:?} target={target:?} {}", app.world().resource::<crate::l4d2r_navigation::Navigation>().debug_infected(entity.to_bits()));
            }
            }
        }
    }
    #[test]
    fn reacting_common_infected_keep_falling_without_chasing_or_attacking() {
        use bevy::ecs::system::RunSystemOnce;
        for reaction in [1, 2, 3] {
            let (mut app, entity) = game_app();
            app.insert_resource(Collision::test_solid_floor());
            app.world_mut().get_mut::<Transform>(entity).unwrap().translation =
                Vec3::new(0., 3., -4.);
            {
                let mut enemy = app.world_mut().get_mut::<Enemy>(entity).unwrap();
                enemy.reaction = reaction;
                enemy.attack = 2.;
                enemy.movement_speed = 4.;
            }
            app.world_mut().resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            for _ in 0..15 {
                app.world_mut().run_system_once(enemy_ai).unwrap();
            }
            let position = app.world().get::<Transform>(entity).unwrap().translation;
            let enemy = app.world().get::<Enemy>(entity).unwrap();
            assert!(position.y < 1., "reaction {reaction} suspended at {position:?}");
            assert_eq!(position.xz(), Vec2::new(0., -4.));
            assert_eq!(enemy.reaction, reaction);
            assert!(enemy.attack > 1.4 && enemy.attack < 1.6);
            assert_eq!(enemy.attack_serial, 0);
            assert_eq!(enemy.movement_speed, 0.);
            assert_eq!(app.world().resource::<Match>().health, 100.);
        }
    }
    #[test]
    fn common_infected_burn_timer_expires_during_an_active_climb() {
        let (mut app, entity) = game_app();
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(0.6, 0., -2.), Vec3::new(1.4, 1.2, 2.));
        app.insert_resource(collision);
        app.world_mut().get_mut::<Transform>(entity).unwrap().translation = Vec3::Y * 0.004;
        app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet = Vec3::X * 4.;
        app.world_mut().resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        let system = app.world_mut().register_system(enemy_ai);
        for _ in 0..15 {
            app.world_mut().run_system(system).unwrap();
            if app.world().get::<Enemy>(entity).unwrap().reaction >= 4 { break; }
        }
        assert!(app.world().get::<Enemy>(entity).unwrap().reaction >= 4, "fixture did not start climbing");
        app.world_mut().get_mut::<Enemy>(entity).unwrap().burning = 0.1;
        for _ in 0..6 { app.world_mut().run_system(system).unwrap(); }
        let enemy = app.world().get::<Enemy>(entity).unwrap();
        assert!(enemy.reaction >= 4 && enemy.attack > 0., "fixture must still be climbing");
        assert_eq!(enemy.burning, 0., "climbing must not extend the burn timer");
    }
    #[test]
    fn ai_specials_keep_falling_when_idle_close_to_a_target_or_staggering() {
        use crate::special_infected::{SpecialClass, ai::{AiSpecial, AiActivity, Mode}};
        for case in 0..3 {
            let (mut app, entity) = game_app();
            app.insert_resource(Collision::test_solid_floor());
            app.world_mut().get_mut::<Transform>(entity).unwrap().translation = Vec3::Y * 3.;
            app.world_mut().resource_mut::<Match>().infected = case != 1;
            let mut ai = AiSpecial::new(SpecialClass::Smoker);
            ai.ability = 30.;
            if case == 2 { ai.mode = Mode::Stagger(2.); }
            app.world_mut().entity_mut(entity).insert((ai, AiActivity("ACT_IDLE", None)));
            app.world_mut().resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            let system = app.world_mut().register_system(crate::special_infected::ai::tick);
            for _ in 0..30 { app.world_mut().run_system(system).unwrap(); }
            let position = app.world().get::<Transform>(entity).unwrap().translation;
            assert!(position.y < 0.02 && position.y >= -0.01, "case {case} suspended or fell through floor: {position:?}");
            assert_eq!(position.xz(), Vec2::ZERO);
            assert_eq!(app.world().get::<Enemy>(entity).unwrap().movement_speed, 0.);
            if case != 1 {
                assert_eq!(app.world().resource::<Match>().health, 100.);
            } else {
                // Once on the floor, the nearby survivor remains a valid
                // claw target; falling must not disable ordinary combat.
                assert!(app.world().resource::<Match>().health < 100.);
            }
        }
    }
    #[test]
    fn bots_do_not_track_occluded_infected_and_reacquire_after_ghost_mode() {
        let (mut app, _) = game_app();
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(4.5, 0., -5.), Vec3::new(5.5, 3., 5.));
        app.insert_resource(collision);
        let mut member = bot(Vec3::X * 10., &pistol());
        member.aim = Vec3::NEG_X;
        app.insert_resource(Squad(vec![member]));
        app.insert_resource(crate::special_infected::Hunter::default());
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.infected = true;
            s.special_class = crate::special_infected::SpecialClass::Smoker;
            s.health = 1000.;
        }
        let system = app.world_mut().register_system(crate::special_infected::tick);
        let step = |app: &mut App| {
            app.world_mut().resource_mut::<Time<Fixed>>().advance_by(std::time::Duration::from_secs_f64(1. / 30.));
            app.world_mut().run_system(system).unwrap();
        };
        // Moving behind cover must not feed live coordinates into the aim.
        app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet.z = -2.;
        for _ in 0..30 { step(&mut app); }
        assert_eq!(app.world().resource::<Match>().health, 1000.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 0);
        assert_eq!(app.world().resource::<Squad>().0[0].aim, Vec3::NEG_X);
        app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet = Vec3::ZERO;
        app.insert_resource(Collision::test_solid_floor());
        step(&mut app);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 0);
        for _ in 0..30 { step(&mut app); }
        assert!(app.world().resource::<Match>().health < 1000.);
        app.world_mut().resource_mut::<crate::special_infected::Hunter>().ghost = true;
        let health = app.world().resource::<Match>().health;
        let serial = app.world().resource::<Squad>().0[0].state.shot_serial;
        for _ in 0..30 { step(&mut app); }
        assert_eq!(app.world().resource::<Match>().health, health);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, serial);
        app.world_mut().resource_mut::<crate::special_infected::Hunter>().ghost = false;
        step(&mut app);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, serial);
    }
    #[test]
    fn sb_stop_freezes_survivor_bots_in_both_teams() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _) = game_app();
        let w = pistol();
        app.world_mut()
            .resource_mut::<Squad>()
            .0
            .push(bot(Vec3::NEG_Z * 10., &w));
        let step = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        };
        let flat = |p: Vec3| p * Vec3::new(1., 0., 1.);
        // Survivor team: a stopped bot holds position and takes no action.
        crate::game_state::set_bots_stopped(true);
        let ammo = app.world().resource::<Squad>().0[0].state.gun().ammo;
        for _ in 0..30 {
            step(&mut app);
            app.world_mut().run_system_once(squad_ai).unwrap();
        }
        let held = &app.world().resource::<Squad>().0[0];
        assert_eq!(flat(held.position), Vec3::NEG_Z * 10.);
        assert_eq!(held.state.shot_serial, 0);
        assert_eq!(held.state.gun().ammo, ammo);
        crate::game_state::set_bots_stopped(false);
        // Infected team: stopped bots do not shoot the spawned player.
        app.world_mut().resource_mut::<Squad>().0[0].position = Vec3::NEG_Z * 3.;
        app.insert_resource(crate::special_infected::Hunter::default());
        app.world_mut().resource_mut::<Match>().infected = true;
        app.world_mut()
            .run_system_once(crate::special_infected::switch_team)
            .unwrap();
        app.world_mut()
            .resource_mut::<crate::special_infected::Hunter>()
            .ghost = false;
        // Spawned (phase 5 ghosts are invulnerable).
        app.world_mut().resource_mut::<Match>().special_phase = 0;
        // Face the visible threat: this test exercises sb_stop rather than
        // detection outside the retail 90-degree visual cone.
        app.world_mut().resource_mut::<Squad>().0[0].aim = Vec3::Z;
        let opposition = app.world_mut().register_system(crate::special_infected::tick);
        crate::game_state::set_bots_stopped(true);
        for _ in 0..30 {
            step(&mut app);
            app.world_mut()
                .run_system(opposition)
                .unwrap();
        }
        assert_eq!(app.world().resource::<Match>().health, 250.);
        assert_eq!(app.world().resource::<Squad>().0[0].state.shot_serial, 0);
        crate::game_state::set_bots_stopped(false);
        for _ in 0..30 {
            step(&mut app);
            app.world_mut()
                .run_system(opposition)
                .unwrap();
        }
        assert!(app.world().resource::<Match>().health < 250.);
    }
    #[test]
    fn hunter_team_switch_restores_inventory_and_pounce_pins_a_survivor() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _) = game_app();
        app.insert_resource(crate::special_infected::Hunter::default());
        {
            let mut state = app.world_mut().resource_mut::<Match>();
            state.health = 63.;
            state.pistol.ammo = 7;
            state.infected = true;
        }
        app.world_mut()
            .run_system_once(crate::special_infected::switch_team)
            .unwrap();
        assert_eq!(app.world().resource::<Match>().health, 250.);
        assert_eq!(
            app.world().resource::<Match>().held_class(),
            crate::special_infected::HUNTER_WEAPON
        );
        app.world_mut().resource_mut::<Match>().infected = false;
        app.world_mut()
            .run_system_once(crate::special_infected::switch_team)
            .unwrap();
        assert_eq!(app.world().resource::<Match>().health, 63.);
        assert_eq!(app.world().resource::<Match>().pistol.ammo, 7);
        app.world_mut().resource_mut::<Match>().infected = true;
        let w = pistol();
        app.world_mut()
            .resource_mut::<Squad>()
            .0
            .push(bot(Vec3::NEG_Z, &w));
        app.world_mut()
            .resource_mut::<crate::special_infected::Hunter>()
            .test_start_pounce(Vec3::NEG_Z * 2.);
        app.world_mut()
            .run_system_once(crate::special_infected::tick)
            .unwrap();
        assert!(app.world().resource::<Squad>().0[0].state.pinned);
        assert_eq!(
            app.world().resource::<Squad>().0[0].state.health,
            100.,
            "retail pounce damage starts after its one-second delay"
        );
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f64(1. / 30.));
        for _ in 0..31 {
            app.world_mut()
                .run_system_once(crate::special_infected::tick)
                .unwrap();
        }
        assert_eq!(app.world().resource::<Squad>().0[0].state.health, 95.);
        assert_eq!(app.world().resource::<Match>().special_phase, 3);
    }
    #[test]
    fn hunter_pounce_keeps_upward_impulse_while_departing_the_floor() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(Collision::test_solid_floor());
        {
            let mut state = app.world_mut().resource_mut::<Match>();
            state.infected = true;
            state.special_phase = 2;
        }
        let dt = 1. / 30.;
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        {
            let world = app.world_mut();
            let mut q = world.query::<&mut Body>();
            let mut body = q.single_mut(world).unwrap();
            body.feet = Vec3::Y * 0.004;
            body.velocity = Vec3::new(0., 200. * UNIT, -600. * UNIT);
            body.grounded = false;
        }
        let mut peak: f32 = 0.;
        for _ in 0..18 {
            app.world_mut().run_system_once(move_player).unwrap();
            let world = app.world_mut();
            let mut q = world.query::<&Body>();
            let body = q.single(world).unwrap();
            peak = peak.max(body.feet.y);
        }
        assert!(peak > 20. * UNIT, "launch peak {peak}");
    }
    #[test]
    fn shift_walk_caps_ground_speed_and_release_restores_weapon_speed() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(Collision::test_floor());
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::KeyW);
            keys.press(KeyCode::ShiftLeft);
        }
        for _ in 0..60 {
            tick(&mut app);
        }
        let speed = |app: &mut App| {
            let world = app.world_mut();
            let mut q = world.query::<&Body>();
            let v = q.single(world).unwrap().velocity;
            Vec2::new(v.x, v.z).length() / UNIT
        };
        assert!((speed(&mut app) - 85.).abs() < 0.01);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ShiftLeft);
        for _ in 0..60 {
            tick(&mut app);
        }
        assert!((speed(&mut app) - 220.).abs() < 0.01);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ShiftLeft);
            keys.press(KeyCode::ControlLeft);
        }
        for _ in 0..60 {
            tick(&mut app);
        }
        assert!((speed(&mut app) - 55.).abs() < 0.01);
    }
    #[test]
    fn squad_follows_on_supported_ground_and_stops_near_player() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(Collision::test_floor());
        app.world_mut().resource_mut::<Squad>().0.push(Companion {
            state: Match::new(&pistol()),
            position: Vec3::X * 8.,
            spawn: Vec3::X * 8.,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            // Facing its path, as bot_look turns it in game.
            aim: Vec3::NEG_X,
        });
        for _ in 0..180 {
            tick(&mut app);
        }
        let position = app.world().resource::<Squad>().0[0].position;
        // Retail bots settle 170-215 units from the human (bots-1027).
        let units = position.x / UNIT;
        assert!((170. ..=215.).contains(&units), "{units} units");
        assert!(position.y.abs() < 0.01);
    }
    fn smoked_player_app(bot_at: Vec3, incap: bool) -> App {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(Collision::test_floor());
        {
            let mut s = app.world_mut().resource_mut::<Match>();
            s.pinned = true;
            s.pinned_by = Some(crate::special_infected::SpecialClass::Smoker);
            if incap {
                s.incap = true;
            }
        }
        app.world_mut().resource_mut::<Squad>().0.push(Companion {
            state: Match::new(&pistol()),
            position: bot_at,
            spawn: bot_at,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::X,
        });
        let _ = crate::special_infected::ai::take_shoved_victims();
        app
    }
    #[test]
    fn bots_shove_a_smoked_player_free_and_never_revive_a_held_player() {
        use crate::special_infected::ai::{take_shoved_victims, Victim};
        // Next to the smoked player: after the retail reaction time
        // (sb_friend_immobilized_reaction_time_normal 2.0 s), shove them.
        // (`tick` advances 1/60 s.)
        let mut app = smoked_player_app(Vec3::X * 1.0, false);
        for _ in 0..90 {
            tick(&mut app);
        }
        assert_eq!(app.world().resource::<Squad>().0[0].state.shove_serial, 0, "reacted before 1.5 s");
        assert!(take_shoved_victims().is_empty());
        for _ in 0..40 {
            tick(&mut app);
        }
        assert_eq!(app.world().resource::<Squad>().0[0].state.shove_serial, 1);
        assert!(take_shoved_victims().contains(&Victim::Player));
        // Far away: run in to shove reach instead of holding follow distance.
        let mut app = smoked_player_app(Vec3::X * 6.0, false);
        let mut shoved = false;
        for _ in 0..300 {
            tick(&mut app);
            shoved |= take_shoved_victims().contains(&Victim::Player);
        }
        let d = app.world().resource::<Squad>().0[0].position.length();
        assert!(shoved && d < 1.3, "bot at {d} m, shoved {shoved}");
        // Downed and still held: no revive progress.
        let mut app = smoked_player_app(Vec3::X * 1.0, true);
        for _ in 0..200 {
            tick(&mut app);
        }
        assert!(app.world().resource::<Match>().incap);
        assert_eq!(app.world().resource::<Squad>().0[0].revive_time, 0.);
    }
    #[test]
    fn bots_back_away_from_a_tank_in_view_and_not_from_commons() {
        // Retail SurvivorLegsRetreat: inside the Tank's 450-unit danger
        // range the bot backs off (while shooting) up to 1.5x that range.
        let run = |model: &str, threat_at: Vec3, bot_at: Vec3| {
            let (mut app, enemy) = game_app();
            app.insert_resource(Collision::test_floor());
            {
                let mut e = app.world_mut().entity_mut(enemy);
                e.get_mut::<Enemy>().unwrap().model = model.into();
                e.get_mut::<Enemy>().unwrap().health = 1e6;
                *e.get_mut::<Transform>().unwrap() = Transform::from_translation(threat_at);
            }
            app.world_mut().resource_mut::<Squad>().0.push(Companion {
                state: Match::new(&pistol()),
                position: bot_at,
                spawn: bot_at,
                model: String::new(),
                vertical: 0.,
                cooldown: 0.,
                revive_time: 0.,
                aim: Vec3::NEG_Z,
            });
            let mut farthest: f32 = 0.;
            // Bots backpedal at walk speed while watching the threat
            // (backing_walk), so allow 8 s.
            for _ in 0..480 {
                tick(&mut app);
                // Hold the threat still (the test app runs the common chase AI).
                app.world_mut().get_mut::<Transform>(enemy).unwrap().translation = threat_at;
                let bot = app.world().resource::<Squad>().0[0].position;
                farthest = farthest.max(((bot - threat_at) * Vec3::new(1., 0., 1.)).length());
            }
            farthest / UNIT
        };
        let tank = run(crate::special_infected::SpecialClass::Tank.model(), Vec3::new(-1.5, 0., -4.), Vec3::new(-1.5, 0., -1.));
        assert!(tank > 450., "bot only got {tank} units from the Tank");
        // The player standing right behind the bot: it backs past them.
        let tank = run(crate::special_infected::SpecialClass::Tank.model(), Vec3::X * -4., Vec3::X * -1.);
        assert!(tank > 450., "bot stuck {tank} units from the Tank behind the player");
        let common = run("models/infected/common_male01.mdl", Vec3::new(-1.5, 0., -4.), Vec3::new(-1.5, 0., -1.));
        assert!(common < 450., "bot ran {common} units from a common");
    }
    #[test]
    fn stacked_bots_spread_out_and_follow_without_overlapping() {
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        app.insert_resource(Collision::test_floor());
        for _ in 0..3 {
            app.world_mut().resource_mut::<Squad>().0.push(Companion {
                state: Match::new(&pistol()),
                position: Vec3::X * 10.,
                spawn: Vec3::X * 10.,
                model: String::new(),
                vertical: 0.,
                cooldown: 0.,
                revive_time: 0.,
                // Facing their path, as bot_look turns them in game (the
                // test app does not run the look system).
                aim: Vec3::NEG_X,
            });
        }
        let mut closest = f32::INFINITY;
        for frame in 0..240 {
            tick(&mut app);
            let feet = app.world_mut().query::<&Body>().single(app.world()).unwrap().feet;
            let mut all: Vec<Vec3> = app.world().resource::<Squad>().0.iter().map(|m| m.position).collect();
            all.push(feet);
            if frame >= 3 {
                for i in 0..all.len() {
                    for j in i + 1..all.len() {
                        closest = closest.min(((all[i] - all[j]) * Vec3::new(1., 0., 1.)).length());
                    }
                }
            }
        }
        // After the first steps nobody stands inside anyone (hulls are 32
        // units wide), and all three still come up behind the player.
        assert!(closest >= 2. * RADIUS - 0.01, "closest {} units", closest / UNIT);
        for m in &app.world().resource::<Squad>().0 {
            assert!(m.position.length() / UNIT < 260., "{:?}", m.position / UNIT);
        }
    }
    #[test]
    fn bots_take_turns_through_a_doorway() {
        // c1m1 hotel stairwell: two bots reached a 56-unit doorway side by
        // side, each angled into the middle and both stayed wedged on the
        // jambs until the stuck teleport.
        let (mut app, enemy) = game_app();
        app.world_mut().despawn(enemy);
        let mut c = Collision::test_floor();
        let (wall, half) = (3., 28. * UNIT);
        c.test_add_box(Vec3::new(wall, 0., -20.), Vec3::new(wall + 0.2, 3., -half));
        c.test_add_box(Vec3::new(wall, 0., half), Vec3::new(wall + 0.2, 3., 20.));
        app.insert_resource(c);
        app.world_mut().query::<&mut Body>().single_mut(app.world_mut()).unwrap().feet = Vec3::X * 14.;
        for z in [-18., 18.] {
            let at = Vec3::new(wall - 20. * UNIT, 0., z * UNIT);
            app.world_mut().resource_mut::<Squad>().0.push(Companion {
                state: Match::new(&pistol()),
                position: at,
                spawn: at,
                model: String::new(),
                vertical: 0.,
                cooldown: 0.,
                revive_time: 0.,
                aim: Vec3::X,
            });
        }
        for _ in 0..240 {
            tick(&mut app);
        }
        for m in &app.world().resource::<Squad>().0 {
            assert!(m.position.x > wall + 1., "bot still at the doorway: {:?}", m.position / UNIT);
        }
    }
}
#[cfg(test)]
mod punch_tests {
    use super::*;
    #[test]
    fn retail_gun_punch_scales_by_stance_and_decays() {
        let mut body = Body::at(Vec3::ZERO);
        assert_eq!(gun_punch(1.2, &body), 1.2);
        body.velocity = Vec3::X * 100. * UNIT;
        assert_eq!(gun_punch(2., &body), 2.5);
        body.crouched = true;
        assert!((gun_punch(2., &body) - 1.8).abs() < 1e-6);
        body.velocity = Vec3::ZERO;
        assert_eq!(gun_punch(2., &body), 1.);
        body.grounded = false;
        assert_eq!(gun_punch(2., &body), 4.);
        // 4 degrees: -(2 + 20) * 0.1 per 0.1 s, then to rest.
        assert!((decay_punch(4., 0.1) - 1.8).abs() < 1e-5);
        assert_eq!(decay_punch(0.5, 0.1), 0.);
        let mut punch = 6.;
        let mut t = 0.;
        while punch > 0. {
            punch = decay_punch(punch, 1. / 30.);
            t += 1. / 30.;
        }
        assert!(t < 0.4, "a shotgun kick recovers in a few frames: {t}");
    }
}
