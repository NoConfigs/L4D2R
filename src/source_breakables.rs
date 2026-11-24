//! Model-authored prop durability and owned break-sound events.
use crate::{binary, kv, source_assets::SourceAssets};
use l4d2r_game_core::entities::{Durability, EntityWorld, Kind};
use std::collections::{BTreeMap, HashMap};
use bevy::prelude::*;
use std::sync::Arc;

/// Owned server 100dd510: break sound by brush material enum.
pub fn material_sound(material: i32) -> &'static str {
    match material {
        0 => "Breakable.Glass", 1 => "Breakable.Crate", 2 => "Breakable.Metal",
        3 | 9 => "Breakable.Flesh", 4 | 8 => "Breakable.Concrete",
        5 => "Breakable.Ceiling", 6 => "Breakable.Computer", _ => "",
    }
}

#[derive(Clone)]
pub struct DoorModel {
    pub path: String,
    pub bounds: (l4d2r_math::Vec3, l4d2r_math::Vec3),
    pub shapes: Arc<Vec<l4d2_formats::phy::Convex>>,
    damage: [Option<f32>; 4],
}
#[derive(Clone)]
pub struct DoorDamage {
    original: DoorModel,
    pub stages: Vec<DoorModel>,
    pub pound: String,
    pub current: Option<usize>,
}
fn model_data(assets: &mut SourceAssets, path: &str) -> Option<kv::Block> {
    let bytes = assets.read(path).ok()?;
    let at = binary::u32_at(&bytes, 312).ok()? as usize;
    let len = binary::u32_at(&bytes, 316).ok()? as usize;
    let node = kv::blocks(&String::from_utf8_lossy(binary::range(&bytes, at, len).ok()?));
    Some(node.children.get("mdlkeyvalue").cloned().unwrap_or(node))
}
fn door_settings(node: &kv::Block, skin: i32) -> Option<&kv::Block> {
    let options = node.children.get("door_options")?;
    options.children.get(&format!("skin{skin}")).or_else(|| options.children.get("defaults"))
}
pub(crate) fn door_model(assets: &mut SourceAssets, path: &str) -> Option<DoorModel> {
    let bytes = assets.read(path).ok()?;
    let (lo, hi, _, _) = l4d2_formats::mdl::read_header_bounds(&bytes)?;
    let shapes = l4d2_formats::phy::parse_phy(&assets.read(&path.replace(".mdl", ".phy")).ok()?).ok()?
        .into_iter().flat_map(|s| s.convexes).collect::<Vec<_>>();
    if shapes.is_empty() { return None; }
    let data = model_data(assets, path);
    let prop = data.as_ref().and_then(|n| n.children.get("prop_data"));
    let damage = ["dmg.bullets", "dmg.club", "dmg.explosive", "dmg.fire"].map(|key| prop.and_then(|p| p.values.get(key))
        .and_then(|n| n.parse::<f32>().ok()).filter(|n| n.is_finite() && *n >= 0.));
    Some(DoorModel { path: path.into(), bounds: (l4d2r_math::Vec3::from_array(lo), l4d2r_math::Vec3::from_array(hi)), shapes: Arc::new(shapes), damage })
}
pub fn load_doors(assets: &mut SourceAssets, world: &EntityWorld) -> HashMap<usize, DoorDamage> {
    let mut result = HashMap::new();
    let mut cache = HashMap::<(String, i32), Option<DoorDamage>>::new();
    for (id, e) in world.ents.iter().enumerate() {
        if !matches!(e.kind, Kind::Door(_)) || !world.durability.contains_key(&id) { continue; }
        let Some(path) = &e.me.mdl else { continue; };
        let skin = e.me.kv_i32("skin").unwrap_or(0);
        let profile = cache.entry((path.clone(), skin)).or_insert_with(|| {
            let node = model_data(assets, path)?;
            let settings = door_settings(&node, skin)?;
            let mut stages = Vec::new();
            for index in 1..=32 {
                let Some(path) = settings.values.get(&format!("damage{index}")) else { break; };
                let path = path.replace('\\', "/").to_ascii_lowercase();
                let path = format!("{}{}.mdl", if path.starts_with("models/") { "" } else { "models/" }, path.trim_end_matches(".mdl"));
                stages.push(door_model(assets, &path)?);
            }
            if stages.is_empty() { return None; }
            Some(DoorDamage { original: door_model(assets, path)?, stages,
                pound: settings.values.get("pound").cloned().unwrap_or_else(|| "WoodenDoor.Pound".into()), current: None })
        });
        if let Some(profile) = profile { result.insert(id, profile.clone()); }
    }
    result
}
/// Owned server 101a37b0 uses integer max-health division and strict `<`.
pub fn damage_stage(maximum: f32, health: f32, count: usize) -> Option<usize> {
    if !maximum.is_finite() || !health.is_finite() || health <= 0. || count == 0 { return None; }
    let maximum = maximum as i32;
    let step = maximum / (count as i32 + 1);
    if step <= 0 { return None; }
    (0..count).take_while(|i| (health as i32) < maximum - step * (*i as i32 + 1)).last()
}
pub fn ray_shapes(start: l4d2r_math::Vec3, end: l4d2r_math::Vec3,
    shapes: &[l4d2_formats::phy::Convex]) -> Option<f32> {
    shapes.iter().filter_map(|shape| {
        if shape.planes.is_empty() { return None; }
        let (mut enter, mut exit) = (0f32, 1f32);
        for (normal, distance) in &shape.planes {
            let a = normal.dot(start) - distance;
            let b = normal.dot(end) - distance;
            if a > 0. && b > 0. { return None; }
            if a <= 0. && b <= 0. { continue; }
            let at = a / (a - b);
            if a > b { enter = enter.max(at); } else { exit = exit.min(at); }
            if enter > exit { return None; }
        }
        Some(enter)
    }).min_by(f32::total_cmp)
}
pub fn sync_doors(logic: &mut crate::l4d2r_cinematic::MapLogic) -> bool {
    let mut changed = false;
    for (&id, profile) in &mut logic.door_damage {
        let Some(d) = logic.world.durability.get_mut(&id) else { continue; };
        if !logic.world.ents[id].alive { continue; }
        let next = damage_stage(d.maximum, d.health, profile.stages.len());
        if next <= profile.current { continue; }
        // Native door transition releases the old model's break pieces at every
        // crossed stage, including stages skipped by one large hit.
        let first = profile.current.map_or(0, |s| s + 1);
        for stage in first..=next.unwrap() {
            let old = if stage == 0 { &profile.original.path } else { &profile.stages[stage-1].path };
            logic.break_debris.pending.push((id, old.clone()));
            let replacement = &profile.stages[stage];
            for (destination, value) in [&mut d.bullets, &mut d.club, &mut d.explosive, &mut d.fire].into_iter().zip(replacement.damage) {
                if let Some(value) = value { *destination = value; }
            }
        }
        profile.current = next;
        let model = &profile.stages[next.unwrap()];
        logic.world.ents[id].me.mdl = Some(model.path.clone());
        logic.prop_bounds.insert(id, model.bounds);
        logic.prop_shapes.insert(id, model.shapes.clone());
        changed = true;
    }
    changed
}
pub fn reset_doors(logic: &mut crate::l4d2r_cinematic::MapLogic) {
    for (&id, profile) in &mut logic.door_damage {
        profile.current = None;
        logic.prop_bounds.insert(id, profile.original.bounds);
        logic.prop_shapes.insert(id, profile.original.shapes.clone());
    }
}
#[derive(Component)]
pub struct DoorDamageDraw(usize);
pub fn presentation(
    mut commands: Commands,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    models: Res<crate::viewer::RenderModels>,
    frontend: Res<crate::game_menu::Frontend>,
    mut draws: Query<(Entity, &crate::l4d2r_cinematic::MapEntity, Option<&DoorDamageDraw>, &mut Visibility)>,
) {
    if !frontend.playing() { return; }
    let mut existing = std::collections::HashSet::new();
    for (entity, owner, stage, mut visible) in &mut draws {
        let Some(profile) = logic.door_damage.get(&owner.0) else { continue; };
        let wanted = profile.current.filter(|_| logic.world.ents[owner.0].alive);
        if let Some(stage) = stage {
            if wanted == Some(stage.0) { existing.insert(owner.0); }
            else { commands.entity(entity).despawn(); }
        } else if wanted.is_some() { *visible = Visibility::Hidden; }
    }
    for (&id, profile) in &logic.door_damage {
        let Some(stage) = profile.current.filter(|_| logic.world.ents[id].alive && !existing.contains(&id)) else { continue; };
        let Some(parts) = models.0.get(&(profile.stages[stage].path.clone(), 0, 0)) else { continue; };
        for (mesh, material) in parts {
            commands.spawn((crate::l4d2r_cinematic::MapEntity(id), DoorDamageDraw(stage),
                Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Visibility::Inherited,
                crate::l4d2r_cinematic::transform(logic.world.world_matrix(id)),
                bevy::camera::visibility::RenderLayers::layer(1), crate::viewer::DynamicModelLighting::default()));
        }
    }
}

fn inherited(
    name: &str,
    data: &BTreeMap<String, kv::Block>,
    depth: usize,
) -> BTreeMap<String, String> {
    if depth > 16 {
        return BTreeMap::new();
    }
    let Some(block) = data.get(&name.to_ascii_lowercase()) else {
        return BTreeMap::new();
    };
    let mut values = block
        .values
        .get("base")
        .map(|base| inherited(base, data, depth + 1))
        .unwrap_or_default();
    values.extend(block.values.clone());
    values
}

/// A model's prop_data health, its own or inherited from its propdata.txt
/// base (0: not breakable). Retail prop_dynamic refuses such models:
/// "prop_dynamic has a health specified in model ...".
pub fn model_health(assets: &mut SourceAssets, path: &str) -> f32 {
    let Some(node) = model_data(assets, path) else { return 0. };
    let Some(block) = node.children.get("prop_data") else { return 0. };
    let parse = |v: Option<&String>| v.and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite());
    if let Some(h) = parse(block.values.get("health")) {
        return h;
    }
    let Some(base) = block.values.get("base") else { return 0. };
    let root = assets.read("scripts/propdata.txt").ok().map(|b| kv::blocks(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let data = root.children.get("propdata.txt").map(|b| b.children.clone()).unwrap_or_default();
    parse(inherited(base, &data, 0).get("health")).unwrap_or(0.)
}

pub fn configure(assets: &mut SourceAssets, world: &mut EntityWorld) -> HashMap<usize, Durability> {
    let root = assets
        .read("scripts/propdata.txt")
        .ok()
        .map(|b| kv::blocks(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let data = root
        .children
        .get("propdata.txt")
        .map(|b| &b.children)
        .cloned()
        .unwrap_or_default();
    let mut models: HashMap<String, Option<(kv::Block, String)>> = HashMap::new();
    let mut result = HashMap::new();
    for (id, entity) in world.ents.iter_mut().enumerate() {
        // ambient_generic uses health as volume; several other point entities also
        // have health keys with class-specific meanings. Never change their Kind.
        if !matches!(
            entity.me.classname.as_str(),
            "prop_physics"
                | "prop_physics_override"
                | "prop_physics_multiplayer"
                | "prop_dynamic"
                | "prop_dynamic_override"
                | "prop_door_rotating"
                | "func_breakable"
                | "func_breakable_surf"
                | "func_physbox"
                | "func_physbox_multiplayer"
        ) {
            continue;
        }

        if entity.me.classname == "prop_door_rotating_checkpoint"
            || entity.me.kv_i32("unbreakable") == Some(1)
        {
            continue;
        }
        let mut values = BTreeMap::new();
        let mut surface = String::new();
        let mut base = String::new();
        let mut damageable_door = false;
        if let Some(path) = &entity.me.mdl {
            let model = models.entry(path.clone()).or_insert_with(|| {
                let bytes = assets.read(path).ok()?;
                let at = binary::u32_at(&bytes, 312).ok()? as usize;
                let len = binary::u32_at(&bytes, 316).ok()? as usize;
                let node = kv::blocks(&String::from_utf8_lossy(
                    binary::range(&bytes, at, len).ok()?,
                ));
                let surface = binary::u32_at(&bytes, 308)
                    .ok()
                    .and_then(|at| binary::cstring(&bytes, at as usize).ok())
                    .unwrap_or_default();
                Some((
                    node.children.get("mdlkeyvalue").cloned().unwrap_or(node),
                    surface,
                ))
            });
            if let Some((node, prop)) = model {
                surface = prop.clone();
                if let Some(block) = node.children.get("prop_data") {
                    base = block.values.get("base").cloned().unwrap_or_default();
                    values = inherited(&base, &data, 0);
                    values.extend(block.values.clone());
                }
                if let Some(defaults) = door_settings(node, entity.me.kv_i32("skin").unwrap_or(0))
                {
                    surface = defaults
                        .values
                        .get("surfaceprop")
                        .cloned()
                        .unwrap_or(surface);
                    damageable_door = defaults.values.contains_key("damage1");
                }
            }
        }
        if matches!(entity.kind, Kind::Door(_)) && !damageable_door {
            continue;
        }
        let native = |key: &str, fallback: f32| {
            values
                .get(key)
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(fallback)
        };
        let health = entity
            .me
            .kv_f32("health")
            .filter(|n| *n > 0.)
            .unwrap_or_else(|| {
                native(
                    "health",
                    match entity.kind {
                        Kind::Breakable { health } => health,
                        _ => 0.,
                    },
                )
            });
        if health <= 0. {
            continue;
        }
        let glass = base.to_ascii_lowercase().starts_with("glass.")
            || surface.to_ascii_lowercase().contains("glass")
            || entity.me.kv_i32("material") == Some(0);
        let sound = entity
            .me
            .kv("breaksound")
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if entity.me.classname.starts_with("func_breakable") {
                    material_sound(entity.me.kv_i32("material").unwrap_or(0))
                } else if glass {
                    "Breakable.Glass"
                } else if surface.contains("metal") {
                    "Breakable.Metal"
                } else if surface.contains("concrete") {
                    "Breakable.Concrete"
                } else {
                    "Breakable.Crate"
                }
                .into()
            });
        let durability = Durability {
            health,
            maximum: health,
            bullets: native("dmg.bullets", 1.).max(0.),
            club: native("dmg.club", 1.).max(0.),
            explosive: native("dmg.explosive", 1.).max(0.),
            fire: native("dmg.fire", 1.).max(0.),
            sound,
        };
        if !matches!(entity.kind, Kind::Door(_)) {
            entity.kind = Kind::Breakable { health };
        }
        result.insert(id, durability);
    }
    world.durability = result.clone();
    println!("Native damageable props/doors: {}", result.len());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damaged_hotel_door_and_fragments_use_upright_authored_pose() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();
        for suffix in ["", "_dm01_01", "_dm02_01", "_dm03_01", "_dm04_01", "_dm02_02"] {
            let path=format!("models/props_downtown/metal_door_112{suffix}.mdl");
            let posed=crate::studio::load_posed_prop(&mut assets,&path).unwrap();
            let (lo,hi)=posed.iter().flat_map(|d|&d.positions).fold((Vec3::splat(f32::INFINITY),Vec3::splat(f32::NEG_INFINITY)),|(lo,hi),p|(lo.min(Vec3::from_array(*p)),hi.max(Vec3::from_array(*p))));
            assert!(hi.y-lo.y>0.2,"{path} lies flat: {lo:?} .. {hi:?}");
            if suffix.is_empty() || suffix.ends_with("_01") {
                assert!(hi.y-lo.y>2.0 && hi.x-lo.x<0.4,"retained door not upright: {path}: {lo:?} .. {hi:?}");
            }
        }
    }
    #[test]
    fn door_stage_draws_replace_original_once_and_cleanup_on_break_and_restart() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {return;}
        let logic=crate::l4d2r_cinematic::MapLogic::load(root,&root.join("left4dead2/maps/c1m1_hotel.bsp"),false).unwrap();
        let id=logic.world.ents.iter().position(|e|e.me.kv("hammerid")==Some("330642")).unwrap();
        let profiles=logic.door_damage[&id].stages.clone();
        let parts=vec![(Handle::default(),Handle::default());2];
        let models=crate::viewer::RenderModels(profiles.iter().map(|p|((p.path.clone(),0,0),parts.clone())).collect());
        let mut app=App::new();
        app.insert_resource(logic).insert_resource(models).insert_resource(crate::game_menu::Frontend{page:crate::game_menu::Page::Game,mode:0,restart:false,started:true}).add_systems(Update,presentation);
        let originals=(0..2).map(|_|app.world_mut().spawn((crate::l4d2r_cinematic::MapEntity(id),Visibility::Inherited,Transform::default())).id()).collect::<Vec<_>>();
        for stage in [Some(0),Some(0),Some(1),Some(1),None] {
            {let mut logic=app.world_mut().resource_mut::<crate::l4d2r_cinematic::MapLogic>(); logic.door_damage.get_mut(&id).unwrap().current=stage;}
            for original in &originals { *app.world_mut().get_mut::<Visibility>(*original).unwrap()=Visibility::Inherited; }
            app.update();
            let world=app.world_mut();
            assert_eq!(world.query::<&DoorDamageDraw>().iter(world).count(),if stage.is_some(){2}else{0},"duplicate/stale stage draw at {stage:?}");
            for original in &originals { assert_eq!(*world.get::<Visibility>(*original).unwrap(),if stage.is_some(){Visibility::Hidden}else{Visibility::Inherited}); }
        }
        {let mut logic=app.world_mut().resource_mut::<crate::l4d2r_cinematic::MapLogic>();logic.door_damage.get_mut(&id).unwrap().current=Some(0);}
        app.update();
        app.world_mut().resource_mut::<crate::l4d2r_cinematic::MapLogic>().world.ents[id].alive=false;
        app.update();
        let world=app.world_mut();assert_eq!(world.query::<&DoorDamageDraw>().iter(world).count(),0);
        world.resource_mut::<crate::l4d2r_cinematic::MapLogic>().restart();
        app.update();
        let world=app.world_mut();assert_eq!(world.query::<&DoorDamageDraw>().iter(world).count(),0);
    }
    #[test]
    fn native_door_thresholds_are_strict_and_material_sounds_are_distinct() {
        for (health, stage) in [(1000., None), (750., None), (749., Some(0)),
            (500., Some(0)), (499., Some(1)), (250., Some(1)), (249., Some(2))] {
            assert_eq!(damage_stage(1000., health, 3), stage, "{health}");
        }
        assert_eq!(damage_stage(0., 1., 3), None);
        assert_eq!(material_sound(7), "");
        assert_eq!(material_sound(5), "Breakable.Ceiling");
        assert_eq!(material_sound(6), "Breakable.Computer");
        assert_eq!(material_sound(9), "Breakable.Flesh");
        let node = kv::blocks(r#"door_options { defaults { damage1 "default" } skin2 { damage1 "alternate" } }"#);
        assert_eq!(door_settings(&node, 2).unwrap().values["damage1"], "alternate");
        assert_eq!(door_settings(&node, 3).unwrap().values["damage1"], "default");
    }
    #[test]
    fn owned_hotel_doors_change_authored_geometry_and_restart_restores_it() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &root.join("left4dead2/maps/c1m1_hotel.bsp"), true).unwrap();
        let id = *logic.door_damage.iter().find(|(_, p)| !p.stages.is_empty()).expect("no native staged doors").0;
        let original = logic.world.ents[id].me.mdl.clone();
        let bounds = logic.prop_bounds[&id];
        let maximum = logic.world.durability[&id].maximum;
        let pose = logic.world.world_matrix(id);
        let count = logic.door_damage[&id].stages.len();
        let step = maximum as i32 / (count as i32 + 1);
        let original_shapes = logic.prop_shapes[&id].clone();
        let mut holes = 0;
        for stage in 0..count {
            let health = logic.world.durability[&id].health;
            let remaining = maximum - step as f32 * (stage + 1) as f32 - 1.;
            logic.world.damage_entity(id, health - remaining, l4d2r_game_core::entities::DamageKind::Bullet);
            assert!(sync_doors(&mut logic));
            assert_eq!(logic.door_damage[&id].current, Some(stage));
            assert_eq!(logic.world.ents[id].me.mdl.as_deref(), Some(logic.door_damage[&id].stages[stage].path.as_str()));
            assert!(Arc::ptr_eq(&logic.prop_shapes[&id], &logic.door_damage[&id].stages[stage].shapes));
            assert_eq!(logic.world.world_matrix(id), pose);
            assert!(matches!(logic.world.ents[id].kind, Kind::Door(_)));
            let span = bounds.1 - bounds.0;
            let axis = (0..3).min_by(|a, b| span[*a].total_cmp(&span[*b])).unwrap();
            let other: Vec<_> = (0..3).filter(|a| *a != axis).collect();
            for u in 1..10 { for v in 1..10 {
                let mut a = (bounds.0 + bounds.1) * 0.5;
                a[other[0]] = bounds.0[other[0]] + span[other[0]] * u as f32 / 10.;
                a[other[1]] = bounds.0[other[1]] + span[other[1]] * v as f32 / 10.;
                a[axis] = bounds.0[axis] - 10.;
                let mut b = a; b[axis] = bounds.1[axis] + 10.;
                if ray_shapes(a, b, &original_shapes).is_some() && ray_shapes(a, b, &logic.prop_shapes[&id]).is_none() {
                    holes += 1;
                    let start = Vec3::from_array(crate::binary::source_position(pose.transform_point3(a).to_array()));
                    let end = Vec3::from_array(crate::binary::source_position(pose.transform_point3(b).to_array()));
                    let ray = end - start;
                    assert!(logic.breakable_hit_distance(id, start, ray.normalize(), ray.length()).is_none(),
                        "rotated world-space shot hit an authored hole");
                }
            }}
        }
        assert!(holes > 0, "damage geometry must expose authored gaps to rays");
        // The authored final Hotel door model sets dmg.bullets=0.
        assert_eq!(logic.world.durability[&id].bullets, 0.);
        let remaining = logic.world.durability[&id].health;
        assert!(!logic.world.damage_entity(id, maximum, l4d2r_game_core::entities::DamageKind::Bullet));
        assert_eq!(logic.world.durability[&id].health, remaining);
        logic.world.damage_entity(id, maximum, l4d2r_game_core::entities::DamageKind::Club);
        logic.world.damage_entity(id, maximum, l4d2r_game_core::entities::DamageKind::Club);
        assert_eq!(logic.world.events.iter().filter(|e| matches!(e, l4d2r_game_core::entities::EntEvent::Broken {ent} if *ent == id)).count(), 1);
        logic.restart();
        assert_eq!(logic.world.ents[id].me.mdl, original);
        assert_eq!(logic.prop_bounds[&id], bounds);
        assert_eq!(logic.door_damage[&id].current, None);
        assert_eq!(logic.world.durability[&id].health, maximum);
    }
    #[test]
    fn glass_health_inputs_and_bulletproof_material_keep_damage_state_consistent() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), false).unwrap();
        let id = logic.world.find("lab_glass")[0];
        logic.world.input(id, "SetHealth", "100", None, None);
        logic.world.input(id, "AddHealth", "25", None, None);
        assert_eq!(logic.world.durability[&id].health, 125.);
        logic.world.damage_entity(id, 10., l4d2r_game_core::entities::DamageKind::Bullet);
        assert_eq!(logic.world.durability[&id].health, 115.);
        logic.world.ents[id].me.ent.0.push(("material".into(), "7".into()));
        // Replace the existing material entry as BSP lookups return the first.
        for (key, value) in &mut logic.world.ents[id].me.ent.0 { if key == "material" { *value = "7".into(); } }
        assert!(!logic.world.damage_entity(id, 10000., l4d2r_game_core::entities::DamageKind::Bullet));
        assert!(logic.world.ents[id].alive);
        logic.world.input(id, "Break", "", None, None);
        assert!(!logic.world.ents[id].alive);
    }
    #[test]
    fn owned_damageable_door_blast_threshold_is_forty_before_propdata_scaling() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root,&root.join("left4dead2/maps/c1m1_hotel.bsp"),true).unwrap();
        let id=*logic.door_damage.keys().next().unwrap();
        let maximum=logic.world.durability[&id].maximum;
        assert!(!logic.world.damage_entity(id,39.,l4d2r_game_core::entities::DamageKind::Blast));
        assert_eq!(logic.world.durability[&id].health,maximum-39.*1.5);
        assert!(logic.world.damage_entity(id,40.,l4d2r_game_core::entities::DamageKind::Blast));
        assert_eq!(logic.world.events.iter().filter(|e|matches!(e,l4d2r_game_core::entities::EntEvent::Broken{ent} if *ent==id)).count(),1);
        logic.restart();
        assert!(!logic.world.damage_entity(id,40.,l4d2r_game_core::entities::DamageKind::Bullet));
        assert_eq!(logic.world.durability[&id].health,maximum-40.);
    }
    #[test]
    fn breakable_crate_and_fence_model_health_match_retail_prop_dynamic_create() {
        // Retail refuses wood_crate001a ("prop_dynamic has a health
        // specified") and accepts the fence used by EDIT WORLD.
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        assert!(model_health(&mut assets, "models/props_junk/wood_crate001a.mdl") > 0.);
        assert_eq!(model_health(&mut assets, "models/props_urban/fence_cover001_128.mdl"), 0.);
    }
    #[test]
    fn inherited_glass_damage_overrides_and_nested_door_settings() {
        let root = kv::blocks(
            r#""PropData.txt" { "Glass.Base" { "dmg.bullets" "1" "dmg.club" "1" } "Glass.Window" { "base" "Glass.Base" "health" "1" "dmg.bullets" "0.5" } "Glass.CSWindow2" { "base" "Glass.Window" } }"#,
        );
        let values = inherited(
            "Glass.CSWindow2",
            &root.children["propdata.txt"].children,
            0,
        );
        assert_eq!(values["health"], "1");
        assert_eq!(values["dmg.bullets"], "0.5");
        assert_eq!(values["dmg.club"], "1");
        let root = kv::blocks(
            r#"mdlkeyvalue { prop_data { "base" "Door.Standard" } door_options { defaults { "damage1" "native_mat" "surfaceprop" "wood" } } }"#,
        );
        assert_eq!(
            root.children["mdlkeyvalue"].children["door_options"].children["defaults"].values["surfaceprop"],
            "wood"
        );
    }
}
