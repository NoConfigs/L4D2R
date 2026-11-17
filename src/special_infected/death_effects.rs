//! Bounded AI death emitters, independent of playable infected state and
//! corpse lifetime. Particle definitions are owned retail PCFs; timing here
//! follows the existing playable effects, not a complete native TE port.
use super::{ai::AiSpecial, SpecialClass};
use crate::{game::Enemy, game_state::Match, l4d2r_cinematic::MapLogic};
use bevy::prelude::*;
use l4d2r_game_core::{entities::{Ent, EntityWorld, Kind}, mapents::MapEnt};
use std::{collections::HashMap, sync::{Arc, Weak}};

const SLOTS: usize = 2;
fn definition(class: SpecialClass) -> Option<(&'static str, f32)> {
    match class {
        SpecialClass::Boomer => Some(("boomer_explode", 3.)),
        SpecialClass::Smoker => Some(("smoker_smokecloud", 0.2)),
        _ => None,
    }
}
fn name(class: SpecialClass, slot: usize) -> String {
    format!("__rewrite_ai_death_{}_{}", class.name(), slot)
}
fn attached_name(effect: &str, slot: usize) -> String {
    format!("__rewrite_ai_attached_{effect}_{slot}")
}
fn prepare_emitter(world: &mut EntityWorld, target: String, effect: &str) {
    if !world.find(&target).is_empty() { return; }
    let index = world.ents.len();
    world.add_runtime_entity(Ent {
        me: MapEnt { index, classname: "info_particle_system".into(), targetname: target.clone(),
            origin: l4d2r_math::Vec3::ZERO, angles: l4d2r_math::Vec3::ZERO, mdl: None, brush: None,
            ent: l4d2_formats::bsp::Entity(vec![("classname".into(), "info_particle_system".into()),
                ("targetname".into(), target), ("effect_name".into(), effect.into())]) },
        conns: vec![], kind: Kind::Particle { active: false, started: 0. }, alive: true, templated: false, parent: None,
    });
}
pub(crate) fn prepare(world: &mut EntityWorld) {
    for class in [SpecialClass::Boomer, SpecialClass::Smoker] {
        for slot in 0..SLOTS { prepare_emitter(world, name(class, slot), definition(class).unwrap().0); }
    }
    for (effect, slots) in [("boomer_chunk_smoke", 8), ("boomer_leg_smoke", 2)] {
        for slot in 0..slots { prepare_emitter(world, attached_name(effect, slot), effect); }
    }
    for slot in 0..ROCK_SLOTS { prepare_emitter(world, rock_name(slot), ROCK_FX); }
}
/// CTankRock::Detonate (server 103677f0) dispatches tank_rock_throw_impact
/// at the rock wherever it breaks: shot down, on a survivor or on the world.
const ROCK_FX: &str = "tank_rock_throw_impact";
const ROCK_SLOTS: usize = 3;
/// Emitter lifetime (s); the native TE runs the system to completion.
const ROCK_FX_TIME: f32 = 3.;
fn rock_name(slot: usize) -> String {
    format!("__rewrite_tank_rock_impact_{slot}")
}
static ROCK_IMPACTS: std::sync::Mutex<Vec<Vec3>> = std::sync::Mutex::new(Vec::new());
pub(crate) fn rock_impact(position: Vec3) {
    if let Ok(mut list) = ROCK_IMPACTS.lock() {
        if list.len() < 16 {
            list.push(position);
        }
    }
}
#[derive(Component)]
pub(crate) struct AttachedSmoke { emitter: usize, map: Weak<l4d2_formats::bsp::Bsp> }
#[derive(Resource, Default)]
pub(crate) struct Pool {
    attachments: HashMap<usize, Entity>,
    active: HashMap<usize, f32>,
    map: Option<Weak<l4d2_formats::bsp::Bsp>>,
    clock: f32,
}
impl Pool {
    fn advance(&mut self, world: &mut EntityWorld) {
        if !self.map.as_ref().is_some_and(|map| map.ptr_eq(&Arc::downgrade(&world.bsp)))
            || world.time < self.clock {
            self.active.clear();
            self.attachments.clear();
            self.map = Some(Arc::downgrade(&world.bsp));
        }
        self.clock = world.time;
        self.active.retain(|id, end| {
            // A same-tick restart may leave time at zero on both sides.
            // Rebuilt map emitters are inactive even if their ids are reused.
            if !world.ents.get(*id).is_some_and(|e| matches!(e.kind, Kind::Particle { active: true, .. })) { return false; }
            if world.time < *end { return true; }
            if let Some(e) = world.ents.get_mut(*id) {
                if let Kind::Particle { active, .. } = &mut e.kind { *active = false; }
            }
            false
        });
        self.attachments.retain(|id, _| self.active.contains_key(id));
    }
    pub(crate) fn attach(&mut self, world: &mut EntityWorld, effect: &str, owner: Entity, position: Vec3) -> Option<AttachedSmoke> {
        self.advance(world);
        let slots = match effect { "boomer_chunk_smoke" => 8, "boomer_leg_smoke" => 2, _ => return None };
        let ids = (0..slots).filter_map(|slot| world.find(&attached_name(effect, slot)).first().copied()).collect::<Vec<_>>();
        let id = ids.iter().copied().find(|id| !self.active.contains_key(id))
            .or_else(|| ids.iter().copied().min_by(|a,b| self.active[a].total_cmp(&self.active[b])))?;
        world.ents[id].me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / crate::collision::UNIT;
        world.ents[id].kind = Kind::Particle { active: true, started: world.time };
        // Bounded by the rewrite fragment lifetime; native fade scheduling remains open.
        self.active.insert(id, world.time + 10.);
        self.attachments.insert(id, owner);
        Some(AttachedSmoke { emitter: id, map: Arc::downgrade(&world.bsp) })
    }
    fn start_rock(&mut self, world: &mut EntityWorld, position: Vec3) -> Option<usize> {
        let ids = (0..ROCK_SLOTS).filter_map(|slot| world.find(&rock_name(slot)).first().copied()).collect::<Vec<_>>();
        let id = ids.iter().copied().find(|id| !self.active.contains_key(id))
            .or_else(|| ids.iter().copied().min_by(|a,b| self.active[a].total_cmp(&self.active[b])))?;
        world.ents[id].me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / crate::collision::UNIT;
        world.ents[id].kind = Kind::Particle { active: true, started: world.time };
        self.active.insert(id, world.time + ROCK_FX_TIME);
        Some(id)
    }
    fn start(&mut self, world: &mut EntityWorld, class: SpecialClass, feet: Vec3) -> Option<usize> {
        let (_, duration) = definition(class)?;
        let ids = (0..SLOTS).filter_map(|slot| world.find(&name(class, slot)).first().copied()).collect::<Vec<_>>();
        let id = ids.iter().copied().find(|id| !self.active.contains_key(id))
            .or_else(|| ids.iter().copied().min_by(|a,b| self.active[a].total_cmp(&self.active[b])))?;
        let position = feet + if class == SpecialClass::Boomer { Vec3::Y * crate::collision::HEIGHT * 0.5 } else { Vec3::ZERO };
        world.ents[id].me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / crate::collision::UNIT;
        world.ents[id].kind = Kind::Particle { active: true, started: world.time };
        self.active.insert(id, world.time + duration);
        Some(id)
    }
}
#[derive(Component)]
struct Played;
pub(crate) fn install(app: &mut App) {
    app.init_resource::<Pool>().add_systems(FixedUpdate, tick.after(super::ai::tick)
        .run_if(crate::game_loading::visible))
        .add_systems(PostUpdate, attached_smoke.after(crate::infected_gore::GoreUpdate)
            .before(crate::source_particles::update)
            .before(bevy::transform::TransformSystems::Propagate));
}
fn tick(mut commands: Commands, state: Res<Match>, mut pool: ResMut<Pool>,
    logic: Option<ResMut<MapLogic>>, dead: Query<(Entity, &Enemy, &AiSpecial, &Transform), Without<Played>>) {
    let Some(mut logic) = logic else { return };
    if state.paused || logic.active { return; }
    pool.advance(&mut logic.world);
    let impacts = ROCK_IMPACTS.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default();
    for position in impacts {
        let emitter = pool.start_rock(&mut logic.world, position);
        if std::env::args().any(|a| a == "--capture-ai-special") {
            println!("Tank rock impact effect emitter={emitter:?} at={position:?}");
        }
    }
    for (id, enemy, ai, transform) in &dead {
        if enemy.health > 0. || definition(ai.class).is_none() { continue; }
        if let Some(emitter) = pool.start(&mut logic.world, ai.class, transform.translation) {
            commands.entity(id).insert(Played);
            if std::env::args().any(|a| a == "--capture-ai-special") {
                println!("AI death effect: {} emitter={emitter} at={:?}", ai.class.name(), transform.translation);
            }
        }
    }
}

fn attached_smoke(state: Res<Match>, mut pool: ResMut<Pool>, logic: Option<ResMut<MapLogic>>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>, playback: Res<crate::l4d2r_animation::PlaybackState>,
    owners: Query<(Entity, &AttachedSmoke, &Transform, Option<&Enemy>)>) {
    let Some(mut logic) = logic else { return };
    if state.paused || logic.active { return; }
    pool.advance(&mut logic.world);
    let map = Arc::downgrade(&logic.world.bsp);
    let detached = pool.attachments.iter().filter_map(|(&id, &owner)| {
        let valid = owners.get(owner).is_ok_and(|(_, smoke, _, _)| smoke.emitter == id && smoke.map.ptr_eq(&map));
        (!valid).then_some(id)
    }).collect::<Vec<_>>();
    for id in detached {
        pool.attachments.remove(&id); pool.active.remove(&id);
        if let Some(e) = logic.world.ents.get_mut(id) {
            if let Kind::Particle { active, .. } = &mut e.kind { *active = false; }
        }
    }
    for (owner, smoke, transform, enemy) in &owners {
        if !smoke.map.ptr_eq(&map) || pool.attachments.get(&smoke.emitter) != Some(&owner) { continue; }
        let position = if enemy.is_some() { library.corpse_center(owner, &playback).unwrap_or(transform.translation) }
            else { transform.translation };
        if let Some(e) = logic.world.ents.get_mut(smoke.emitter) {
            e.me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / crate::collision::UNIT;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    fn load_logic() -> MapLogic {
        MapLogic::load(std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2"),
            std::path::Path::new("maps/rewrite_lab.bsp"), false).unwrap()
    }
    #[test]
    fn attached_boomer_smoke_tracks_owners_and_cannot_be_stopped_by_replaced_lease() {
        let mut world = World::new();
        world.insert_resource(load_logic()); world.init_resource::<Pool>();
        world.init_resource::<crate::l4d2r_animation::AnimationLibrary>();
        world.init_resource::<crate::l4d2r_animation::PlaybackState>();
        let mut state = Match::new(&crate::game_state::tests_weapon()); state.paused = false;
        world.insert_resource(state);
        let mut owners = Vec::new();
        for index in 0..9 {
            let owner = world.spawn(Transform::from_translation(Vec3::X * index as f32)).id();
            world.resource_scope(|world, mut pool: Mut<Pool>| {
                let smoke = pool.attach(&mut world.resource_mut::<MapLogic>().world,
                    "boomer_chunk_smoke", owner, Vec3::ZERO).unwrap();
                world.entity_mut(owner).insert(smoke);
            });
            owners.push(owner);
            world.resource_mut::<MapLogic>().world.time += 0.01;
        }
        assert_eq!(world.resource::<Pool>().attachments.len(), 8);
        let last = owners[8];
        let emitter = world.get::<AttachedSmoke>(last).unwrap().emitter;
        assert_eq!(world.get::<AttachedSmoke>(owners[0]).unwrap().emitter, emitter);
        world.despawn(owners[0]);
        world.run_system_once(attached_smoke).unwrap();
        let origin = world.resource::<MapLogic>().world.ents[emitter].me.origin;
        assert!((origin.x * crate::collision::UNIT - 8.).abs() < 1e-5);
        assert_eq!(world.resource::<Pool>().attachments[&emitter], last);
        world.despawn(last); world.run_system_once(attached_smoke).unwrap();
        assert!(!world.resource::<Pool>().active.contains_key(&emitter));
        assert!(matches!(world.resource::<MapLogic>().world.ents[emitter].kind, Kind::Particle { active: false, .. }));
        world.resource_mut::<MapLogic>().restart();
        world.run_system_once(attached_smoke).unwrap();
        assert!(world.resource::<Pool>().attachments.is_empty());
    }
    #[test]
    fn pooled_deaths_are_independent_bounded_and_expire_without_stopping_each_other() {
        let mut logic = load_logic();
        let count = logic.world.ents.len();
        prepare(&mut logic.world); prepare(&mut logic.world);
        assert_eq!(logic.world.ents.len(), count);
        let mut pool = Pool::default();
        pool.advance(&mut logic.world);
        let first = pool.start(&mut logic.world, SpecialClass::Smoker, Vec3::X).unwrap();
        logic.world.time = 0.1;
        let second = pool.start(&mut logic.world, SpecialClass::Smoker, Vec3::Z).unwrap();
        assert_ne!(first, second);
        assert_ne!(logic.world.ents[first].me.origin, logic.world.ents[second].me.origin);
        logic.world.time = 0.21; pool.advance(&mut logic.world);
        assert!(matches!(logic.world.ents[first].kind, Kind::Particle { active: false, .. }));
        assert!(matches!(logic.world.ents[second].kind, Kind::Particle { active: true, .. }));
        for _ in 0..30 { pool.start(&mut logic.world, SpecialClass::Boomer, Vec3::ZERO); }
        assert_eq!(logic.world.ents.len(), count);
        assert!(pool.active.len() <= 3);
        logic.world.time = 4.; pool.advance(&mut logic.world);
        assert!(pool.active.is_empty());
        pool.start(&mut logic.world, SpecialClass::Smoker, Vec3::ONE);
        logic.restart(); pool.advance(&mut logic.world);
        assert!(pool.active.is_empty());
        assert_eq!(logic.world.time, 0.);
        pool.start(&mut logic.world, SpecialClass::Boomer, Vec3::ONE);
        logic.restart(); pool.advance(&mut logic.world);
        assert!(pool.active.is_empty(), "zero-time restart must clear reused ids too");
        let mut next = load_logic(); pool.advance(&mut next.world);
        assert!(pool.active.is_empty());
    }
    #[test]
    fn ai_death_emitter_starts_once_and_expiry_is_not_tied_to_corpse_entity() {
        let mut world = World::new();
        world.insert_resource(load_logic());
        world.init_resource::<Pool>();
        let mut state = Match::new(&crate::game_state::tests_weapon()); state.paused = false;
        world.insert_resource(state);
        let id = world.spawn((Enemy { model: String::new(), death_loot: None, loot_dropped: false,
            death: Default::default(), movement_speed: 0., reaction_serial: 0, attack_serial: 0,
            health: 0., attack: 0., reaction: 0, corpse_time: 12., burning: 0., vertical: 0. },
            AiSpecial::new(SpecialClass::Smoker), Transform::from_translation(Vec3::X))).id();
        world.run_system_once(tick).unwrap();
        assert!(world.get::<Played>(id).is_some());
        assert_eq!(world.resource::<Pool>().active.len(), 1);
        world.resource_mut::<MapLogic>().world.time = 0.1;
        world.run_system_once(tick).unwrap();
        assert_eq!(world.resource::<Pool>().active.len(), 1);
        world.despawn(id);
        world.resource_mut::<MapLogic>().world.time = 1.;
        world.run_system_once(tick).unwrap();
        assert!(world.resource::<Pool>().active.is_empty());
    }
}

#[cfg(test)]
mod rock_effect_tests {
    #[test]
    fn owned_tank_rock_impact_effect_and_music_cues_exist() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let manifest = String::from_utf8_lossy(&assets.read("particles/particles_manifest.txt").unwrap()).to_lowercase();
        let found = manifest
            .split('"')
            .filter(|s| s.ends_with(".pcf"))
            .filter_map(|p| assets.read(p.trim_start_matches('!')).ok())
            .filter_map(|bytes| crate::source_pcf::parse(&bytes).ok())
            .any(|pcf| pcf.iter().any(|e| e.kind == "DmeParticleSystemDefinition" && e.name == super::ROCK_FX));
        assert!(found, "{} in a manifest PCF", super::ROCK_FX);
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for cue in [
            "event.bleedingout", "event.bleedingouthit", "event.bleedingoutend", "event.survivordeath",
            "event.survivordeathhit", "event.vomitintheface", "event.boomerhit", "event.saferoom",
            "event.scenariolose", "event.scenariowin", "event.hunteralertclose", "event.chargeralertfar",
            "event.bleedingout_l4d1", "event.survivordeath_l4d1", "event.saferoom_l4d1",
            "event.scenariowin_l4d1", "event.scenariolose_l4d1",
        ] {
            assert!(registry.definitions.contains_key(cue), "{cue}");
        }
    }
}
