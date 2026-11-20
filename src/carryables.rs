//! Carryable props occupy the hands temporarily without replacing weapon inventory.
use bevy::prelude::*;
#[derive(Component)]
pub(crate) struct Flight {
    velocity: Vec3,
    start: Option<Vec3>,
    age: f32,
}
// Retail server.dll constructors: duration 1.95s, range 75 units,
// aim tolerance 0.1. Local carry state is discarded on map/round replacement.
const COLA_DURATION: f32 = 1.95;
const COLA_RANGE: f32 = 75. * crate::collision::UNIT;
/// How far the crosshair ray may pass from a use point and still target it.
const AIM_REACH: f32 = 24. * crate::collision::UNIT;
// Owned server.dll gas_can_use_duration=2, gascan_use_range=65.
const GAS_DURATION: f32 = 2.;
const GAS_RANGE: f32 = 65. * crate::collision::UNIT;
#[derive(Default)]
pub(crate) struct Delivery {
    target: Option<usize>,
    elapsed: f32,
    completed: std::collections::HashSet<usize>,
    map: usize,
    clock: f32,
    pouring: bool,
    cues: Vec<(usize, u8)>,
}
impl Delivery {
    fn cancel(
        &mut self,
        state: &mut crate::game_state::Match,
        world: &mut l4d2r_game_core::entities::EntityWorld,
    ) {
        if let Some(id) = self.target.take() {
            if self.pouring {
                self.cues.push((id, 1));
            }
            self.pouring = false;
            if world.ents.get(id).is_some_and(|e| e.alive) {
                world.fire_output_pub(id, "OnUseCancelled");
            }
        }
        self.elapsed = 0.;
        if state.medical_kind == 10 {
            state.medical_kind = 0;
            state.action = 0.;
        }
    }
    fn tick(
        &mut self,
        state: &mut crate::game_state::Match,
        input: &mut crate::game::Input,
        held: bool,
        body: &crate::game::Body,
        logic: &mut crate::l4d2r_cinematic::MapLogic,
        collision: &crate::collision::Collision,
        dt: f32,
    ) {
        let map = std::sync::Arc::as_ptr(&logic.world.bsp) as usize;
        if self.map != map || logic.world.time < self.clock {
            *self = Self { map, ..default() };
        }
        self.clock = logic.world.time;
        if state.carried.is_some() {
            state.hint.clear();
        }
        let gas = state.carried.as_deref() == Some("weapon_gascan");
        let duration = if gas { GAS_DURATION } else { COLA_DURATION };
        let range = if gas { GAS_RANGE } else { COLA_RANGE };
        let flag = if gas { 1 } else { 2 };
        let fuel_available = !gas
            || logic
                .world
                .scavenge_progress()
                .is_none_or(|(n, total)| n < total);
        let available = state.slot == 6
            && (gas || state.carried.as_deref() == Some("weapon_cola_bottles"))
            && fuel_available
            && !state.dead
            && !state.incap
            && !state.infected
            && !state.pinned;
        let eye = body.feet + Vec3::Y * body.stance.eye();
        let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
        let target = available
            .then(|| {
                logic
                    .world
                    .ents
                    .iter()
                    .enumerate()
                    .filter_map(|(id, e)| {
                        if !e.alive
                            || e.me.classname != "point_prop_use_target"
                            || e.me.spawnflags() & flag == 0
                            || e.me.kv_i32("StartDisabled") == Some(1)
                            || (!gas && self.completed.contains(&id))
                        {
                            return None;
                        }
                        let point = Vec3::from_array(crate::binary::source_position(
                            logic.world.world_matrix(id).w_axis.truncate().to_array(),
                        ));
                        let delta = point - eye;
                        let distance = delta.length();
                        // Aim: the retail 0.1 tolerance cone, or the crosshair ray
                        // passing within a hand's reach of the point. Up close the
                        // receiving hatch is far below the point's angle (Whitaker's
                        // flap at c1m2), and the cone alone rejected a direct look.
                        let along = forward.dot(delta);
                        let miss = (delta - forward * along).length();
                        if distance > range
                            || distance <= 0.001
                            || (forward.dot(delta / distance) < 0.9
                                && (along <= 0. || miss > AIM_REACH))
                        {
                            return None;
                        }
                        // The point is behind the receiving hatch. Its opening animation
                        // owns that obstruction; other intervening walls still block use.
                        let hatch = e
                            .conns
                            .iter()
                            .find(|c| {
                                c.output.eq_ignore_ascii_case("OnUseStarted")
                                    && c.input.eq_ignore_ascii_case("SetAnimation")
                            })
                            .and_then(|c| logic.world.find(&c.target).first().copied());
                        let trace = hatch.map_or_else(
                            || collision.trace(eye, point, Vec3::ZERO),
                            |id| collision.prop_trace(id, eye, point, Vec3::ZERO),
                        );
                        (trace.fraction >= 0.99 && !trace.start_solid).then_some((id, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|p| p.0)
            })
            .flatten();
        if !held || target != self.target {
            self.cancel(state, &mut logic.world);
        }
        let Some(id) = target else {
            return;
        };
        state.hint = if gas {
            "Hold left click to pour fuel"
        } else {
            "Hold left click to deliver cola"
        }
        .into();
        if !held {
            return;
        }
        input.use_item = false;
        input.fire = false;
        input.fire_held = false;
        input.shove = false;
        if self.target.is_none() {
            self.target = Some(id);
            self.pouring = gas;
            if gas {
                self.cues.push((id, 0));
            }
            self.elapsed = 0.;
            state.action_total = duration;
            state.medical_kind = 10;
            logic.world.fire_output_pub(id, "OnUseStarted");
        }
        self.elapsed += dt.max(0.);
        state.action = (duration - self.elapsed).max(0.);
        // The use bar (progress_bar, medical_kind 10) shows the progress.
        state.hint.clear();
        if self.elapsed + 1e-5 >= duration {
            if !gas {
                self.completed.insert(id);
            }
            if gas {
                logic.world.score_scavenge_item();
                self.cues.push((id, 2));
                self.pouring = false;
            }
            self.target = None;
            self.elapsed = 0.;
            state.carried = None;
            state.slot = if state.primary.is_some() { 1 } else { 2 };
            state.medical_kind = 0;
            state.action = 0.;
            logic.world.fire_output_pub(id, "OnUseFinished");
        }
    }
}
pub fn update(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut state: ResMut<crate::game_state::Match>,
    mut input: ResMut<crate::game::Input>,
    body: Single<&crate::game::Body>,
    rules: Res<crate::game_state::Rules>,
    collision: Res<crate::collision::Collision>,
    models: Option<Res<crate::viewer::RenderModels>>,
    meshes: Option<Res<Assets<Mesh>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut delivery: Local<Delivery>,
    mut sounds: Option<ResMut<crate::game_audio::Queue>>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    mut drops: Query<(
        Entity,
        &mut Transform,
        &mut crate::game::Pickup,
        &mut Flight,
        &crate::loot::DropHull,
    )>,
) {
    if state.paused {
        return;
    }
    let dt = time.delta_secs();
    for (entity, mut transform, mut pickup, mut flight, hull) in &mut drops {
        let center = pickup.origin - transform.translation;
        if let Some(start) = flight.start.take() {
            transform.translation = start;
        }
        flight.age += dt;
        flight.velocity.y -= 800. * crate::collision::UNIT * dt;
        let from = transform.translation;
        let delta = flight.velocity * dt;
        let hit = collision.trace(from + center, from + center + delta, hull.0);
        transform.translation = from + delta * hit.fraction;
        pickup.origin = transform.translation + center;
        if hit.fraction < 1. || hit.start_solid || flight.age > 8. {
            commands.entity(entity).remove::<Flight>();
        }
    }
    if let Some(logic) = logic.as_deref_mut() {
        delivery.tick(
            &mut state,
            &mut input,
            mouse
                .as_ref()
                .is_some_and(|buttons| buttons.pressed(MouseButton::Left)),
            &body,
            logic,
            &collision,
            dt,
        );
        for (id, cue) in std::mem::take(&mut delivery.cues) {
            if let Some(sounds) = sounds.as_deref_mut() {
                if cue == 0 {
                    sounds.0.push_back(crate::game_audio::Request {
                        name: "Player.UsingGasCan".into(),
                        position: None,
                        looping: false,
                        gain: 1.,
                        map_entity: Some(id), soundlevel: None,
                    });
                } else {
                    sounds.1.push(id);
                    for (entity, sound) in &playing {
                        if sound.0 == id {
                            commands.entity(entity).despawn();
                        }
                    }
                    if cue == 1 {
                        sounds.play("Player.UsingGasCanStop", None);
                    }
                }
            }
        }
        if state.medical_kind == 10 {
            return;
        }
    }
    let Some(class) = state.carried.clone() else {
        return;
    };
    let toss = input.fire || input.fire_held;
    // Grabbed by a special: the carried item drops.
    let release = state.slot != 6 || state.dead || state.incap || state.infected || state.pinned || toss;
    if !release {
        if !state.hint.starts_with("Hold left click to") {
            state.hint = "Left click to toss; switch weapon to drop".into();
        }
        return;
    }
    state.carried = None;
    if state.slot == 6 {
        state.slot = 2;
    }
    state.reload = 0.;
    input.fire = false;
    input.fire_held = false;
    let direction = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let start = body.feet + Vec3::Y * body.stance.eye() + direction * 0.5;
    let entity = crate::loot::drop_item(
        &mut commands,
        models.as_deref(),
        meshes.as_deref(),
        &rules,
        &collision,
        &class,
        body.feet + direction * 0.6,
        None,
    );
    commands.entity(entity).insert(Flight {
        velocity: body.velocity + direction * if toss { 8. } else { 1. },
        start: Some(start),
        age: 0.,
    });
}

#[cfg(test)]
mod whitaker_tests {
    use super::*;
    #[test]
    fn installed_streets_whitaker_cola_and_tanker_chain() {
        use l4d2r_game_core::entities::Kind;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let mut dialogue = logic.dialogue.take().unwrap();
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        let mut audio = crate::game_audio::Queue::default();
        let mut speech = vec![];
        let button = logic.world.find("gunshop_door_button")[0];
        let door = logic.world.find("gunshop_door_01")[0];
        let tanker = logic.world.find("tanker_destroy_relay")[0];
        let clip = logic.world.find("tanker_clip_brush")[0];
        let wall = logic.world.find("tanker_wall_01")[0];
        let nav = logic.world.find("tanker_nav_blocker")[0];
        let weapon = crate::game_state::Weapon::load(&mut assets, "weapon_pistol").unwrap();
        let mut pump = |logic: &mut crate::l4d2r_cinematic::MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                dialogue.wait_for_scene_loads(logic.world.time);
                logic.world.tick(1. / 60.);
                dialogue.tick(
                    &mut logic.world,
                    &mut audio,
                    &["mechanic", "producer", "coach", "gambler"],
                );
                for r in audio.0.drain(..) {
                    let def = registry
                        .definitions
                        .get(&r.name.to_lowercase())
                        .unwrap_or_else(|| panic!("Missing speech {}", r.name));
                    assert!(assets.exists(&def.waves[0]), "Missing WAV {}", r.name);
                    speech.push(r.name);
                }
            }
        };
        let button_eye = Vec3::from_array(crate::binary::source_position([-4924., -2010., 491.4]));
        let button_dir =
            Vec3::from_array(crate::binary::source_position([0., -1., 0.])).normalize();
        assert!(
            logic.use_near(button_eye, button_dir, &collision),
            "Whitaker button must respond to normal nearby use"
        );
        pump(&mut logic, 25.);
        assert!(!logic.world.ents[button].alive);
        assert!(
            matches!(&logic.world.ents[door].kind, Kind::Door(d) if !d.locked),
            "Whitaker must open the door before the 45-second fallback"
        );
        let outside = logic.world.find("trigger_store_glow")[0];
        logic.world.fire_output_pub(outside, "OnTrigger");
        pump(&mut logic, 40.);
        logic
            .world
            .queue_input("store_alarm_relay", "Trigger", "", 0., None);
        pump(&mut logic, 2.);
        let cola = logic.world.find("cola")[0];
        assert!(
            logic.world.ents[cola].alive,
            "Opening the store must spawn the mission cola"
        );
        logic.world.fire_output_pub(cola, "OnPlayerPickup");
        pump(&mut logic, 0.5);
        let mut probes = vec![];
        for id in [clip]
            .into_iter()
            .chain(logic.world.find("tanker_wall_01").iter().copied())
        {
            let (lo, hi) = logic.bounds(id).unwrap();
            println!("Route blocker {id} bounds {lo:?} .. {hi:?}");
            for t in [0.15, 0.35, 0.5, 0.65, 0.85] {
                for y in [10.8, 11.3, 11.8, 12.3] {
                    for axis in [0, 2] {
                        let mut from = lo.lerp(hi, t);
                        from.y = y;
                        let mut to = from;
                        from[axis] = lo[axis] - 0.6;
                        to[axis] = hi[axis] + 0.6;
                        let hit = collision.trace(from, to, Vec3::new(0.3, 0.6, 0.3));
                        if hit.fraction < 0.99 || hit.start_solid {
                            probes.push((from, to));
                        }
                    }
                }
            }
        }
        // Short walking segments inside the old clipping volume avoid unrelated
        // buildings and the truck cab beyond its much larger bounding box.
        let (lo, hi) = logic.bounds(clip).unwrap();
        for x in 1..10 {
            for z in 1..10 {
                let center = Vec3::new(
                    lo.x + (hi.x - lo.x) * x as f32 / 10.,
                    10.8,
                    lo.z + (hi.z - lo.z) * z as f32 / 10.,
                );
                for axis in [Vec3::X, Vec3::Z] {
                    let a = center - axis * 0.7;
                    let b = center + axis * 0.7;
                    let hit = collision.trace(a, b, Vec3::new(0.3, 0.6, 0.3));
                    if hit.fraction < 0.99 || hit.start_solid {
                        probes.push((a, b));
                    }
                }
            }
        }
        let mut state = crate::game_state::Match::new(&weapon);
        state.paused = false;
        state.slot = 6;
        state.carried = Some("weapon_cola_bottles".into());
        let point = Vec3::from_array(crate::binary::source_position([-5372., -1980., 669.]));
        // Landing side of the hatch; y=-1916 is inside the authored wall.
        let mut body = crate::game::Body::at(Vec3::from_array(crate::binary::source_position([
            -5372., -1932., 616.,
        ])));
        let dir = (point - body.feet - Vec3::Y * body.stance.eye()).normalize();
        body.yaw = (-dir.x).atan2(-dir.z);
        body.pitch = dir.y.asin();
        let mut delivery = Delivery::default();
        let mut input = crate::game::Input::default();
        delivery.tick(
            &mut state, &mut input, true, &body, &mut logic, &collision, 0.5,
        );
        assert!(
            delivery.target.is_some(),
            "Native delivery point must be reachable through its receiving hatch"
        );
        delivery.tick(
            &mut state, &mut input, false, &body, &mut logic, &collision, 0.1,
        );
        assert!(delivery.target.is_none());
        assert!(state.carried.is_some(), "Cancelling must retain the cola");
        for _ in 0..117 {
            delivery.tick(
                &mut state,
                &mut input,
                true,
                &body,
                &mut logic,
                &collision,
                1. / 60.,
            );
        }
        assert!(
            state.carried.is_none(),
            "1.95-second handoff must consume cola"
        );
        assert_eq!(delivery.completed.len(), 1);
        assert!(
            logic.world.ents[tanker].alive,
            "Explosion must respect dialogue delay"
        );
        pump(&mut logic, 14.);
        assert!(
            !logic.world.ents[tanker].alive,
            "Dialogue must trigger before 15-second fallback"
        );
        pump(&mut logic, 5.); // Long native lines can start destruction at 11.5 seconds.
        assert!(
            !logic.world.ents[clip].alive,
            "Tanker collision must be removed"
        );
        assert!(!logic.world.ents[wall].alive, "Route wall must be removed");
        assert!(
            matches!(
                logic.world.ents[nav].kind,
                Kind::NavBlocker { blocked: false, .. }
            ),
            "Route nav must unlock"
        );
        assert!(
            logic
                .world
                .find("tanker_front")
                .iter()
                .all(|&i| matches!(logic.world.ents[i].kind, Kind::Brush { enabled: false, .. }))
        );
        pump(&mut logic, 10.);
        drop(pump);
        println!("Verified Whitaker speech: {speech:?}");
        assert!(
            speech.iter().any(|s| s.to_lowercase().contains("whitaker")),
            "Must play Whitaker dialogue"
        );
        assert!(
            speech
                .iter()
                .any(|s| s.to_lowercase().contains("wayblocked")),
            "Must play the native cola mission instructions"
        );
        collision.sync_entities(&logic);
        let clear = probes
            .iter()
            .filter(|(a, b)| {
                let hit = collision.trace(*a, *b, Vec3::new(0.3, 0.6, 0.3));
                hit.fraction >= 0.99 && !hit.start_solid
            })
            .count();
        println!(
            "Tanker route: {clear}/{} previously blocked survivor-hull traces now clear",
            probes.len()
        );
        assert!(
            clear > 0,
            "Destruction must open physical passage, not only nav flags"
        );
    }
}

#[cfg(test)]
mod fuel_tests {
    use super::*;
    fn pump(
        logic: &mut crate::l4d2r_cinematic::MapLogic,
        d: &mut crate::source_director::Director,
        frames: usize,
    ) {
        let mut audio = crate::game_audio::Queue::default();
        for _ in 0..frames {
            logic.world.tick(1. / 60.);
            d.set_script_time(logic.world.time);
            if !d.sync_world(&logic.world) { continue; }
            for (input, param) in std::mem::take(&mut logic.world.cine.director_commands) {
                d.command(&input, &param);
            }
            for r in std::mem::take(&mut logic.world.cine.script_commands) {
                d.entity_script_context(r.entity, &r.input, &r.param, r.activator, r.caller);
            }
            d.tick(1. / 60., 100., 0, 0, Vec3::ZERO);
            crate::game::flush_vscript_commands(d, Some(logic), &mut audio);
        }
    }
    #[test]
    fn installed_atrium_eight_repeatable_pours_run_native_escape_script() {
        use l4d2r_game_core::entities::Kind;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m4_atrium.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let weapon = crate::game_state::Weapon::load(&mut assets, "weapon_pistol").unwrap();
        for name in ["escape_car", "escape_car_glass"] {
            let id=logic.world.find(name)[0];
            let model=logic.world.ents[id].me.mdl.as_ref().unwrap();
            assert_eq!(logic.world.cine.anims.get(&id).map(|v|v.0.as_str()),Some("atrium_plinth"));
            let mut idle=crate::studio::load(&mut assets,model,0,0).unwrap();
            let mut plinth=idle.clone();
            for (meshes,sequence) in [(&mut idle,"idle"),(&mut plinth,"atrium_plinth")] {
                let pose=crate::studio::pose::sample_l4d2r(&mut assets,model,&[sequence],0.).unwrap();
                crate::studio::pose::apply(&mut assets,model,meshes,&pose,false).unwrap();
            }
            let bounds=|meshes:&Vec<crate::studio::ModelMesh>| {
                meshes.iter().flat_map(|m|&m.positions).fold((Vec3::splat(f32::INFINITY),Vec3::splat(f32::NEG_INFINITY)),|(lo,hi),p|(lo.min(Vec3::from_array(*p)),hi.max(Vec3::from_array(*p))))
            };
            let (lo,hi)=bounds(&plinth);let (a,b)=bounds(&idle);
            assert!((lo+hi-a-b).length()>1.,"Native plinth pose must differ materially from idle: {name}");
            println!("Native car pose {name}: plinth bounds {lo:?} / {hi:?}; idle bounds {a:?} / {b:?}; collision {:?}",logic.prop_collision_pose.get(&id));
        }
        let mut director = crate::source_director::Director::load(
            &mut assets,
            &path,
            crate::campaign_settings::Difficulty::Normal,
        )
        .unwrap();
        pump(&mut logic, &mut director, 60);
        assert!(
            logic.world.cine.rescue_vehicle_nav_unblocked,
            "Native map script navigation binding must execute"
        );
        logic
            .world
            .queue_input("relay_explain_finale", "Trigger", "", 0., None);
        logic
            .world
            .queue_input("trigger_finale", "UseFinale", "", 0., None);
        pump(&mut logic, &mut director, 60);
        assert_eq!(
            logic.world.scavenge_progress(),
            Some((0, 8)),
            "Native single-player script must set eight cans"
        );
        let escape = logic.world.find("trigger_escape")[0];
        assert!(matches!(
            logic.world.ents[escape].kind,
            Kind::Trigger { disabled: true, .. }
        ));
        let target = logic.world.find("pour_target")[0];
        let point = Vec3::from_array(crate::binary::source_position(
            logic
                .world
                .world_matrix(target)
                .w_axis
                .truncate()
                .to_array(),
        ));
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let mut positions=vec![];
        for radius in [1.1,1.4,1.55] { for angle in 0..24 {
            let a=std::f32::consts::TAU*angle as f32/24.;
            let candidate=point+Vec3::new(a.sin()*radius,-crate::collision::EYE,a.cos()*radius);
            let Ok(feet)=collision.supported_spawn_near(candidate,crate::collision::HEIGHT) else {continue};
            let eye=feet+Vec3::Y*crate::collision::EYE;
            let trace=collision.trace(eye,point,Vec3::ZERO);
            if eye.distance(point)<=GAS_RANGE && !trace.start_solid && trace.fraction>=0.99 && collision.clear_hull(feet,crate::collision::HEIGHT) {
                positions.push(feet);
            }
        }}
        let feet=*positions.first().expect("A standing survivor must be able to reach the native fuel target");
        let mut body=crate::game::Body::at(feet);
        let dir=(point-feet-Vec3::Y*body.stance.eye()).normalize();
        body.yaw=(-dir.x).atan2(-dir.z);body.pitch=dir.y.asin();
        println!("Fuel standing position: Source {:?}, angles {} {}",[feet.x/crate::collision::UNIT,-feet.z/crate::collision::UNIT,feet.y/crate::collision::UNIT],-body.pitch.to_degrees(),body.yaw.to_degrees()+90.);
        let mut state = crate::game_state::Match::new(&weapon);
        state.paused = false;
        state.slot = 6;
        state.carried = Some("weapon_gascan".into());
        let mut input = crate::game::Input::default();
        let mut delivery = Delivery::default();
        delivery.tick(
            &mut state, &mut input, true, &body, &mut logic, &collision, 0.5,
        );
        assert!(
            delivery.target.is_some(),
            "Fuel cap must be reachable using real map collision"
        );
        delivery.tick(
            &mut state, &mut input, false, &body, &mut logic, &collision, 0.1,
        );
        assert_eq!(state.carried.as_deref(), Some("weapon_gascan"));
        assert_eq!(logic.world.scavenge_progress(), Some((0, 8)));
        for can in 1..=8 {
            state.slot = 6;
            state.carried = Some("weapon_gascan".into());
            for _ in 0..120 {
                delivery.tick(
                    &mut state,
                    &mut input,
                    true,
                    &body,
                    &mut logic,
                    &collision,
                    1. / 60.,
                );
            }
            assert!(
                state.carried.is_none(),
                "Can {can} must be consumed after two seconds"
            );
            pump(&mut logic, &mut director, 10);
            assert_eq!(logic.world.scavenge_progress(), Some((can, 8)));
            if can < 8 {
                assert!(matches!(
                    logic.world.ents[escape].kind,
                    Kind::Trigger { disabled: true, .. }
                ));
            }
        }
        assert!(
            matches!(
                logic.world.ents[escape].kind,
                Kind::Trigger {
                    disabled: false,
                    ..
                }
            ),
            "Native GasCanPoured must trigger relay_car_ready on the eighth pour"
        );
        let platform = logic.world.find("prop_escape_platform")[0];
        assert_eq!(logic.world.ents[platform].me.kv_i32("glowstate"), Some(3));
        state.slot = 6;
        state.carried = Some("weapon_gascan".into());
        delivery.tick(
            &mut state, &mut input, true, &body, &mut logic, &collision, 3.,
        );
        assert!(
            state.carried.is_some(),
            "Full objective must not consume extra fuel"
        );
        assert_eq!(delivery.cues.iter().filter(|(_,cue)|*cue==0).count(),9);
        assert_eq!(delivery.cues.iter().filter(|(_,cue)|*cue==1).count(),1);
        assert_eq!(delivery.cues.iter().filter(|(_,cue)|*cue==2).count(),8);
        println!(
            "Atrium native objective: eight completed pours; escape trigger enabled and car highlighted"
        );
    }
}
#[cfg(test)]
mod whitaker_reach_tests {
    use super::*;
    #[test]
    fn whitaker_cola_delivers_while_looking_at_the_hatch_from_the_landing() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let weapon = crate::game_state::tests_weapon();
        // Standing on the landing in front of Whitaker's door, looking level
        // and at the lower flap, at 20-70 units.
        for (y, z) in [(-1995., 680.), (-2000., 650.), (-2030., 655.), (-2045., 680.)] {
            let feet = Vec3::from_array(crate::binary::source_position([-5372., y, 616.5]));
            let mut body = crate::game::Body::at(feet);
            let eye = feet + Vec3::Y * body.stance.eye();
            let look = Vec3::from_array(crate::binary::source_position([-5372., -1958., z]));
            let dir = (look - eye).normalize();
            body.yaw = (-dir.x).atan2(-dir.z);
            body.pitch = dir.y.asin();
            let mut state = crate::game_state::Match::new(&weapon);
            state.paused = false;
            state.slot = 6;
            state.carried = Some("weapon_cola_bottles".into());
            let mut delivery = Delivery::default();
            let mut input = crate::game::Input::default();
            delivery.tick(&mut state, &mut input, true, &body, &mut logic, &collision, 0.1);
            assert!(delivery.target.is_some(), "cola must target the hatch from y={y} looking at z={z}");
            delivery.cancel(&mut state, &mut logic.world);
        }
    }
    #[test]
    fn store_double_doors_open_together_and_cannot_be_shut() {
        use l4d2r_game_core::entities::{DoorState, Kind};
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let doors = logic.world.find("store_doors");
        assert_eq!(doors.len(), 2);
        let state = |logic: &crate::l4d2r_cinematic::MapLogic, id: usize| match &logic.world.ents[id].kind {
            Kind::Door(d) => d.state,
            _ => unreachable!(),
        };
        logic.world.use_ent(doors[0], None);
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        for &id in &doors {
            assert_eq!(state(&logic, id), DoorState::Open, "both store doors open from one use");
        }
        for &id in &doors {
            logic.world.use_ent(id, None);
        }
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        for &id in &doors {
            assert_eq!(state(&logic, id), DoorState::Open, "store doors cannot be shut");
        }
        // An ordinary "Use closes" door still toggles shut.
        let closable = (0..logic.world.ents.len())
            .find(|&i| {
                logic.world.ents[i].me.classname == "prop_door_rotating"
                    && logic.world.ents[i].me.spawnflags() & 8192 != 0
                    && logic.world.ents[i].me.spawnflags() & 2048 == 0
                    && logic.world.ents[i].me.kv("slavename").is_none()
                    && state(&logic, i) == DoorState::Closed
            })
            .unwrap();
        logic.world.use_ent(closable, None);
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        assert_eq!(state(&logic, closable), DoorState::Open);
        logic.world.use_ent(closable, None);
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        assert_eq!(state(&logic, closable), DoorState::Closed);
    }
}
