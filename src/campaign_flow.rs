//! Native exit-volume bridge to the existing-window map loader.
use crate::{
    campaign_settings::{Catalog, SessionSettings},
    collision::{Collision, HEIGHT},
    game::Body,
    game_state::{Match, Squad},
    l4d2r_cinematic::MapLogic,
};
use bevy::prelude::*;
#[derive(Clone)]
pub struct Carry {
    pub player: Match,
    pub bots: Vec<Match>,
}
#[derive(Resource, Default)]
pub struct Checkpoint {
    pub goal: Option<Vec3>,
    exit: Option<usize>,
    clock: f32,
    /// Bot sent to close the checkpoint door, and where to walk.
    pub door: Option<(usize, Vec3)>,
    /// Seconds until the next door-closing check (None: not armed).
    door_clock: Option<f32>,
    /// Every living survivor is inside with the checkpoint door shut: the
    /// chapter is ending (Event.SafeRoom plays from here).
    pub sealed: bool,
}
/// Retail SurvivorBotReachedCheckpoint (server.dll 103b3f60/103b4090): once
/// every living survivor is inside, a bot checks after RandomFloat(1, 2) s
/// and then every sb_close_checkpoint_door_interval (2) s, walks to the open
/// checkpoint door and closes it ("Closing the checkpoint door").
pub const DOOR_CHECK_INTERVAL: f32 = 2.;
/// Distance (m) from the door at which the bot uses it.
pub const DOOR_USE_RANGE: f32 = 1.6;
pub fn carry_state(mut state: Match) -> Match {
    state.ladder_pose = None;
    if state.dead {
        state.resuscitate();
    }
    state.paused = true;
    state.action = 0.;
    state.medical_kind = 0;
    state.medical_target = None;
    state.reload = 0.;
    state.reload_phase = 0;
    state.cooldown = 0.;
    state.burst_left = 0;
    state.shove = 0.;
    state.damage_flash = 0.;
    state.throw_stage = 0;
    state.throw_class = None;
    state.throw_clock = 0.;
    state.slot = 2;
    state.elapsed = 0.;
    state.hint.clear();
    state
}
/// Seconds of the map's outro after FinaleWon before returning to the menu.
pub const FINALE_OUTRO: f32 = 12.;
/// trigger_finale FinaleEscapeFinished (the survivors escaped): the campaign
/// is complete. The map's own outro relays/cameras run, then the game
/// returns to the main menu. Retail's credits and statistics screens are not
/// reproduced.
pub fn finale_won(
    time: Res<Time<Fixed>>,
    logic: Res<MapLogic>,
    mut state: ResMut<Match>,
    mut frontend: ResMut<crate::game_menu::Frontend>,
    mut clock: Local<f32>,
) {
    // Online, a guest's campaign ends with the host's (net_flow).
    let won = if crate::net_play::guest() { crate::net_flow::host_won() } else { logic.world.cine.finale_won };
    if !won || !frontend.started {
        *clock = 0.;
        return;
    }
    if *clock == 0. {
        println!("Finale won: campaign complete");
    }
    *clock += time.delta_secs();
    state.hint = "Rescued! Campaign complete".into();
    if *clock >= FINALE_OUTRO {
        frontend.page = crate::game_menu::Page::Home;
        frontend.started = false;
        state.paused = true;
        *clock = 0.;
    }
}
pub fn update(
    time: Res<Time<Fixed>>,
    session: Res<SessionSettings>,
    catalog: Res<Catalog>,
    mut logic: ResMut<MapLogic>,
    collision: Res<Collision>,
    body: Single<&Body>,
    mut state: ResMut<Match>,
    squad: Res<Squad>,
    source: Res<crate::game_ui::HudSource>,
    mut checkpoint: ResMut<Checkpoint>,
    mut requests: ResMut<crate::map_transition::Requests>,
) {
    let _timing = crate::performance::scope("campaign_flow::update");
    // In multiplayer the host leads chapter changes; guests follow it
    // (net_play.rs).
    if state.paused || state.dead || state.incap || logic.active || requests.0.is_some() || crate::net_play::guest() {
        return;
    }
    let current = session
        .map
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let exit = logic.world.ents.iter().enumerate().find_map(|(id, e)| {
        if !e.alive
            || !matches!(
                e.me.classname.as_str(),
                "info_changelevel" | "trigger_changelevel"
            )
        {
            return None;
        }
        if matches!(
            e.kind,
            l4d2r_game_core::entities::Kind::Trigger { disabled: true, .. }
        ) {
            return None;
        }
        // SF_CHANGELEVEL_NOTOUCH (2): the start saferoom's volume back to
        // the previous chapter only marks the transition area (c2m4's
        // c2m3_c2m4_changelevel sent a fresh start straight back to c2m3).
        if e.me.kv("spawnflags").and_then(|v| v.trim().parse::<i64>().ok()).is_some_and(|f| f & 2 != 0) {
            return None;
        }
        let name = e.me.kv("map")?;
        if name.eq_ignore_ascii_case(&current)
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        let model = e.me.brush?;
        let (lo, hi) = logic.bounds(id)?;
        if body.feet.x + crate::collision::RADIUS < lo.x
            || body.feet.x - crate::collision::RADIUS > hi.x
            || body.feet.z + crate::collision::RADIUS < lo.z
            || body.feet.z - crate::collision::RADIUS > hi.z
            || body.feet.y + HEIGHT < lo.y
            || body.feet.y > hi.y
        {
            return None;
        }
        if !collision.overlaps_model(model, logic.world.world_matrix(id), body.feet, HEIGHT) {
            return None;
        }
        let path = catalog
            .campaigns
            .iter()
            .flat_map(|c| &c.chapters)
            .find(|ch| {
                ch.map
                    .file_stem()
                    .is_some_and(|n| n.to_str().is_some_and(|s| s.eq_ignore_ascii_case(name)))
            })
            .map(|ch| ch.map.clone())?;
        Some((id, model, path))
    });
    let Some((id, model, path)) = exit else {
        *checkpoint = Checkpoint::default();
        return;
    };
    checkpoint.goal = Some(body.feet);
    let matrix = logic.world.world_matrix(id);
    let gathered = squad
        .0
        .iter()
        .filter(|m| !m.state.dead)
        .all(|m| !m.state.incap && collision.overlaps_model(model, matrix, m.position, HEIGHT));
    let center = logic
        .bounds(id)
        .map_or(body.feet, |(lo, hi)| (lo + hi) * 0.5);
    let door = logic.world.ents.iter().enumerate().filter(|(_,e)| e.alive && e.me.classname=="prop_door_rotating_checkpoint")
        .filter_map(|(i,e)|logic.bounds(i).map(|(lo,hi)|(i,e,(lo+hi)*0.5)))
        .filter(|(_,_,p)|p.distance(center)<20.)
        .min_by(|a,b|a.2.distance_squared(center).total_cmp(&b.2.distance_squared(center)))
        .map(|(i,e,p)|(i,p,matches!(&e.kind,l4d2r_game_core::entities::Kind::Door(d) if d.frac<=0.02 && d.goal<=0.02)));
    let closed = door.is_none_or(|(_, _, closed)| closed);
    match door.filter(|_| gathered && !closed) {
        Some((door_id, at, _)) => {
            // Stand on the safe-room side of the door, not in its swing.
            let inward = (center - at).with_y(0.).normalize_or_zero();
            let spot = (at + inward * 0.9).with_y(body.feet.y.min(at.y));
            close_door(&mut checkpoint, &mut logic, &squad, &collision, model, id, door_id, spot, time.delta_secs(), time.elapsed_secs())
        }
        None => {
            checkpoint.door = None;
            checkpoint.door_clock = None;
        }
    }
    checkpoint.sealed = gathered && closed;
    if !gathered || !closed {
        checkpoint.clock = 0.;
        checkpoint.exit = Some(id);
        state.hint = if !gathered {
            "Gather every living survivor in the safe room"
        } else {
            "Close the checkpoint door to continue"
        }
        .into();
        return;
    }
    if checkpoint.exit != Some(id) {
        checkpoint.clock = 0.;
        checkpoint.exit = Some(id);
    }
    checkpoint.clock += time.delta_secs();
    if checkpoint.clock >= 1. {
        let mut next = session.clone();
        next.map = path;
        requests.1 = Some(Carry {
            player: state.clone(),
            bots: squad.0.iter().map(|m| m.state.clone()).collect(),
        });
        requests.0 = Some((source.0.clone(), next));
        state.paused = true;
    }
}

/// Arm, run and act on the bot door-closing check for the open checkpoint
/// door `id` while everyone is inside; `at` is where the bot stands to use it.
#[allow(clippy::too_many_arguments)]
fn close_door(
    checkpoint: &mut Checkpoint,
    logic: &mut MapLogic,
    squad: &Squad,
    collision: &Collision,
    model: usize,
    exit: usize,
    id: usize,
    at: Vec3,
    dt: f32,
    elapsed: f32,
) {
    if let Some((bot, _)) = checkpoint.door {
        let Some(m) = squad.0.get(bot).filter(|m| able(m)) else {
            checkpoint.door = None;
            return;
        };
        checkpoint.door = Some((bot, at));
        if (m.position - at).with_y(0.).length() < DOOR_USE_RANGE {
            // Same path as the player's +use (MapLogic::use_near).
            logic.world.use_ent(id, None);
            collision.sync_entities(logic);
            checkpoint.door = None;
            checkpoint.door_clock = Some(DOOR_CHECK_INTERVAL);
        }
        return;
    }
    // RandomFloat(1, 2): a cheap spread from the clock is enough here.
    let clock = checkpoint.door_clock.get_or_insert(1. + (elapsed * 7.31).fract());
    *clock -= dt;
    if *clock > 0. {
        return;
    }
    *clock = DOOR_CHECK_INTERVAL;
    let matrix = logic.world.world_matrix(exit);
    checkpoint.door = squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, m)| able(m) && collision.overlaps_model(model, matrix, m.position, HEIGHT))
        .min_by(|a, b| a.1.position.distance_squared(at).total_cmp(&b.1.position.distance_squared(at)))
        .map(|(i, _)| (i, at));
}
fn able(m: &crate::game_state::Companion) -> bool {
    !m.state.dead && !m.state.incap && !m.state.pinned
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chapter_carry_preserves_inventory_health_and_clears_transient_actions() {
        let mut s = Match::new(&crate::game_state::tests_weapon());
        s.health = 37.;
        s.medkit = true;
        s.adrenaline = true;
        s.throwable = Some("weapon_pipe_bomb".into());
        s.revives = 1;
        s.action = 3.;
        s.burst_left = 2;
        s.ladder_pose = Some(crate::ai_ladder::Pose { facing: Vec3::X, descending: false });
        let next = carry_state(s);
        assert_eq!(next.health, 37.);
        assert!(next.medkit && next.adrenaline);
        assert_eq!(next.throwable.as_deref(), Some("weapon_pipe_bomb"));
        assert_eq!(next.revives, 1);
        assert_eq!(next.action, 0.);
        assert_eq!(next.burst_left, 0);
        assert!(next.ladder_pose.is_none());
    }
    #[test]
    fn checkpoint_waits_for_the_party_and_closed_door_then_queues_native_destination() {
        use bevy::ecs::system::RunSystemOnce;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let lab = crate::test_lab::path();
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &lab, true).unwrap();
        logic.active = false;
        let door = logic.world.find("lab_checkpoint")[0];
        if let l4d2r_game_core::entities::Kind::Door(d) = &mut logic.world.ents[door].kind {
            d.frac = 1.;
            d.goal = 1.;
        } else {
            panic!("Checkpoint did not initialize as a native door")
        }
        let collision = crate::collision::Collision::for_trigger_audit(&logic);
        let feet = Vec3::from_array(crate::binary::source_position([3820., 2750., 4.]));
        let weapon = crate::game_state::tests_weapon();
        let mut state = Match::new(&weapon);
        state.paused = false;
        state.health = 37.;
        state.medkit = true;
        let bot = crate::game_state::Companion {
            state: Match::new(&weapon),
            position: Vec3::ZERO,
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::ZERO,
        };
        let destination = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::default())
            .insert_resource(state)
            .insert_resource(Squad(vec![bot]))
            .insert_resource(logic)
            .insert_resource(collision)
            .insert_resource(crate::game_ui::HudSource(root.into()))
            .insert_resource(SessionSettings {
                map: lab,
                survivor: crate::campaign_settings::Survivor::Ellis,
                difficulty: crate::campaign_settings::Difficulty::Normal,
                original: false,
            })
            .insert_resource(Catalog {
                campaigns: vec![crate::campaign_settings::Campaign {
                    name: "Test".into(),
                    original: false,
                    chapters: vec![crate::campaign_settings::Chapter {
                        name: "Hotel".into(),
                        map: destination.clone(),
                        image: String::new(),
                    }],
                }],
            })
            .init_resource::<Checkpoint>()
            .init_resource::<crate::map_transition::Requests>();
        app.world_mut().spawn(Body::at(feet));
        for stage in 0..3 {
            if stage >= 1 {
                app.world_mut().resource_mut::<Squad>().0[0].position = feet;
            }
            if stage == 2 {
                if let l4d2r_game_core::entities::Kind::Door(d) =
                    &mut app.world_mut().resource_mut::<MapLogic>().world.ents[door].kind
                {
                    d.frac = 0.;
                    d.goal = 0.;
                }
            }
            for _ in 0..70 {
                app.world_mut()
                    .resource_mut::<Time<Fixed>>()
                    .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
                app.world_mut().run_system_once(update).unwrap();
            }
            assert_eq!(
                app.world()
                    .resource::<crate::map_transition::Requests>()
                    .0
                    .is_some(),
                stage == 2
            );
        }
        let requests = app.world().resource::<crate::map_transition::Requests>();
        assert_eq!(requests.0.as_ref().unwrap().1.map, destination);
        assert!(app.world().resource::<Checkpoint>().door.is_none(), "closed door: nobody sent");
        assert_eq!(requests.1.as_ref().unwrap().player.health, 37.);
        assert!(requests.1.as_ref().unwrap().player.medkit);
    }
}
#[cfg(test)]
mod bot_door {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use l4d2r_game_core::entities::Kind;
    /// Everyone inside with the door open: a bot is sent after the retail
    /// 1-2 s check, walks to the door and closes it.
    #[test]
    fn a_bot_closes_the_checkpoint_door_once_everyone_is_inside() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let lab = crate::test_lab::path();
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &lab, true).unwrap();
        logic.active = false;
        let door = logic.world.find("lab_checkpoint")[0];
        // Retail checkpoint doors all carry "Use closes" (8192; c1m1, c1m2,
        // c2m1, c5m1); the laboratory door omits it.
        logic.world.ents[door].me.ent.0.retain(|(k, _)| !k.eq_ignore_ascii_case("spawnflags"));
        logic.world.ents[door].me.ent.0.push(("spawnflags".into(), "8192".into()));
        logic.world.use_ent(door, None);
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        assert!(matches!(&logic.world.ents[door].kind, Kind::Door(d) if d.frac > 0.5), "door opened");
        let collision = crate::collision::Collision::for_trigger_audit(&logic);
        let feet = Vec3::from_array(crate::binary::source_position([3820., 2750., 4.]));
        let weapon = crate::game_state::tests_weapon();
        let mut state = Match::new(&weapon);
        state.paused = false;
        let bot = |position| crate::game_state::Companion {
            state: Match::new(&weapon),
            position,
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::ZERO,
        };
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::default())
            .insert_resource(state)
            .insert_resource(Squad(vec![bot(feet), bot(feet)]))
            .insert_resource(logic)
            .insert_resource(collision)
            .insert_resource(crate::game_ui::HudSource(root.into()))
            .insert_resource(SessionSettings {
                map: lab,
                survivor: crate::campaign_settings::Survivor::Ellis,
                difficulty: crate::campaign_settings::Difficulty::Normal,
                original: false,
            })
            .insert_resource(Catalog {
                campaigns: vec![crate::campaign_settings::Campaign {
                    name: "Test".into(),
                    original: false,
                    chapters: vec![crate::campaign_settings::Chapter {
                        name: "Hotel".into(),
                        map: root.join("left4dead2/maps/c1m1_hotel.bsp"),
                        image: String::new(),
                    }],
                }],
            })
            .init_resource::<Checkpoint>()
            .init_resource::<crate::map_transition::Requests>();
        app.world_mut().spawn(Body::at(feet));
        let step = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            app.world_mut().run_system_once(update).unwrap();
        };
        // Not before the first 1 s check.
        for _ in 0..50 {
            step(&mut app);
        }
        assert!(app.world().resource::<Checkpoint>().door.is_none());
        let mut sent = None;
        for _ in 0..80 {
            step(&mut app);
            sent = sent.or(app.world().resource::<Checkpoint>().door);
        }
        let (index, spot) = sent.expect("a bot is sent to close the door");
        assert!(index < 2);
        let goal = app.world().resource::<Checkpoint>().goal.unwrap();
        assert!(spot.distance(goal) < 20.);
        // Arriving at the spot closes the door.
        app.world_mut().resource_mut::<Squad>().0[index].position = spot;
        step(&mut app);
        let logic = app.world().resource::<MapLogic>();
        assert!(matches!(&logic.world.ents[door].kind, Kind::Door(d) if d.goal <= 0.02), "door closing");
        assert!(app.world().resource::<Checkpoint>().door.is_none());
    }
}
#[cfg(test)]
mod finale_entity {
    use l4d2r_game_core::entities::Kind;
    /// c2m5's stage button: disabled until the map enables it, then a use
    /// starts the finale; the escape relay's FinaleEscapeFinished wins it.
    #[test]
    fn concert_finale_button_starts_and_escape_finished_wins() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m5_concert.bsp");
        if !path.is_file() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let id = logic.world.find("stage_escape_button")[0];
        let started = |logic: &crate::l4d2r_cinematic::MapLogic| matches!(logic.world.ents[id].kind, Kind::Finale { started: true, .. });
        assert!(logic.bounds(id).is_some(), "the use ray needs the finale prop's bounds");
        logic.world.use_ent(id, None);
        assert!(!started(&logic), "StartDisabled finale ignores use");
        logic.world.input(id, "Enable", "", None, None);
        logic.world.cine.director_commands.clear();
        logic.world.use_ent(id, None);
        assert!(started(&logic));
        // The Director starts the finale after the map's FinaleStart outputs.
        logic.world.flush_io();
        assert!(logic.world.cine.director_commands.iter().any(|(c, _)| c == "BeginFinale"));
        assert!(!logic.world.cine.finale_won);
        logic.world.input(id, "FinaleEscapeVehicleReadyForSurvivors", "", None, None);
        assert!(logic.world.cine.finale_vehicle_ready);
        logic.world.input(id, "FinaleEscapeFinished", "", None, None);
        assert!(logic.world.cine.finale_won);
    }
}
