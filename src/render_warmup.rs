//! Prepare real render variants behind the loading artwork before play starts.
use bevy::{camera::visibility::NoFrustumCulling, prelude::*};
use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use bevy::render::{Render, RenderApp, RenderSystems,
    mesh::{RenderMesh, allocator::{allocate_and_free_meshes, MeshAllocatorSettings}},
    render_asset::{ExtractedAssets, RenderAsset, prepare_assets}};

#[derive(Resource, Clone, Default)]
struct MeshUploads {
    pending: Arc<AtomicUsize>,
    revision: Arc<AtomicUsize>,
}

struct QueuedMesh {
    mesh: Mesh,
    added: bool,
    modified: bool,
}
#[derive(Resource, Default)]
struct MeshUploadQueue {
    order: VecDeque<AssetId<Mesh>>,
    meshes: HashMap<AssetId<Mesh>, QueuedMesh>,
}
impl MeshUploadQueue {
    fn admit(&mut self, extracted: &mut ExtractedAssets<RenderMesh>, budget: usize) {
        for id in &extracted.removed { self.meshes.remove(id); }
        // Skinned meshes skip the queue. An entity's SkinnedMesh takes effect
        // the frame it is inserted; if its new skinned mesh waited here, the
        // prepass kept the entity's previous non-skinned pipeline and drew it
        // with the skinned bind group (wgpu validation error; Bevy quits).
        let mut direct = Vec::new();
        for (id, mesh) in extracted.extracted.drain(..) {
            let added = extracted.added.remove(&id);
            let modified = extracted.modified.remove(&id);
            if mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX) {
                let earlier = self.meshes.remove(&id);
                direct.push((id, mesh, added || earlier.as_ref().is_some_and(|q| q.added),
                    modified || earlier.is_some_and(|q| q.modified)));
                continue;
            }
            if let Some(queued) = self.meshes.get_mut(&id) {
                queued.mesh = mesh;
                queued.added |= added;
                queued.modified |= modified;
            } else {
                self.order.push_back(id);
                self.meshes.insert(id, QueuedMesh { mesh, added, modified });
            }
        }
        for (id, mesh, added, modified) in direct {
            if added { extracted.added.insert(id); }
            if modified { extracted.modified.insert(id); }
            extracted.extracted.push((id, mesh));
        }
        let mut bytes = 0;
        while let Some(id) = self.order.front().copied() {
            let Some(queued) = self.meshes.get(&id) else {
                self.order.pop_front();
                continue;
            };
            let size = <RenderMesh as RenderAsset>::byte_len(&queued.mesh).unwrap_or(0);
            // Always admit one oversized asset so progress cannot deadlock.
            if bytes != 0 && bytes + size > budget { break; }
            self.order.pop_front();
            let queued = self.meshes.remove(&id).unwrap();
            if queued.added { extracted.added.insert(id); }
            if queued.modified { extracted.modified.insert(id); }
            extracted.extracted.push((id, queued.mesh));
            bytes += size;
            if bytes >= budget { break; }
        }
    }
}
fn throttle_mesh_uploads(
    mut queue: ResMut<MeshUploadQueue>,
    mut extracted: ResMut<ExtractedAssets<RenderMesh>>,
    status: Res<MeshUploads>,
) {
    let incoming = extracted.extracted.len();
    let previous = queue.meshes.len();
    queue.admit(&mut extracted, 8 << 20);
    status.pending.store(queue.meshes.len(), Ordering::Release);
    // Only fence completion of a deferred batch. Small live meshes continue
    // changing while paused and must not restart warmup indefinitely.
    if previous != 0 && queue.meshes.is_empty() {
        status.revision.fetch_add(1, Ordering::Release);
    }
    if incoming > 1000 || (previous != 0 && queue.meshes.is_empty()) {
        println!("Mobile mesh upload queue: admitted={} pending={}", extracted.extracted.len(), queue.meshes.len());
        crate::performance::memory_checkpoint("Mobile mesh upload batch");
    }
}
/// A mesh nothing reads after loading (packed static props). On phones its
/// main-world copy is dropped once warmup has uploaded it: CPU and GPU share
/// memory there, so the copy counted twice against the 3 GB limit. The drop
/// is untracked, so the GPU mesh is kept and nothing is re-uploaded.
#[derive(Component, Default)]
pub struct ReleaseCpuMesh(u8);
fn release_cpu_meshes(
    mut commands: Commands,
    warm: Option<Res<Warmup>>,
    uploads: Option<Res<MeshUploads>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut marked: Query<(Entity, &Mesh3d, &mut ReleaseCpuMesh)>,
) {
    if marked.is_empty() {
        return;
    }
    // The render world took its own copy when the mesh was extracted (the
    // frame after it was added; the upload queue holds that copy until the
    // GPU upload). Dropping the main-world copy then, rather than after the
    // whole warmup, keeps the two copies from stacking at the loading peak
    // (PC phone profile c2m1 -> c2m2: both copies were alive at the peak).
    let _ = (&warm, &uploads);
    let (mut count, mut bytes) = (0usize, 0usize);
    for (entity, mesh, mut age) in &mut marked {
        // Three frames: extraction and the first warmup selection have run.
        if age.0 < 3 {
            age.0 += 1;
            continue;
        }
        commands.entity(entity).remove::<ReleaseCpuMesh>();
        let Some(asset) = meshes.get_mut_untracked(&mesh.0) else {
            continue;
        };
        if let Ok(attributes) = asset.try_attributes() {
            bytes += attributes.map(|(_, v)| v.get_bytes().len()).sum::<usize>();
        }
        if let Ok(Some(indices)) = asset.try_indices_option() {
            bytes += indices.len() * if matches!(indices, bevy::mesh::Indices::U16(_)) { 2 } else { 4 };
        }
        if asset.take_gpu_data().is_ok() {
            count += 1;
        }
    }
    if count > 0 {
        println!("Released CPU copies of {count} static meshes: {:.1} MiB", bytes as f64 / 1048576.);
        crate::performance::memory_checkpoint("Static mesh CPU copies released");
    }
}
/// Phones: a CPU-only mesh (a model template, or a copy of one) gains its
/// GPU copy the first time an entity draws it.
/// A mesh only drawn after loading (`ReleaseCpuMesh`: packed static props,
/// private lit copies) becomes GPU-only instead, so extraction moves its
/// data to the render world rather than cloning it. The clone had every
/// static mesh of the map in memory twice for the first warmup frames
/// (+265 MiB on the iPhone at c2m1's first upload, where it was killed).
fn upload_drawn_meshes(drawn: Query<(&Mesh3d, Has<ReleaseCpuMesh>), Changed<Mesh3d>>, mut meshes: ResMut<Assets<Mesh>>) {
    use bevy::asset::RenderAssetUsages;
    let phone = crate::performance::mobile_memory_profile();
    for (mesh, draw_only) in &drawn {
        let Some(usage) = meshes.get(&mesh.0).map(|m| m.asset_usage) else { continue };
        let wanted = if draw_only && phone {
            RenderAssetUsages::RENDER_WORLD
        } else {
            usage | RenderAssetUsages::RENDER_WORLD
        };
        if usage != wanted {
            if let Some(mut asset) = meshes.get_mut(&mesh.0) {
                asset.asset_usage = wanted;
            }
        }
    }
}
#[derive(Resource, Default)]
pub struct Warmup {
    frames: u32,
    stable: u32,
    previous: usize,
    keys: HashSet<u64>,
    selected: HashSet<Entity>,
    mesh_revision: usize,
}
/// Includes the completion frame: FixedUpdate has not resumed in that frame.
#[derive(Resource, Default)]
pub struct LoadingFrame(pub bool);
impl Warmup {
    fn advance(&mut self, total: usize, waiting: usize) -> bool {
        self.frames += 1;
        self.stable = if total > 0 && waiting == 0 && total == self.previous {
            self.stable + 1
        } else {
            0
        };
        self.previous = total;
        self.frames >= 32 && self.stable >= 6
    }
}
#[derive(Component)]
struct Prepared {
    visibility: Visibility,
    no_culling: bool,
}
pub fn begin(world: &mut World) {
    if !world.resource::<crate::game_menu::Frontend>().playing() {
        return;
    }
    let root = world.resource::<crate::game_ui::HudSource>().0.clone();
    let map = world
        .resource::<crate::campaign_settings::SessionSettings>()
        .map
        .clone();
    world.insert_resource(Warmup::default());
    world.insert_resource(LoadingFrame(true));
    world.resource_mut::<crate::game_state::Match>().paused = true;
    crate::map_transition::overlay(world, &root, &map);
}
fn stage(
    mut commands: Commands,
    warm: Option<ResMut<Warmup>>,
    meshes: Res<Assets<Mesh>>,
    source: Res<Assets<crate::viewer::SourceMaterial>>,
    water: Res<Assets<crate::source_water::WaterMaterial>>,
    standard: Res<Assets<StandardMaterial>>,
    parents: Query<&ChildOf>,
    mut draws: Query<
        (
            Entity,
            &mut Visibility,
            Option<&mut Prepared>,
            Option<&Mesh3d>,
            Option<&NoFrustumCulling>,
            Option<&crate::l4d2r_animation::GpuSkin>,
            Option<&MeshMaterial3d<crate::viewer::SourceMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&bevy::pbr::Lightmap>,
            Option<&bevy::camera::visibility::RenderLayers>,
            Option<&MeshMaterial3d<crate::source_water::WaterMaterial>>,
        ),
        (With<Transform>, Without<Camera>),
    >,
) {
    let Some(mut warm) = warm else {
        return;
    };
    // Queue one representative for each material/layout specialization, not
    // the entire map. Hidden effect pools are covered by the same keys.
    for (entity, _, _, mesh, _, gpu, material, std_material, lightmap, layers, water_material) in &draws {
        let Some(mesh) = mesh.and_then(|m| meshes.get(&m.0)) else {
            continue;
        };
        // Meshes released by `release_cpu_meshes` have no CPU layout left.
        let (Ok(attributes), Ok(indices)) = (mesh.try_attributes(), mesh.try_indices_option()) else {
            continue;
        };
        let mut key = std::collections::hash_map::DefaultHasher::new();
        mesh.primitive_topology().hash(&mut key);
        indices.is_some().hash(&mut key);
        for (attribute, values) in attributes {
            attribute.id.hash(&mut key);
            std::mem::discriminant(values).hash(&mut key);
        }
        if let Some(m) = material.and_then(|m| source.get(&m.0)) {
            0u8.hash(&mut key);
            crate::viewer::warm_material_key(m).hash(&mut key);
        } else if let Some(m) = std_material.and_then(|m| standard.get(&m.0)) {
            1u8.hash(&mut key);
            bevy::pbr::StandardMaterialKey::from(m).hash(&mut key);
        } else if let Some(m) = water_material.and_then(|m| water.get(&m.0)) {
            2u8.hash(&mut key);
            bevy::pbr::StandardMaterialKey::from(&m.base).hash(&mut key);
            crate::source_water::WaterKey::from(&m.extension).hash(&mut key);
        } else {
            continue;
        }
        gpu.is_some().hash(&mut key);
        lightmap.is_some().hash(&mut key);
        format!("{layers:?}").hash(&mut key);
        if warm.keys.insert(key.finish()) {
            warm.selected.insert(entity);
        }
    }
    let mut forced = warm.selected.clone();
    for &entity in &warm.selected {
        let mut at = entity;
        for _ in 0..64 {
            let Ok(parent) = parents.get(at) else {
                break;
            };
            at = parent.parent();
            forced.insert(at);
        }
    }
    for (entity, mut visibility, prepared, mesh, no_culling, gpu, _, _, _, _, _) in &mut draws {
        if !forced.contains(&entity) {
            continue;
        }
        if let Some(mut prepared) = prepared {
            // GPU rig setup may remove its initial CPU-skin culling override.
            if gpu.is_some() && no_culling.is_none() {
                prepared.no_culling = false;
            }
        } else {
            commands.entity(entity).insert(Prepared {
                visibility: *visibility,
                no_culling: no_culling.is_some(),
            });
        }
        if mesh.is_some() && no_culling.is_none() {
            commands.entity(entity).insert(NoFrustumCulling);
        }
        if *visibility != Visibility::Inherited {
            *visibility = Visibility::Inherited;
        }
    }
}
/// Warmup frame at which every mesh is re-uploaded (the previous map's
/// meshes have been released by then).
const REPACK_FRAME: u32 = 8;
/// Bevy's mesh allocator never shrinks a vertex/index slab while any mesh is
/// still in it. After a chapter change, the few meshes that outlive the old
/// map pin its slabs (c1m1 -> c1m2: an extra 437 MB vertex slab), so the new
/// map's meshes are split across old and new slabs: more draw batches, ~1 GB
/// of dead VRAM and every later GPU allocation/submit slowed (c1m2 render
/// thread 8.9 -> 15.6 ms). Marking every mesh changed frees and re-allocates
/// them all in one allocator pass, which drops the emptied slabs.
fn repack_meshes(world: &mut World) {
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let count = meshes.iter_mut().count();
    println!("Render warmup: repacked {count} meshes");
}
fn refresh_mesh_instances(world: &mut World) {
    // GPU preprocessing can cache a zero-length draw when the entity arrives
    // before its throttled mesh, and buffer growth can relocate older ranges.
    // Re-extract instance metadata after uploads; never re-upload the meshes.
    let mut count = 0;
    for mut mesh in world.query::<&mut Mesh3d>().iter_mut(world) {
        mesh.set_changed();
        count += 1;
    }
    println!("Mobile mesh instances refreshed: {count}; mesh assets unchanged");
}
pub fn ready(world: &mut World) {
    let loading = world.contains_resource::<Warmup>();
    if let Some(mut frame) = world.get_resource_mut::<LoadingFrame>() {
        frame.0 = loading;
    }
    if !loading {
        return;
    }
    world.resource_mut::<crate::game_state::Match>().paused = true;
    *world.resource_mut::<crate::game::Input>() = default();
    let (total, mut waiting) = world
        .get_resource::<crate::performance::PipelineStatus>()
        .map_or((1, 0), |s| *s.0.lock().unwrap());
    // Keep the loading screen up until every throttled map texture is added.
    waiting += world
        .get_resource::<crate::graphics_options::PendingTextures>()
        .map_or(0, |p| p.0.len());
    let mobile_uploads = world.get_resource::<MeshUploads>().map(|s|
        (s.pending.load(Ordering::Acquire), s.revision.load(Ordering::Acquire)));
    waiting += mobile_uploads.map_or(0, |s| s.0);
    if let Some((0, revision)) = mobile_uploads {
        if world.resource::<Warmup>().mesh_revision != revision {
            refresh_mesh_instances(world);
            let mut warm = world.resource_mut::<Warmup>();
            warm.mesh_revision = revision;
            warm.stable = 0;
        }
    }
    if world.resource::<Warmup>().frames == REPACK_FRAME && mobile_uploads.is_none() {
        repack_meshes(world);
    }
    let frame = world.resource::<Warmup>().frames;
    if matches!(frame, 0..=3 | 8 | 16 | 32 | 64 | 128) {
        crate::performance::memory_checkpoint(&format!("Render warmup frame {frame}; pending={waiting}"));
        crate::performance::gpu_audit(&format!("warmup {frame}"));
    }
    if !world.resource_mut::<Warmup>().advance(total, waiting) {
        return;
    }
    let frames = world.resource::<Warmup>().frames;
    let draws = world
        .query::<(Entity, &Prepared)>()
        .iter(world)
        .map(|(e, p)| (e, p.visibility, p.no_culling))
        .collect::<Vec<_>>();
    for (entity, visibility, no_culling) in draws {
        let mut e = world.entity_mut(entity);
        e.insert(visibility);
        e.remove::<Prepared>();
        if !no_culling {
            e.remove::<NoFrustumCulling>();
        }
    }
    world.remove_resource::<Warmup>();
    // PVS visibility may have been overridden only for warmup.
    world.resource_mut::<crate::viewer::Map>().set_changed();
    crate::map_transition::remove_overlay(world);
    let paused =
        world.resource::<crate::game_menu::Frontend>().page != crate::game_menu::Page::Game;
    world.resource_mut::<crate::game_state::Match>().paused = paused;
    println!("Render warmup ready: frames={frames} pipelines={total} waiting={waiting}");
    crate::stack_sampler::warmup_finished();
}
pub fn install(app: &mut App) {
    if cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-memory") {
        let status = MeshUploads::default();
        app.insert_resource(status.clone());
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            // Growing a default 512 MiB slab keeps its old and replacement
            // Metal buffers live during the copy. Bound that peak on phones.
            let mut allocator = MeshAllocatorSettings::default();
            allocator.max_slab_size = 32 << 20;
            allocator.large_threshold = 16 << 20;
            render.insert_resource(allocator)
                .insert_resource(status)
                .init_resource::<MeshUploadQueue>()
                .add_systems(Render, throttle_mesh_uploads
                    .in_set(RenderSystems::PrepareAssets)
                    .before(allocate_and_free_meshes)
                    .before(prepare_assets::<RenderMesh>));
        }
        app.add_systems(Update, release_cpu_meshes)
            .add_systems(Last, upload_drawn_meshes);
        println!("Mobile mesh uploads: 8 MiB/frame; slabs=32 MiB; warmup reupload disabled");
    }
    app.init_resource::<LoadingFrame>()
        .add_systems(
            Startup,
            begin
                .after(crate::viewer::RenderSetup::Scene)
                .after(crate::game_ui::setup),
        )
        .add_systems(
            PostUpdate,
            stage
                .after(crate::viewer::cull_pvs)
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refreshing_instances_does_not_mark_mesh_assets_for_reupload() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        let handle = world.resource_mut::<Assets<Mesh>>().add(triangle(7.));
        let entity = world.spawn(Mesh3d(handle.clone())).id();
        world.clear_trackers();
        refresh_mesh_instances(&mut world);
        assert_eq!(world.query_filtered::<Entity, Changed<Mesh3d>>().iter(&world).collect::<Vec<_>>(), vec![entity]);
        assert!(!world.get_resource_ref::<Assets<Mesh>>().unwrap().is_changed());
        assert_eq!(world.resource::<Assets<Mesh>>().get(&handle).unwrap().attribute(Mesh::ATTRIBUTE_POSITION),
            triangle(7.).attribute(Mesh::ATTRIBUTE_POSITION));
    }
    fn triangle(x: f32) -> Mesh {
        let mut mesh = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[x, 0., 0.]; 3]);
        mesh
    }
    #[test]
    fn upload_budget_keeps_latest_changes_and_cancels_removed_meshes() {
        let mut assets = Assets::<Mesh>::default();
        let handles = (0..3).map(|i| assets.add(triangle(i as f32))).collect::<Vec<_>>();
        let mut extracted = ExtractedAssets::<RenderMesh>::default();
        for h in &handles {
            extracted.extracted.push((h.id(), assets.get(h).unwrap().clone()));
            extracted.added.insert(h.id());
        }
        let mut queue = MeshUploadQueue::default();
        queue.admit(&mut extracted, 36);
        assert_eq!(extracted.extracted.len(), 1);
        assert_eq!(extracted.extracted[0].0, handles[0].id());
        assert_eq!(queue.meshes.len(), 2);
        assert_eq!(extracted.added.len(), 1);
        // Simulate the next extraction after the renderer consumed the batch.
        extracted = ExtractedAssets::<RenderMesh>::default();
        extracted.removed.insert(handles[2].id());
        extracted.modified.insert(handles[1].id());
        extracted.extracted.push((handles[1].id(), triangle(99.)));
        queue.admit(&mut extracted, 36);
        assert!(queue.meshes.is_empty());
        assert_eq!(extracted.extracted.len(), 1);
        assert!(extracted.added.contains(&handles[1].id()));
        assert!(extracted.modified.contains(&handles[1].id()));
        assert!(extracted.removed.contains(&handles[2].id()));
        assert_eq!(extracted.extracted[0].1.attribute(Mesh::ATTRIBUTE_POSITION),
            triangle(99.).attribute(Mesh::ATTRIBUTE_POSITION));
    }
    #[test]
    fn oversized_upload_makes_progress_and_defers_the_next_mesh() {
        let mut assets = Assets::<Mesh>::default();
        let a = assets.add(triangle(1.));
        let b = assets.add(triangle(2.));
        let mut extracted = ExtractedAssets::<RenderMesh>::default();
        extracted.extracted = vec![(a.id(), triangle(1.)), (b.id(), triangle(2.))];
        let mut queue = MeshUploadQueue::default();
        queue.admit(&mut extracted, 1);
        assert_eq!(extracted.extracted.len(), 1);
        assert_eq!(queue.meshes.len(), 1);
        extracted = ExtractedAssets::<RenderMesh>::default();
        queue.admit(&mut extracted, 1);
        assert_eq!(extracted.extracted[0].0, b.id());
        assert!(queue.meshes.is_empty());
    }
    #[test]
    fn loading_waits_for_settled_pipeline_queue_not_just_a_fixed_delay() {
        let mut warm = Warmup::default();
        for _ in 0..40 {
            assert!(!warm.advance(60, 5));
        }
        for _ in 0..5 {
            assert!(!warm.advance(60, 0));
        }
        assert!(warm.advance(60, 0));
        assert!(!warm.advance(61, 0));
        for _ in 0..5 {
            assert!(!warm.advance(61, 0));
        }
        assert!(warm.advance(61, 0));
    }
}
