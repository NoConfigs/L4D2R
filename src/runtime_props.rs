//! Props created at runtime by console commands (`prop_dynamic_create`,
//! `prop_physics_create`, `ent_create` with a model; retail testscripts and
//! the world editor). Their model, materials and textures load on demand,
//! cached per model; collision comes from the model's .phy pieces and its
//! .mdl bounds, as the map loader does for map props.
use bevy::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

type Draws = Vec<(Handle<Mesh>, Handle<crate::viewer::SourceMaterial>)>;

#[derive(Resource, Default)]
pub struct RuntimeModels {
    models: HashMap<String, Option<Draws>>,
    /// Entities waiting for their draws, and those already drawn.
    pending: Vec<usize>,
    drawn: HashMap<usize, Vec<Entity>>,
}

pub fn install(app: &mut App) {
    app.init_resource::<RuntimeModels>().add_systems(Update, present.after(crate::game_menu::update));
}

fn texture(assets: &mut crate::source_assets::SourceAssets, images: &mut Assets<Image>, path: &str, srgb: bool) -> Option<Handle<Image>> {
    let t = crate::vtf::decode(&assets.read(path).ok()?, 2048).ok()?;
    Some(images.add(Image::new(
        bevy::render::render_resource::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        t.rgba,
        if srgb {
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb
        } else {
            bevy::render::render_resource::TextureFormat::Rgba8Unorm
        },
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    )))
}

/// The model's draws (meshes + materials), loaded once.
#[allow(clippy::too_many_arguments)]
fn load_model(
    assets: &mut crate::source_assets::SourceAssets,
    path: &str,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<crate::viewer::SourceMaterial>,
    images: &mut Assets<Image>,
    lighting: &crate::viewer::LightingTable,
) -> Option<Draws> {
    let parts = crate::studio::load(assets, path, 0, 0).ok()?;
    let assets = RefCell::new(assets);
    let images = RefCell::new(images);
    let mut cache: HashMap<String, Handle<crate::viewer::SourceMaterial>> = HashMap::new();
    let mut out = vec![];
    for part in parts {
        let material = match cache.get(&part.material) {
            Some(h) => h.clone(),
            None => {
                let m = crate::material::load(&mut assets.borrow_mut(), &part.material).ok()?;
                let textures: RefCell<HashMap<(String, bool), Option<Handle<Image>>>> = RefCell::new(HashMap::new());
                let lookup = |p: &str, srgb: bool| {
                    textures.borrow_mut().entry((p.to_string(), srgb)).or_insert_with(|| {
                        texture(&mut assets.borrow_mut(), &mut images.borrow_mut(), p, srgb)
                    }).clone()
                };
                let handle = materials.add(crate::viewer::source_material(
                    &part.material,
                    &m,
                    &|p| lookup(p, true),
                    &|p| lookup(p, false),
                    (None, Vec4::ZERO, Vec4::ZERO),
                    &HashMap::new(),
                    lighting.buffer_handle(),
                ));
                cache.insert(part.material.clone(), handle.clone());
                let mesh = crate::viewer::mesh(part.positions.clone(), Some(part.normals.clone()), part.uv.clone(), m.uv);
                out.push((meshes.add(mesh), handle));
                continue;
            }
        };
        let uv = crate::material::load(&mut assets.borrow_mut(), &part.material).map_or([1., 1., 0., 0., 0., 1., 1.], |m| m.uv);
        out.push((meshes.add(crate::viewer::mesh(part.positions, Some(part.normals), part.uv, uv)), material));
    }
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn present(
    mut commands: Commands,
    mut state: ResMut<RuntimeModels>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    lighting: Res<crate::viewer::LightingTable>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut logic) = logic else { return };
    state.pending.extend(std::mem::take(&mut *crate::dev_console::SPAWNED.lock().unwrap()));
    // Killed entities lose their draws.
    let dead: Vec<usize> = state.drawn.keys().copied().filter(|id| !logic.world.ents.get(*id).is_some_and(|e| e.alive)).collect();
    for id in dead {
        for e in state.drawn.remove(&id).unwrap_or_default() {
            commands.entity(e).try_despawn();
        }
    }
    if state.pending.is_empty() {
        return;
    }
    let Some(source) = source else { return };
    let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) else { return };
    let mut synced = false;
    for id in std::mem::take(&mut state.pending) {
        let Some(model) = logic.world.ents.get(id).filter(|e| e.alive).and_then(|e| e.me.mdl.clone()) else { continue };
        if !state.models.contains_key(&model) {
            let draws = load_model(&mut assets, &model, &mut meshes, &mut materials, &mut images, &lighting);
            println!("Runtime prop model {model}: {}", draws.as_ref().map_or("failed to load".to_string(), |d| format!("{} draws", d.len())));
            state.models.insert(model.clone(), draws);
        }
        // Collision: .phy convex pieces and .mdl bounds (map loader rules).
        if let Ok(bytes) = assets.read(&model.replace(".mdl", ".phy")) {
            let pieces: Vec<_> = l4d2_formats::phy::parse_phy(&bytes).ok().into_iter().flatten().flat_map(|s| s.convexes).collect();
            if !pieces.is_empty() {
                logic.prop_shapes.insert(id, Arc::new(pieces));
            }
        }
        if let Ok(bytes) = assets.read(&model) {
            if let Some((lo, hi, vlo, vhi)) = l4d2_formats::mdl::read_header_bounds(&bytes) {
                let (lo, hi) = if lo == hi { (vlo, vhi) } else { (lo, hi) };
                logic.prop_bounds.insert(id, (l4d2r_math::Vec3::from_array(lo), l4d2r_math::Vec3::from_array(hi)));
            }
        }
        synced = true;
        let Some(Some(draws)) = state.models.get(&model).cloned() else { continue };
        let transform = crate::l4d2r_cinematic::transform(logic.world.world_matrix(id));
        let spawned = draws
            .iter()
            .map(|(mesh, material)| {
                commands
                    .spawn((
                        crate::l4d2r_cinematic::MapEntity(id),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        Visibility::Inherited,
                        transform,
                        crate::viewer::DynamicModelLighting::default(),
                    ))
                    .id()
            })
            .collect();
        println!("Runtime prop {id} {model} at {:?}", logic.world.ents[id].me.origin);
        state.drawn.insert(id, spawned);
    }
    if synced {
        if let Some(collision) = collision {
            collision.sync_entities(&logic);
        }
    }
}
