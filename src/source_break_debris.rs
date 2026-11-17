//! Authored PHY `break` pieces. Rendering/data are native; motion uses the local prop solver.
use crate::{kv, source_assets::SourceAssets, source_breakables::DoorModel};
use bevy::prelude::*;
use l4d2r_game_core::{
    entities::{Ent, EntityWorld, Kind},
    mapents::MapEnt,
};
use std::collections::{BTreeSet, HashMap, HashSet};
const GLASS: &str = "__l4d2r_glass_fragments";

#[derive(Clone, Debug)]
pub struct Piece {
    pub model: String,
    pub offset: l4d2r_math::Vec3,
    pub fade: f32,
    pub burst: f32,
    pub disabled: bool,
    pub debris: bool,
}
fn model_path(path: &str) -> String {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    format!(
        "{}{}.mdl",
        if path.starts_with("models/") {
            ""
        } else {
            "models/"
        },
        path.trim_end_matches(".mdl")
    )
}
/// Unlike a map of children, preserve every repeated top-level `break` block.
pub fn parse(text: &str) -> Vec<Piece> {
    let tokens = kv::tokens(text);
    let (mut i, mut result) = (0, Vec::new());
    while i + 1 < tokens.len() {
        let key = &tokens[i];
        i += 1;
        if tokens[i] != "{" {
            i += 1;
            continue;
        }
        i += 1;
        let start = i;
        let mut depth = 1;
        while i < tokens.len() && depth > 0 {
            if tokens[i] == "{" {
                depth += 1;
            }
            if tokens[i] == "}" {
                depth -= 1;
            }
            i += 1;
        }
        if !key.eq_ignore_ascii_case("break") || depth != 0 {
            continue;
        }
        let values = tokens[start..i - 1]
            .chunks_exact(2)
            .map(|p| (p[0].to_ascii_lowercase(), p[1].clone()))
            .collect::<HashMap<_, _>>();
        let Some(model) = values.get("model") else {
            continue;
        };
        // Ragdoll and bone/attachment placement need the native articulated path.
        if values.contains_key("placementbone") || values.contains_key("placementattachment") {
            continue;
        }
        let number = |key: &str, default| {
            values
                .get(key)
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|n| n.is_finite())
                .unwrap_or(default)
        };
        result.push(Piece {
            model: model_path(model),
            offset: l4d2r_math::Vec3::from_array(kv::vector(values.get("offset"))),
            fade: number("fadetime", 0.).max(0.),
            burst: number("burst", 0.).max(0.),
            // Native parser uses key presence for motiondisabled, regardless of value.
            disabled: values.contains_key("motiondisabled"),
            debris: number("debris", 1.) >= 1.,
        });
    }
    result
}
#[derive(Default)]
pub struct Debris {
    pub definitions: HashMap<String, Vec<Piece>>,
    models: HashMap<String, DoorModel>,
    bodies: HashMap<String, crate::prop_physics::Body>,
    pub pending: Vec<(usize, String)>,
    pub active: HashMap<usize, f32>,
    slots: Vec<usize>,
    free: Vec<usize>,
    pub generation: u64,
}
fn entity(model: String) -> Ent {
    Ent {
        me: MapEnt {
            index: 0,
            classname: "prop_physics".into(),
            targetname: String::new(),
            mdl: Some(model),
            origin: l4d2r_math::Vec3::ZERO,
            angles: l4d2r_math::Vec3::ZERO,
            brush: None,
            ent: l4d2_formats::bsp::Entity(vec![]),
        },
        conns: vec![],
        kind: Kind::Brush {
            enabled: true,
            solid: true,
        },
        alive: true,
        templated: false,
        parent: None,
    }
}
impl Debris {
    pub fn load(
        assets: &mut SourceAssets,
        world: &EntityWorld,
        doors: &HashMap<usize, crate::source_breakables::DoorDamage>,
    ) -> Self {
        let mut result = Self::default();
        let sources = world
            .durability
            .keys()
            .filter_map(|id| world.ents[*id].me.mdl.clone())
            .chain(
                doors
                    .values()
                    .flat_map(|p| p.stages.iter().map(|s| s.path.clone())),
            )
            .collect::<BTreeSet<_>>();
        for model in sources {
            let pieces = assets
                .read(&model.replace(".mdl", ".phy"))
                .ok()
                .and_then(|b| crate::physics_data::text(&b).ok().map(parse))
                .unwrap_or_default();
            for p in &pieces {
                if result.models.contains_key(&p.model) {
                    continue;
                }
                if let Some(mut model) = crate::source_breakables::door_model(assets, &p.model) {
                    // Fragment MDL header bounds may include the intact model's
                    // empty origin region. Only authored PHY geometry supports a body.
                    model.bounds = model.shapes.iter().fold(
                        (
                            l4d2r_math::Vec3::splat(f32::INFINITY),
                            l4d2r_math::Vec3::splat(f32::NEG_INFINITY),
                        ),
                        |(lo, hi), shape| (lo.min(shape.mins), hi.max(shape.maxs)),
                    );
                    result.models.insert(p.model.clone(), model);
                }
            }
            result.definitions.insert(model, pieces);
        }
        if world.ents.iter().any(|e| matches!(e.me.classname.as_str(), "func_breakable" | "func_breakable_surf")) {
            let mut glass = vec![];
            for index in 1..=6 {
                let path = format!("models/gibs/glass_shard{index:02}.mdl");
                if let Some(mut model) = crate::source_breakables::door_model(assets, &path) {
                    model.bounds = model.shapes.iter().fold(
                        (l4d2r_math::Vec3::splat(f32::INFINITY), l4d2r_math::Vec3::splat(f32::NEG_INFINITY)),
                        |(lo, hi), shape| (lo.min(shape.mins), hi.max(shape.maxs)));
                    result.models.insert(path.clone(), model);
                    glass.push(Piece { model: path, offset: l4d2r_math::Vec3::ZERO,
                        fade: 10., burst: 60., disabled: false, debris: true });
                }
            }
            result.definitions.insert(GLASS.into(), glass);
        }
        let paths = result.models.keys().cloned().collect::<BTreeSet<_>>();
        let entries = paths.iter().map(|p| entity(p.clone()).me).collect();
        let template_world = EntityWorld::new(world.bsp.clone(), entries, 1);
        let templates = crate::prop_physics::Props::load(&template_world, assets);
        for (id, body) in templates.0 {
            result
                .bodies
                .insert(template_world.ents[id].me.mdl.clone().unwrap(), body);
        }
        result
    }
    pub fn render_models(&self) -> Vec<String> {
        self.models
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}
/// Called once before the map event queue is cleared; stage changes enqueue the old model.
pub fn update(logic: &mut crate::l4d2r_cinematic::MapLogic) -> bool {
    let mut state = std::mem::take(&mut logic.break_debris);
    let mut changed = false;
    let expired = state
        .active
        .iter()
        .filter(|(_, end)| **end <= logic.world.time)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    for id in expired {
        logic.world.ents[id].alive = false;
        logic.physics.0.remove(&id);
        logic.prop_bounds.remove(&id);
        logic.prop_shapes.remove(&id);
        state.active.remove(&id);
        state.free.push(id);
        changed = true;
    }
    let broken = logic
        .world
        .events
        .iter()
        .filter_map(|event| match event {
            l4d2r_game_core::entities::EntEvent::Broken { ent } => Some(*ent),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut requests = vec![];
    for id in broken {
        if let Some(model) = logic.world.ents[id].me.mdl.clone() {
            state.pending.push((id, model));
        } else if logic.world.ents[id].me.classname == "func_breakable"
            && logic.world.ents[id].me.kv_i32("material").unwrap_or(0) == 0 {
            if let Some((model, glass)) = logic.world.ents[id].me.brush
                .and_then(|m| logic.world.bsp.models.get(m)).zip(state.definitions.get(GLASS)) {
                let center = (model.mins + model.maxs) * 0.5;
                let size = model.maxs - model.mins;
                let mut axes = [0, 1, 2];
                axes.sort_by(|&a, &b| size[b].total_cmp(&size[a]));
                let pieces = glass.iter().enumerate().map(|(i, p)| {
                    let mut p = p.clone(); p.offset = center;
                    p.offset[axes[0]] += (i as f32 / 5. - 0.5) * size[axes[0]] * 0.7;
                    p.offset[axes[1]] += if i % 2 == 0 { -0.2 } else { 0.2 } * size[axes[1]];
                    p
                }).collect::<Vec<_>>();
                requests.push((id, pieces));
            }
        }
    }
    for (owner, model) in std::mem::take(&mut state.pending) {
        if let Some(pieces) = state.definitions.get(&model) { requests.push((owner, pieces.clone())); }
    }
    for (&owner, surface) in &mut logic.break_surfaces {
        for (index, point) in surface.released.drain(..).enumerate() {
            if let Some(pieces) = state.definitions.get(GLASS).filter(|p| !p.is_empty()) {
                let mut piece = pieces[index % pieces.len()].clone();
                piece.offset = point;
                requests.push((owner, vec![piece]));
            }
        }
    }
    for (owner, pieces) in requests {
        let matrix = logic.world.world_matrix(owner);
        let transform = crate::l4d2r_cinematic::transform(matrix);
        let source_quat = l4d2r_math::Quat::from_mat4(&matrix);
        let (yaw, pitch, roll) = source_quat.to_euler(l4d2r_math::EulerRot::ZYX);
        let inherited = logic
            .physics
            .0
            .get(&owner)
            .map_or(Vec3::ZERO, |b| b.velocity);
        for p in &pieces {
            if p.model.starts_with("models/gibs/glass_shard") && state.active.keys()
                .filter(|id| logic.world.ents[**id].me.mdl.as_deref().is_some_and(|m| m.starts_with("models/gibs/glass_shard")))
                .count() >= 24 { continue; }
            // Bounded active pool: repeated map events cannot accumulate unlimited bodies/draws.
            if state.active.len() >= 128 {
                break;
            }
            let (Some(geometry), Some(template)) =
                (state.models.get(&p.model), state.bodies.get(&p.model))
            else {
                continue;
            };
            let mut ent = entity(p.model.clone());
            ent.me.origin = matrix.transform_point3(p.offset);
            ent.me.angles = l4d2r_math::Vec3::new(pitch, yaw, roll) * (180. / std::f32::consts::PI);
            ent.me
                .ent
                .0
                .push(("spawnflags".into(), if p.debris { "4" } else { "0" }.into()));
            let id = if let Some(id) = state.free.pop() {
                ent.me.index = id;
                logic.world.ents[id] = ent;
                id
            } else {
                let id = logic.world.add_runtime_entity(ent);
                state.slots.push(id);
                id
            };
            let center = Vec3::from_array(crate::binary::source_position(
                ((geometry.bounds.0 + geometry.bounds.1) * 0.5).to_array(),
            ));
            let radial = transform.rotation
                * (center + Vec3::from_array(crate::binary::source_position(p.offset.to_array())));
            let mut body = template.clone();
            body.launch_debris(
                inherited + radial.normalize_or_zero() * p.burst * crate::collision::UNIT,
                // Independent, bounded tumble per piece. Native random-stream
                // parity remains separate; never spawn every fragment with
                // zero angular velocity and freeze its first floor contact.
                Vec3::new((id as f32 * 1.7).sin(), 0.3, (id as f32 * 2.3).cos())
                    .normalize_or_zero() * std::f32::consts::PI,
                p.disabled,
            );
            logic.physics.0.insert(id, body);
            logic.prop_bounds.insert(id, geometry.bounds);
            logic.prop_shapes.insert(id, geometry.shapes.clone());
            state.active.insert(
                id,
                if p.fade > 0. {
                    logic.world.time + p.fade
                } else {
                    f32::INFINITY
                },
            );
            changed = true;
        }
    }
    logic.break_debris = state;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_door_fragments_tumble_and_land_on_their_broad_faces() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &root.join("left4dead2/maps/c1m1_hotel.bsp"), false).unwrap();
        let owner = logic.world.ents.iter().position(|e| e.me.kv("hammerid") == Some("330642")).unwrap();
        logic.world.damage_entity(owner, 601., l4d2r_game_core::entities::DamageKind::Bullet);
        crate::source_breakables::sync_doors(&mut logic);
        update(&mut logic);
        let ids = logic.break_debris.active.keys().copied().collect::<HashSet<_>>();
        assert!(!ids.is_empty());
        logic.physics.0.retain(|id, _| ids.contains(id));
        for &id in &ids {
            logic.world.ents[id].me.origin -= l4d2r_math::Vec3::new(1828., 6820., 2624.);
        }
        let floor = crate::collision::Collision::test_floor();
        let mut physics = std::mem::take(&mut logic.physics);
        for _ in 0..480 { physics.tick(&mut logic.world, &logic.prop_bounds, &floor, 1./60.); }
        let mut broad_pieces = 0;
        for &id in &ids {
            let (lo, hi) = logic.prop_bounds[&id];
            let span = hi - lo;
            let world = logic.world.world_matrix(id);
            let mut low = f32::INFINITY; let mut high = f32::NEG_INFINITY;
            for shape in logic.prop_shapes[&id].iter() { for p in shape.triangles.iter().flatten() {
                let p = world.transform_point3(*p); low = low.min(p.z); high = high.max(p.z);
            }}
            assert!(low > -1., "fragment fell through floor: {low}");
            println!("fragment {}: span={span:?}, resting height={}, sleeping={}",
                logic.world.ents[id].me.mdl.as_deref().unwrap(), high - low, physics.0[&id].asleep);
            // Thick corner chunks can rest on an angled face. The large,
            // thin door panel is the piece that must not remain upright.
            if span.max_element() < 24. || span.min_element() * 4. > span.max_element() { continue; }
            broad_pieces += 1;
            assert!(high - low < span.max_element() * 0.35 + 1.,
                "fragment still standing: span={span:?}, height={}", high - low);
        }
        assert!(broad_pieces >= 1, "fixture must exercise an actual large door panel");
    }
    #[test]
    fn lab_window_releases_bounded_owned_glass_bodies_once() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, std::path::Path::new("maps/rewrite_lab.bsp"), true).unwrap();
        let id = logic.world.find("lab_glass")[0];
        logic.world.damage_breakable(id, 10000.);
        update(&mut logic);
        let count = logic.break_debris.active.len();
        assert_eq!(count, 6);
        assert!(logic.break_debris.active.keys().all(|id| logic.physics.0.contains_key(id)));
        logic.world.events.clear();
        assert!(!update(&mut logic));
        assert_eq!(logic.break_debris.active.len(), count);
        logic.world.time += 11.; update(&mut logic);
        assert!(logic.break_debris.active.is_empty());
    }
    #[test]
    fn repeated_native_break_blocks_keep_order_offsets_and_flags() {
        let pieces = parse(
            r#"solid { "index" "0" } break { "model" "Props_Doors\part_a" "offset" "1 2 3" "fadetime" "10" }
            break { "model" "models/props_doors/part_b.mdl" "motiondisabled" "0" "debris" "0" "burst" "20" }
            break { "ragdoll" "unsupported" } break { "model" "bone_piece" "placementbone" "handle" }"#,
        );
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].model, "models/props_doors/part_a.mdl");
        assert_eq!(pieces[0].offset, l4d2r_math::Vec3::new(1., 2., 3.));
        assert_eq!(pieces[0].fade, 10.);
        assert_eq!(pieces[1].model, "models/props_doors/part_b.mdl");
        assert!(pieces[1].disabled);
        assert!(!pieces[1].debris);
        assert_eq!(pieces[1].burst, 20.);
    }
    #[test]
    fn owned_hotel_damage_releases_authored_pieces_once_with_bounded_lifetime_and_restart() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(
            root,
            &root.join("left4dead2/maps/c1m1_hotel.bsp"),
            false,
        )
        .unwrap();
        let id = logic
            .world
            .ents
            .iter()
            .position(|e| e.me.kv("hammerid") == Some("330642"))
            .unwrap();
        let matrix = logic.world.world_matrix(id);
        logic.world.events.clear();
        logic
            .world
            .damage_entity(id, 601., l4d2r_game_core::entities::DamageKind::Bullet);
        crate::source_breakables::sync_doors(&mut logic);
        assert!(update(&mut logic));
        let spawned = logic
            .break_debris
            .active
            .keys()
            .copied()
            .collect::<Vec<_>>();
        assert!(spawned
            .iter()
            .any(|id| logic.world.ents[*id].me.mdl.as_deref()
                == Some("models/props_downtown/metal_door_112_dm02_02.mdl")));
        for fragment in &spawned {
            assert!(logic.physics.0.contains_key(fragment));
            assert_eq!(logic.world.world_matrix(*fragment), matrix);
            let geometry = &logic.prop_shapes[fragment];
            let bounds = geometry.iter().fold(
                (
                    l4d2r_math::Vec3::splat(f32::INFINITY),
                    l4d2r_math::Vec3::splat(f32::NEG_INFINITY),
                ),
                |(lo, hi), shape| (lo.min(shape.mins), hi.max(shape.maxs)),
            );
            assert_eq!(
                logic.prop_bounds[fragment], bounds,
                "empty MDL header region used as fragment support"
            );
        }
        let count = spawned.len();
        logic.world.events.clear();
        assert!(!update(&mut logic));
        assert_eq!(logic.break_debris.active.len(), count);
        let before = spawned
            .iter()
            .map(|id| {
                let (lo, hi) = logic.prop_bounds[id];
                (*id, logic.world.world_matrix(*id).transform_point3((lo + hi) * 0.5).z)
            })
            .collect::<HashMap<_, _>>();
        let mut physics = std::mem::take(&mut logic.physics);
        for _ in 0..10 {
            physics.tick(
                &mut logic.world,
                &logic.prop_bounds,
                &crate::collision::Collision::default(),
                1. / 60.,
            );
        }
        logic.physics = physics;
        for id in &spawned {
            assert!(
                { let (lo, hi) = logic.prop_bounds[id];
                    logic.world.world_matrix(*id).transform_point3((lo + hi) * 0.5).z < before[id] },
                "fragment centre of mass did not fall"
            );
        }
        logic.world.time += 11.;
        assert!(update(&mut logic));
        assert!(logic.break_debris.active.is_empty());
        assert!(spawned
            .iter()
            .all(|id| !logic.world.ents[*id].alive && !logic.physics.0.contains_key(id)));
        let source = logic.door_damage[&id].stages[1].path.clone();
        for _ in 0..2 {
            let old_len = logic.world.ents.len();
            logic
                .break_debris
                .pending
                .extend((0..200).map(|_| (id, source.clone())));
            assert!(update(&mut logic));
            assert_eq!(logic.break_debris.active.len(), 128);
            assert!(logic.world.ents.len() <= old_len + 128);
            assert_eq!(logic.break_debris.slots.len(), 128);
            logic.world.time += 11.;
            assert!(update(&mut logic));
            assert!(logic.break_debris.active.is_empty());
        }
        logic.restart();
        assert!(logic.break_debris.active.is_empty());
        assert!(spawned.iter().all(|id| !logic.physics.0.contains_key(id)));
    }
}
pub fn reset(logic: &mut crate::l4d2r_cinematic::MapLogic) {
    for id in logic.break_debris.slots.drain(..) {
        logic.physics.0.remove(&id);
        logic.prop_bounds.remove(&id);
        logic.prop_shapes.remove(&id);
    }
    logic.break_debris.active.clear();
    logic.break_debris.free.clear();
    logic.break_debris.pending.clear();
    logic.break_debris.generation += 1;
}
/// Opt-in real-game break/settle capture; ordinary sessions never run this.
pub(crate) fn capture(mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    state: Res<crate::game_state::Match>, mut fired: Local<bool>) {
    if *fired || state.paused || state.elapsed < 0.5 { return; }
    *fired = true;
    if let Some(id) = logic.world.ents.iter().position(|e| e.me.kv("hammerid") == Some("330642")) {
        logic.world.damage_entity(id, 601., l4d2r_game_core::entities::DamageKind::Bullet);
    } else if let Some(id) = logic.world.find("lab_glass").first().copied() {
        logic.world.damage_breakable(id, 10000.);
    }
}
#[derive(Component)]
pub struct DebrisDraw {
    owner: usize,
    generation: u64,
    model: String,
}
pub fn presentation(
    mut commands: Commands,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    models: Res<crate::viewer::RenderModels>,
    frontend: Res<crate::game_menu::Frontend>,
    draws: Query<(Entity, &DebrisDraw)>,
) {
    if !frontend.playing() {
        return;
    }
    let mut existing = HashSet::new();
    for (entity, draw) in &draws {
        let wanted = draw.generation == logic.break_debris.generation
            && logic.break_debris.active.contains_key(&draw.owner)
            && logic
                .world
                .ents
                .get(draw.owner)
                .is_some_and(|e| e.me.mdl.as_ref() == Some(&draw.model));
        if wanted {
            existing.insert(draw.owner);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for id in logic
        .break_debris
        .active
        .keys()
        .filter(|id| !existing.contains(id))
    {
        let Some(model) = logic.world.ents[*id].me.mdl.clone() else {
            continue;
        };
        let Some(parts) = models.0.get(&(model.clone(), 0, 0)) else {
            continue;
        };
        for (mesh, material) in parts {
            commands.spawn((
                crate::l4d2r_cinematic::MapEntity(*id),
                DebrisDraw {
                    owner: *id,
                    generation: logic.break_debris.generation,
                    model: model.clone(),
                },
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Visibility::Inherited,
                crate::l4d2r_cinematic::transform(logic.world.world_matrix(*id)),
                bevy::camera::visibility::RenderLayers::layer(1),
                crate::viewer::DynamicModelLighting::default(),
            ));
        }
    }
}
