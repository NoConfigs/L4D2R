use crate::{
    binary, material,
    scene::{ModelKey, Scene},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::RenderLayers,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    input::mouse::AccumulatedMouseMotion,
    mesh::PrimitiveTopology,
    pbr::{ExtendedMaterial, Lightmap, MaterialExtension},
    post_process::auto_exposure::{
        AutoExposure, AutoExposureCompensationCurve, AutoExposurePlugin,
    },
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    window::{CursorGrabMode, CursorOptions},
};
use std::collections::HashMap;
#[derive(bevy::render::render_resource::ShaderType, Reflect, Debug, Clone)]
pub(crate) struct WoundUniform {
    pub params: [Vec4; 2],
    pub atlas: [Vec4; 2],
    pub blood: Vec4,
    pub blood_range: Vec4,
}
impl Default for WoundUniform {
    fn default() -> Self {
        Self { params: [Vec4::new(1., 0.95, 0., 0.); 2], atlas: [Vec4::new(1., 1., 0., 0.); 2],
            blood: Vec4::new(0.15, 0., 0., 0.), blood_range: Vec4::new(0.1, 0.9, 0., 0.) }
    }
}
impl WoundUniform {
    fn from_material(w: &crate::material::WoundSettings) -> Self {
        Self { params: [Vec4::new(w.mapping_scale, w.bias, 0., 0.); 2],
            atlas: [Vec4::new(if w.texture.contains("combined") { 0.5 } else { 1. }, 1., 0., 0.); 2],
            blood: Vec3::from_array(w.blood).extend(0.), blood_range: Vec4::new(w.blood_range[0], w.blood_range[1], 0., 0.) }
    }
}
#[derive(bevy::render::render_resource::ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct InfectedPhongUniform {
    exponents: Vec4,
    fresnel: Vec4,
    tint: Vec4,
    detail: Vec4,
}
impl From<&crate::material::InfectedPhongSettings> for InfectedPhongUniform {
    fn from(value: &crate::material::InfectedPhongSettings) -> Self {
        Self { exponents: Vec4::from_array(value.exponents),
            fresnel: Vec4::from_array(value.fresnel), tint: Vec4::from_array(value.tint),
            detail: Vec4::from_array(value.detail) }
    }
}
#[derive(bevy::render::render_resource::ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct CapPhongUniform {
    params: Vec4,
    fresnel: Vec4,
    controls: Vec4,
    tint: Vec4,
}
impl From<&crate::material::CapPhongSettings> for CapPhongUniform {
    fn from(value: &crate::material::CapPhongSettings) -> Self {
        Self { params: Vec4::from_array(value.params), fresnel: Vec4::from_array(value.fresnel),
            controls: Vec4::from_array(value.controls), tint: Vec4::from_array(value.tint) }
    }
}
#[derive(Asset, bevy::render::render_resource::AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(SourceLightingKey)]
pub(crate) struct SourceLighting {
    #[uniform(100)]
    settings: Vec4,
    #[uniform(100)]
    ambient_cube: [Vec4; 6],
    #[uniform(100)]
    sun_direction: Vec4,
    #[uniform(100)]
    sun_color: Vec4,
    /// The map's single lightmap atlas, bound on world-surface materials.
    /// Bevy's per-entity `Lightmap` component gave each of ~4000 brush
    /// entities its own slot (one slab per 4) and rebuilt a bind group per
    /// slab per render phase every frame; one shared binding samples the same
    /// texture at the same UVs.
    #[texture(104)]
    #[sampler(105)]
    lightmap: Option<Handle<Image>>,
    /// x: lightmap enabled, y: exposure (StandardMaterial::lightmap_exposure).
    #[uniform(100)]
    lightmap_params: Vec4,
    /// Per-draw model lighting table, indexed by `MeshTag` (slot + 1). Lets
    /// every dynamically lit draw of a material share one material, so Bevy
    /// can batch them instead of issuing one draw per lighting value.
    #[storage(107, read_only)]
    lighting_table: Handle<bevy::render::storage::ShaderBuffer>,
    #[texture(108)]
    #[cfg_attr(not(any(target_os = "ios", feature = "mobile-materials")), sampler(109))]
    emission: Option<Handle<Image>>,
    #[uniform(100)]
    emission_tint: Vec4,
    /// Four pelvis-bind-space ellipsoids (two volumes per wound).
    #[uniform(100)]
    pub(crate) wound_rows: [Vec4; 12],
    #[uniform(100)]
    pub(crate) wound: WoundUniform,
    #[texture(113)]
    #[sampler(114)]
    wound_mask: Option<Handle<Image>>,
    #[texture(115)]
    #[sampler(116)]
    infected_base: Option<Handle<Image>>,
    #[texture(117)]
    #[sampler(118)]
    infected_palette: Option<Handle<Image>>,
    /// xy quarter-sheet offset; zw skin/clothing lookup row centres.
    #[uniform(100)]
    infected_lookup: Vec4,
    #[uniform(100)]
    infected_phong: InfectedPhongUniform,
    #[texture(121)]
    #[sampler(122)]
    infected_detail: Option<Handle<Image>>,
    #[texture(123)]
    #[cfg_attr(not(any(target_os = "ios", feature = "mobile-materials")), sampler(124))]
    infected_burn: Option<Handle<Image>>,
    #[uniform(100)]
    cap_phong: CapPhongUniform,
    #[texture(126)]
    #[sampler(127)]
    pub(crate) glass_mask: Option<Handle<Image>>,
    /// WorldVertexTransition `$basetexture2`.
    #[texture(128)]
    #[sampler(129)]
    pub(crate) blend_texture: Option<Handle<Image>>,
    /// WorldVertexTransition `$blendmodulatetexture` (linear).
    #[texture(130)]
    #[cfg_attr(not(any(target_os = "ios", feature = "mobile-materials")), sampler(131))]
    pub(crate) blend_modulate: Option<Handle<Image>>,
    /// `$envmap` capture (HDR cube, Source-axis faces).
    #[texture(132, dimension = "cube")]
    #[sampler(133)]
    pub(crate) envmap: Option<Handle<Image>>,
    /// rgb $envmaptint; w 1 when an envmap is bound.
    #[uniform(100)]
    pub(crate) envmap_tint: Vec4,
    /// $envmapsaturation, $envmapcontrast, base-alpha mask, normal-alpha mask.
    #[uniform(100)]
    pub(crate) envmap_params: Vec4,
    /// x: bumped lightmaps (SOURCE_BUMPED), y: `$ssbump`.
    #[uniform(100)]
    pub(crate) bump: Vec4,
    /// The `$bumpmap` the bumped-lightmap path reads (linear).
    #[texture(134)]
    #[cfg_attr(not(any(target_os = "ios", feature = "mobile-materials")), sampler(135))]
    pub(crate) bump_map: Option<Handle<Image>>,
}
impl SourceLighting {
    pub(crate) fn has_wound_mask(&self) -> bool {
        self.wound_mask.is_some()
    }
}
/// Slots in the model lighting table: 6 ambient-cube faces + sun direction +
/// sun colour (8 vec4s, 128 bytes) each. Fixed size so the GPU buffer, and the
/// material bind groups that reference it, are never replaced.
pub(crate) const LIGHT_SLOTS: usize = 4096;
const LIGHT_SLOT_VEC4S: usize = 8;
/// The last lighting-table slot holds per-map sky/far-clip values
/// (native_fog::sky_globals); it is never handed to a draw.
const GLOBAL_SLOT: usize = LIGHT_SLOTS - 1;
#[derive(Resource)]
pub(crate) struct LightingTable {
    buffer: Handle<bevy::render::storage::ShaderBuffer>,
    data: Vec<Vec4>,
    owners: HashMap<Entity, u32>,
    free: Vec<u32>,
    next: u32,
    dirty: bool,
    /// Shared dynamic variant per (base material, viewmodel).
    variants: HashMap<(AssetId<SourceMaterial>, bool), Handle<SourceMaterial>>,
}
/// A Source-lit material whose lighting comes from vertex colours (like
/// static props): base x vertex light x exposure, with map fog.
pub(crate) fn vertex_lit_material(base: StandardMaterial, table: &LightingTable) -> SourceMaterial {
    SourceMaterial {
        base,
        extension: SourceLighting {
            settings: Vec4::new(1., 0., 0., 0.),
            ambient_cube: [Vec4::ZERO; 6],
            sun_direction: Vec4::ZERO,
            sun_color: Vec4::ZERO,
            lightmap: None,
            lightmap_params: Vec4::ZERO,
            lighting_table: table.buffer.clone(),
            emission: None,
            emission_tint: Vec4::ZERO,
            wound_rows: [Vec4::ZERO; 12],
            wound: WoundUniform::default(),
            wound_mask: None,
            infected_base: None,
            infected_palette: None,
            infected_lookup: Vec4::ZERO,
            infected_phong: InfectedPhongUniform::default(),
            infected_detail: None,
            infected_burn: None,
            cap_phong: CapPhongUniform::default(),
            glass_mask: None,
            envmap: None,
            envmap_tint: Vec4::ZERO,
            envmap_params: Vec4::ZERO,
                bump: Vec4::ZERO,
                bump_map: None,
            blend_texture: None,
            blend_modulate: None,
        },
    }
}
impl FromWorld for LightingTable {
    fn from_world(world: &mut World) -> Self {
        let data = vec![Vec4::ZERO; LIGHT_SLOTS * LIGHT_SLOT_VEC4S];
        let buffer = world
            .resource_mut::<Assets<bevy::render::storage::ShaderBuffer>>()
            .add(bevy::render::storage::ShaderBuffer::with_size(
                data.len() * 16,
                bevy::asset::RenderAssetUsages::default(),
            ));
        Self {
            buffer,
            data,
            owners: HashMap::new(),
            free: vec![],
            next: 0,
            dirty: false,
            variants: HashMap::new(),
        }
    }
}
fn table_bytes(data: &[Vec4]) -> Vec<u8> {
    data.iter()
        .flat_map(|v| v.to_array())
        .flat_map(f32::to_le_bytes)
        .collect()
}
impl LightingTable {
    /// The shared lighting buffer (crate::runtime_props' materials).
    pub(crate) fn buffer_handle(&self) -> Handle<bevy::render::storage::ShaderBuffer> {
        self.buffer.clone()
    }
    fn slot(&mut self, entity: Entity) -> Option<u32> {
        if let Some(slot) = self.owners.get(&entity) {
            return Some(*slot);
        }
        let slot = self.free.pop().or_else(|| {
            (self.next < GLOBAL_SLOT as u32).then(|| {
                self.next += 1;
                self.next - 1
            })
        })?;
        self.owners.insert(entity, slot);
        Some(slot)
    }
    fn owner(&self, entity: Entity) -> Option<u32> {
        self.owners.get(&entity).copied()
    }
    fn release(&mut self, entity: Entity) {
        if let Some(slot) = self.owners.remove(&entity) {
            self.free.push(slot);
        }
    }
    fn write(&mut self, slot: u32, cube: [Vec4; 6], direction: Vec4, color: Vec4) -> bool {
        let at = slot as usize * LIGHT_SLOT_VEC4S;
        let values = [
            cube[0], cube[1], cube[2], cube[3], cube[4], cube[5], direction, color,
        ];
        if self.data[at..at + LIGHT_SLOT_VEC4S] == values {
            return false;
        }
        self.data[at..at + LIGHT_SLOT_VEC4S].copy_from_slice(&values);
        self.dirty = true;
        true
    }
    fn reset(&mut self) {
        self.owners.clear();
        self.free.clear();
        self.next = 0;
        self.variants.clear();
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SourceLightingKey {
    sky: bool,
    wounds: bool,
    infected: bool,
    cap_phong: bool,
    glass: bool,
    blend: bool,
    blend_modulate: bool,
    envmap: bool,
    fence: bool,
    half_lambert: bool,
    bumped: bool,
}
impl From<&SourceLighting> for SourceLightingKey {
    fn from(value: &SourceLighting) -> Self {
        Self {
            sky: value.settings.w > 0.5,
            wounds: value.wound_rows.iter().any(|r| *r != Vec4::ZERO),
            infected: value.infected_base.is_some(),
            cap_phong: value.cap_phong.params.w > 0.5,
            glass: value.glass_mask.is_some(),
            blend: value.blend_texture.is_some(),
            blend_modulate: value.blend_texture.is_some() && value.blend_modulate.is_some(),
            envmap: value.envmap.is_some(),
            fence: value.lightmap_params.z > 0.,
            half_lambert: value.lightmap_params.w > 0.5,
            bumped: value.bump.x > 0.5,
        }
    }
}
#[derive(Component, Default)]
pub struct DynamicModelLighting {
    material: Option<Handle<SourceMaterial>>,
    sample_position: Option<Vec3>,
    center: Option<Vec3>,
    /// Lighting-table slot carried by this draw's `MeshTag`.
    slot: Option<u32>,
    appearance: Option<crate::infected_appearance::Appearance>,
}
impl DynamicModelLighting {
    /// Replaces the base material the lit variants are made from (a runtime
    /// prop's rendercolor tint).
    pub(crate) fn set_base(&mut self, material: Handle<SourceMaterial>) {
        self.material = Some(material);
    }
}
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ModelLightingSystems { Write }
#[derive(Default)]
struct LightingMaterialCache(
    HashMap<(AssetId<SourceMaterial>, bool, [u32; 32]), Handle<SourceMaterial>>,
);
impl LightingMaterialCache {
    fn resolve(
        &mut self,
        materials: &mut Assets<SourceMaterial>,
        base: &Handle<SourceMaterial>,
        viewmodel: bool,
        cube: [Vec4; 6],
        direction: Vec4,
        color: Vec4,
        original_normal: Option<&Option<Handle<Image>>>,
    ) -> Option<Handle<SourceMaterial>> {
        let mut bits = [0; 32];
        for (slot, value) in bits.iter_mut().zip(
            cube.into_iter()
                .chain([direction, color])
                .flat_map(|v| v.to_array()),
        ) {
            *slot = value.to_bits();
        }
        let key = (base.id(), viewmodel, bits);
        if let Some(handle) = self.0.get(&key) {
            return Some(handle.clone());
        }
        let mut native = materials.get(base)?.clone();
        if let Some(normal) = original_normal {
            native.base.normal_map_texture = normal.clone();
        }
        native.extension.settings.y = 1.;
        native.extension.settings.z = if viewmodel { 1. } else { 0. };
        native.extension.ambient_cube = cube;
        native.extension.sun_direction = direction;
        native.extension.sun_color = color;
        let handle = materials.add(native);
        if self.0.len() >= 8192 {
            self.0.clear();
        }
        self.0.insert(key, handle.clone());
        Some(handle)
    }
}
/// World-surface variant of a material that samples the shared lightmap
/// atlas (cached per base material). `--profile-legacy-lightmap` keeps the
/// base material and Bevy's per-entity `Lightmap` component instead.
fn world_material(
    cache: &mut HashMap<AssetId<SourceMaterial>, Handle<SourceMaterial>>,
    materials: &mut Assets<SourceMaterial>,
    base: &Handle<SourceMaterial>,
    light: &Handle<Image>,
    legacy: bool,
) -> Handle<SourceMaterial> {
    if legacy {
        return base.clone();
    }
    cache
        .entry(base.id())
        .or_insert_with(|| {
            let mut material = materials.get(base).unwrap().clone();
            material.extension.lightmap = Some(light.clone());
            let [_, _, fence, half_lambert] = material.extension.lightmap_params.to_array();
            material.extension.lightmap_params =
                Vec4::new(1., material.base.lightmap_exposure, fence, half_lambert);
            // Bumped lightmaps need the bump map; world meshes carry the
            // per-vertex basis stride (SOURCE_BUMP).
            // L4D2R_NO_BUMPED: debug, the flat lightmap only.
            if material.extension.bump_map.is_some() && std::env::var_os("L4D2R_NO_BUMPED").is_none() {
                material.extension.bump.x = 1.;
            }
            materials.add(material)
        })
        .clone()
}
/// One lighting-cache cell: the ambient cube and the visible sun.
fn sample_model_light(
    world: &crate::bsp::World,
    shadows: &crate::lighting::WorldShadow,
    position: Vec3,
) -> ([[f32; 3]; 6], Vec4, Vec4) {
    let cube = model_ambient(world, shadows, position);
    let _sun = crate::performance::scope("model_lighting::sun");
    let mut direction = Vec4::ZERO;
    let mut color = Vec4::ZERO;
    if let Some(sun) = world.suns.first() {
        let ray = Vec3::from_array(sun.direction).normalize_or_zero();
        if !shadows.blocked(position.to_array(), ray.to_array()) {
            direction = ray.extend(0.);
            color = Vec3::from_array(sun.intensity).extend(0.);
        }
    }
    (cube, direction, color)
}
thread_local! {
    /// Set inside lighting pool tasks so nested samples stay serial.
    static IN_LIGHT_POOL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub(crate) fn model_ambient(
    world: &crate::bsp::World,
    shadows: &crate::lighting::WorldShadow,
    position: Vec3,
) -> [[f32; 3]; 6] {
    let t = crate::performance::scope("model_lighting::field");
    let mut cube = world.ambient_field.sample(position.to_array());
    drop(t);
    let t_sky = crate::performance::scope("model_lighting::skylights");
    let axes = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::NEG_Z,
        Vec3::Z,
        Vec3::Y,
        Vec3::NEG_Y,
    ];
    // Further skylights (the sun is `suns[0]`, applied as the direct light)
    // add their visible light to the cube, as the engine lights models with
    // every world light.
    for sun in world.suns.iter().skip(1) {
        let direction = Vec3::from_array(sun.direction).normalize_or_zero();
        if direction == Vec3::ZERO || shadows.blocked(position.to_array(), direction.to_array()) {
            continue;
        }
        let color = Vec3::from_array(sun.intensity);
        for (face, axis) in axes.iter().enumerate() {
            let contribution = color * axis.dot(direction).max(0.);
            for c in 0..3 {
                cube[face][c] += contribution[c];
            }
        }
    }
    // The engine's light cache skips world lights outside the sample point's
    // PVS. Without it every sample shadow-traced lights across the whole map
    // (c2m2's ~230 fairground lights: ~80 ms for one view-model sample).
    drop(t_sky);
    let _t_local = crate::performance::scope("model_lighting::local");
    let here = world.visibility.cluster_at(position);
    // Cheap tests first; the shadow traces after, across the task pool when
    // many lights remain and this is not already a pool worker (one camera
    // jump's view-model sample spent ~37 ms tracing c2m2's lights serially).
    let mut lit: Vec<(Vec3, Vec3, [f32; 3])> = Vec::new();
    for light in &world.local_lights {
        if light.cluster >= 0 && !world.visibility.visible(here, &[light.cluster as u16]) {
            continue;
        }
        let delta = Vec3::from_array(light.origin) - position;
        let distance = delta.length() / 0.0254;
        if distance < 1. || (light.radius > 0. && distance > light.radius) {
            continue;
        }
        let direction = delta.normalize();
        let [a, b, c] = light.attenuation;
        let attenuation = (a + b * distance + c * distance * distance).max(1.);
        let mut color = Vec3::from_array(light.intensity) / attenuation;
        if light.spot {
            let cosine = Vec3::from_array(light.normal).dot(-direction);
            let [inner, outer] = light.cone;
            if cosine <= outer {
                continue;
            }
            color *= ((cosine - outer) / (inner - outer).max(0.001))
                .clamp(0., 1.)
                .powf(light.exponent.max(1.));
        }
        if color.max_element() < 0.0005 {
            continue;
        }
        lit.push((color, direction, light.origin));
    }
    let origin = position.to_array();
    // Few lights can still be slow: c2m2's reveal had one sample whose
    // handful of long traces took ~28 ms serially, so split from 3 up.
    let visible: Vec<bool> = if lit.len() > 2 && !IN_LIGHT_POOL.with(|f| f.get()) {
        let pool = bevy::tasks::ComputeTaskPool::get();
        let chunk = lit.len().div_ceil(pool.thread_num().max(1));
        pool.scope(|scope| {
            for part in lit.chunks(chunk) {
                scope.spawn(async move {
                    IN_LIGHT_POOL.with(|f| f.set(true));
                    let out: Vec<bool> = part.iter().map(|(_, _, o)| !shadows.blocked_to(origin, *o)).collect();
                    IN_LIGHT_POOL.with(|f| f.set(false));
                    out
                });
            }
        })
        .into_iter()
        .flatten()
        .collect()
    } else {
        lit.iter().map(|(_, _, o)| !shadows.blocked_to(origin, *o)).collect()
    };
    for ((color, direction, _), visible) in lit.into_iter().zip(visible) {
        if !visible {
            continue;
        }
        for (face, axis) in axes.iter().enumerate() {
            let contribution = color * axis.dot(direction).max(0.);
            for c in 0..3 {
                cube[face][c] += contribution[c];
            }
        }
    }
    cube
}
#[allow(clippy::too_many_arguments)]
fn update_model_lighting(
    mut commands: Commands,
    map: Option<Res<Map>>,
    shadows: Res<crate::lighting::WorldShadow>,
    cameras: Query<&GlobalTransform, With<GameCamera>>,
    meshes: Res<Assets<Mesh>>,
    mut materials: ResMut<Assets<SourceMaterial>>,
    mut table: (
        ResMut<LightingTable>,
        ResMut<crate::gpu_writes::GpuWrites>,
        RemovedComponents<DynamicModelLighting>,
    ),
    mut draws: Query<(
        Entity,
        &Mesh3d,
        &GlobalTransform,
        Option<&RenderLayers>,
        &mut MeshMaterial3d<SourceMaterial>,
        &mut DynamicModelLighting,
        Option<&bevy::camera::primitives::Aabb>,
        Option<&crate::l4d2r_animation::GpuSkin>,
        Option<&crate::l4d2r_animation::AnimatedDraw>,
        Option<&ChildOf>,
    )>,
    mut cache: Local<HashMap<[i32; 3], ([[f32; 3]; 6], Vec4, Vec4)>>,
    mut shared: Local<LightingMaterialCache>,
    graphics_originals: Option<Res<crate::graphics_options::Originals>>,
    fog_state: Option<Res<crate::native_fog::Runtime>>,
    appearances: Query<&crate::infected_appearance::Appearance>,
) {
    // No scene while a phone map change is between release and apply: drop
    // the lighting material variants now (they hold the old map's textures),
    // not when the next scene arrives after it has been prepared.
    let Some(map) = map else {
        shared.0.clear();
        cache.clear();
        table.0.reset();
        return;
    };
    let _timing = crate::performance::scope("viewer::update_model_lighting");
    let (mut table, mut writes, mut removed) = table;
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let legacy = *LEGACY.get_or_init(|| {
        std::env::args().any(|a| a == "--profile-legacy-lighting" || a == "--shared-lighting")
    });
    for entity in removed.read() {
        table.release(entity);
    }
    if map.is_changed() {
        cache.clear();
        shared.0.clear();
        table.reset();
        let globals = crate::native_fog::sky_globals(&map.0.world.entities);
        let at = GLOBAL_SLOT * LIGHT_SLOT_VEC4S;
        table.data[at..at + 3].copy_from_slice(&globals);
        // [3] shadow colour + enabled, [4] cast distance (crate::model_shadows).
        let shadows = crate::model_shadows::globals(&crate::model_shadows::read(&map.0.world.entities));
        table.data[at + 3..at + 5].copy_from_slice(&shadows);
        writes.write(
            table.buffer.id(),
            (at * 16) as u64,
            table_bytes(&table.data[at..at + LIGHT_SLOT_VEC4S]),
        );
    }
    // World far clipping follows the same live fog controller as CPU culling.
    // Update just this global vec4; changing fog must not reset model slots.
    if let Some(far) = fog_state.as_ref().and_then(|s| s.far()) {
        let at = GLOBAL_SLOT * LIGHT_SLOT_VEC4S + 2;
        if table.data[at].x != far {
            table.data[at].x = far;
            writes.write(table.buffer.id(), (at * 16) as u64, table_bytes(&table.data[at..at+1]));
        }
    }
    if let Some(color) = fog_state.as_ref().and_then(|s| s.sky_color()) {
        let at = GLOBAL_SLOT * LIGHT_SLOT_VEC4S + 1;
        if table.data[at] != color {
            table.data[at]=color;
            writes.write(table.buffer.id(),(at*16) as u64,table_bytes(&table.data[at..at+1]));
        }
    }
    // Many models revealed at once (a map start) each miss the per-cell
    // cache and sample every local light with a shadow trace: c2m2's reveal
    // spent ~80 ms in one frame. Gather this frame's missing cells for draws
    // that will resample, compute them across the task pool, then let the
    // loop below hit the cache.
    {
        let mut missing: Vec<([i32; 3], Vec3)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (entity, mesh, transform, layers, _, state, bounds, gpu, animated, parent) in draws.iter() {
            // Resampled in place by the loop below (a changed appearance, or
            // a slot lost to a table reset): those draws keep their position,
            // so skipping them here left ~2,000 cells of c2m2's reveal to
            // serial sampling (36-46 ms) after a full cache was cleared.
            let appearance = animated.and_then(|draw| appearances.get(draw.root).ok())
                .or_else(|| parent.and_then(|parent| appearances.get(parent.parent()).ok())).copied();
            let resample = state.appearance != appearance
                || (!legacy && state.slot.is_some() && table.owner(entity) != state.slot);
            // The view model samples at the camera, as the loop below does;
            // left out, its cell (often many lights through windows) was
            // sampled serially after the batch (36 ms at c2m2's reveal).
            if layers.is_some_and(|l| l.intersects(&RenderLayers::layer(2))) {
                let Some(position) = cameras.iter().next().map(|c| c.translation()) else { continue };
                if !resample && state.sample_position.is_some_and(|p| p.distance_squared(position) < 0.09) {
                    continue;
                }
                let cell = position.to_array().map(|v| (v / 0.5).floor() as i32);
                if !cache.contains_key(&cell) && seen.insert(cell) {
                    missing.push((cell, position));
                }
                continue;
            }
            let center = if gpu.is_some() {
                bounds.map_or(Vec3::Y * 0.9, |b| Vec3::from(b.center))
            } else if let Some(center) = state.center {
                center
            } else {
                // New draw: the same mesh-bounds centre the loop caches.
                let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = meshes
                    .get(&mesh.0)
                    .and_then(|mesh| mesh.attribute(Mesh::ATTRIBUTE_POSITION))
                else {
                    continue;
                };
                if positions.is_empty() {
                    continue;
                }
                let (lo, hi) = positions.iter().fold(
                    (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                    |(lo, hi), p| (lo.min(Vec3::from_array(*p)), hi.max(Vec3::from_array(*p))),
                );
                (lo + hi) * 0.5
            };
            let position = transform.transform_point(center) + Vec3::Y * 0.04;
            if !resample && state.sample_position.is_some_and(|p| p.distance_squared(position) < 0.09) {
                continue;
            }
            let cell = position.to_array().map(|v| (v / 0.5).floor() as i32);
            if !cache.contains_key(&cell) && seen.insert(cell) {
                missing.push((cell, position));
            }
        }
        // A full cache is cleared (the loop below does the same); draws
        // that keep their place keep their own sample. Skipping the batch
        // instead left c2m2's ~2,100 revealed cells to serial sampling once
        // its ~6,000 earlier cells filled the cache (38 ms frame).
        if missing.len() > 1 && missing.len() <= 8192 && cache.len() + missing.len() > 8192 {
            cache.clear();
        }
        if missing.len() > 1 && cache.len() + missing.len() <= 8192 {
            let _timing = crate::performance::scope("model_lighting::parallel_sample");
            let world = &map.0.world;
            let shadows = &*shadows;
            let pool = bevy::tasks::ComputeTaskPool::get();
            let chunk = missing.len().div_ceil(pool.thread_num().max(1));
            let results = pool.scope(|scope| {
                for part in missing.chunks(chunk) {
                    scope.spawn(async move {
                        IN_LIGHT_POOL.with(|f| f.set(true));
                        let out = part.iter()
                            .map(|&(cell, position)| (cell, sample_model_light(world, shadows, position)))
                            .collect::<Vec<_>>();
                        IN_LIGHT_POOL.with(|f| f.set(false));
                        out
                    });
                }
            });
            for (cell, light) in results.into_iter().flatten() {
                cache.insert(cell, light);
            }
        }
    }
    for (entity, mesh, transform, layers, mut material, mut state, bounds, gpu, animated, parent) in &mut draws {
        let appearance = animated.and_then(|draw| appearances.get(draw.root).ok())
            .or_else(|| parent.and_then(|parent| appearances.get(parent.parent()).ok())).copied();
        if materials.get(&material.0).is_some_and(|m| m.base.unlit) {
            continue;
        }
        if state.material.is_none() {
            let Some(native) = materials.get(&material.0) else {
                continue;
            };
            if native.base.unlit {
                continue;
            }
            state.material = Some(material.0.clone());
        }
        let center = if gpu.is_some() {
            bounds.map_or(Vec3::Y * 0.9, |b| Vec3::from(b.center))
        } else {
            *state.center.get_or_insert_with(|| {
                let _timing = crate::performance::scope("model_lighting::center");
                let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = meshes
                    .get(&mesh.0)
                    .and_then(|mesh| mesh.attribute(Mesh::ATTRIBUTE_POSITION))
                else {
                    return Vec3::ZERO;
                };
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for p in positions {
                    let p = Vec3::from_array(*p);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                if positions.is_empty() {
                    Vec3::ZERO
                } else {
                    (lo + hi) * 0.5
                }
            })
        };
        let viewmodel = layers.is_some_and(|l| l.intersects(&RenderLayers::layer(2)));
        let position = if viewmodel {
            cameras
                .iter()
                .next()
                .map(|c| c.translation())
                .unwrap_or(transform.translation())
        } else {
            transform.transform_point(center) + Vec3::Y * 0.04
        };
        // A table reset (map change) frees every slot; a draw still tagged with
        // one it no longer owns must re-acquire even if it has not moved.
        let stale_slot = !legacy && state.slot.is_some() && table.owner(entity) != state.slot;
        if !stale_slot
            && state.appearance == appearance
            && state
                .sample_position
                .is_some_and(|p| p.distance_squared(position) < 0.09)
        {
            continue;
        }
        let cell = position.to_array().map(|v| (v / 0.5).floor() as i32);
        if cache.len() > 8192 {
            cache.clear();
        }
        let (cube, sun_direction, sun_color) = *cache.entry(cell).or_insert_with(|| {
            let _timing = crate::performance::scope("model_lighting::sample");
            sample_model_light(&map.0.world, &shadows, position)
        });
        // The view model takes the same ambient light as world models, with no
        // fill: in c1m2's dark store retail's pistol is nearly black (live
        // comparison 19 Nov); it shares the world's tonemap scale now
        // (native_tonemap::share_world_exposure).
        // L4D2R_VM_LIGHT: debug, the view model's sampled light.
        if viewmodel && std::env::var_os("L4D2R_VM_LIGHT").is_some() {
            println!("L4D2R VM light at {position:?}: cube {cube:?} sun {sun_direction:?} {sun_color:?}");
        }
        let mut cube = cube.map(|c| Vec3::from_array(c).extend(0.));
        if let Some(appearance) = appearance {
            cube[0].w = appearance.skin as f32;
            cube[1].w = appearance.cloth as f32;
            cube[2].w = appearance.sheet as f32;
            cube[3].w = appearance.burn as f32;
            cube[4].w = 1.;
        }
        state.appearance = appearance;
        // Table path: one shared material per (base, viewmodel); this draw's
        // lighting lives in its table slot, selected by MeshTag.
        if !legacy {
            if let Some(slot) = table.slot(entity) {
                let base = state.material.clone().unwrap();
                let variant = table
                    .variants
                    .entry((base.id(), viewmodel))
                    .or_insert_with(|| {
                        let mut native = materials.get(&base).unwrap().clone();
                        // Graphics options record a new material's normal map
                        // as its original; hand over the base's real one.
                        if let Some(normal) = graphics_originals
                            .as_ref()
                            .and_then(|o| o.0.get(&base.id()))
                        {
                            native.base.normal_map_texture = normal.clone();
                        }
                        native.extension.settings.y = 1.;
                        native.extension.settings.z = if viewmodel { 1. } else { 0. };
                        materials.add(native)
                    })
                    .clone();
                // Wounded draws own their cutout uniform. Keep that material;
                // lighting still updates through this draw's shared table slot.
                if material.0 != variant && !materials.get(&material.0)
                    .is_some_and(|m| m.extension.wound_rows.iter().any(|r| *r != Vec4::ZERO)) {
                    material.0 = variant;
                }
                if state.slot != Some(slot) {
                    commands
                        .entity(entity)
                        .insert(bevy::mesh::MeshTag(slot + 1));
                    state.slot = Some(slot);
                }
                table.write(slot, cube, sun_direction, sun_color);
                state.sample_position = Some(position);
                continue;
            }
        }
        static UNIQUE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if materials.get(&material.0).is_some_and(|m|
            m.extension.wound_rows.iter().any(|r| *r != Vec4::ZERO)) || *UNIQUE.get_or_init(|| {
            let args = std::env::args().collect::<Vec<_>>();
            !args.iter().any(|a| a == "--shared-lighting")
                || args.iter().any(|a| a == "--profile-unique-lighting")
        }) {
            if material.0 == *state.material.as_ref().unwrap() {
                let mut native = materials.get(&material.0).unwrap().clone();
                native.extension.settings.y = 1.;
                material.0 = materials.add(native);
            }
            // Lighting is sampled per 0.5 m cell, so most moves land on the
            // same values. `get_mut` alone marks the asset changed and forces
            // Bevy to rebuild its GPU bind group, so only write real changes.
            let z = if viewmodel { 1. } else { 0. };
            if materials.get(&material.0).is_some_and(|m| {
                m.extension.settings.z == z
                    && m.extension.ambient_cube == cube
                    && m.extension.sun_direction == sun_direction
                    && m.extension.sun_color == sun_color
            }) {
                state.sample_position = Some(position);
                continue;
            }
            // Profiled call count = material writes (each rebuilds a bind group).
            let _write = crate::performance::scope("viewer::lighting_writes");
            if let Some(mut native) = materials.get_mut(&material.0) {
                native.extension.settings.z = if viewmodel { 1. } else { 0. };
                native.extension.ambient_cube = cube;
                native.extension.sun_direction = sun_direction;
                native.extension.sun_color = sun_color;
            }
        } else if let Some(handle) = shared.resolve(
            &mut materials,
            state.material.as_ref().unwrap(),
            viewmodel,
            cube,
            sun_direction,
            sun_color,
            graphics_originals
                .as_ref()
                .and_then(|o| o.0.get(&state.material.as_ref().unwrap().id())),
        ) {
            if material.0 != handle {
                material.0 = handle;
            }
        }
        state.sample_position = Some(position);
    }
    // Write only the slots in use into the fixed GPU buffer, which (with every
    // material bind group referencing it) is never replaced.
    if std::mem::take(&mut table.dirty) {
        let _upload = crate::performance::scope("viewer::lighting_table_upload");
        let used = table.next as usize * LIGHT_SLOT_VEC4S;
        writes.write(table.buffer.id(), 0, table_bytes(&table.data[..used]));
    }
}
impl MaterialExtension for SourceLighting {
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Metal on iPhone has 16 fragment samplers across ALL bind groups.
        // Keep palette, wound and lightmap samplers independent; share the
        // ordinary surface samplers instead of exceeding the hardware limit.
        if cfg!(any(target_os = "ios", feature = "mobile-materials")) {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_MOBILE_SAMPLERS".into());
            }
        }
        if key.bind_group_data.wounds {
            let wound_layout = layout.0.get_layout(&[
                crate::infected_gore::WOUND_BIND_POSITION.at_shader_location(8),
            ])?;
            if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
                debug_assert_eq!(buffer.array_stride, wound_layout.array_stride);
                buffer.attributes.extend(wound_layout.attributes);
            }
            descriptor.vertex.shader_defs.push("SOURCE_WOUNDS".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_WOUNDS".into());
            }
        }
        if key.bind_group_data.glass {
            let glass_layout=layout.0.get_layout(&[crate::source_break_surfaces::GLASS_MASK_UV.at_shader_location(9)])?;
            if let Some(buffer)=descriptor.vertex.buffers.first_mut() {buffer.attributes.extend(glass_layout.attributes);}
            descriptor.vertex.shader_defs.push("SOURCE_GLASS_MASK".into());
            if let Some(fragment)=descriptor.fragment.as_mut() {fragment.shader_defs.push("SOURCE_GLASS_MASK".into());}
        }
        if key.bind_group_data.infected {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_INFECTED".into());
            }
        }
        if key.bind_group_data.envmap {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_ENVMAP".into());
            }
        }
        if key.bind_group_data.blend {
            let blend_layout = layout.0.get_layout(&[SOURCE_BLEND.at_shader_location(13)])?;
            if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
                buffer.attributes.extend(blend_layout.attributes);
            }
            descriptor.vertex.shader_defs.push("SOURCE_BLEND".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_BLEND".into());
                if key.bind_group_data.blend_modulate {
                    fragment.shader_defs.push("SOURCE_BLEND_MODULATE".into());
                }
            }
        }
        if key.bind_group_data.cap_phong {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_CAP_PHONG".into());
            }
        }
        if key.bind_group_data.bumped {
            let bump_layout = layout.0.get_layout(&[SOURCE_BUMP.at_shader_location(14)])?;
            if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
                buffer.attributes.extend(bump_layout.attributes);
            }
            descriptor.vertex.shader_defs.push("SOURCE_BUMPED".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_BUMPED".into());
            }
        }
        if key.bind_group_data.half_lambert {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_HALF_LAMBERT".into());
            }
        }
        if key.bind_group_data.fence {
            // Valve's fence render state: blended, but alpha tested and
            // depth writing like an opaque cutout.
            if let Some(depth) = descriptor.depth_stencil.as_mut() {
                depth.depth_write_enabled = Some(true);
            }
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_FENCE".into());
            }
        }
        if key.bind_group_data.sky {
            descriptor
                .vertex
                .shader_defs
                .push("SOURCE_SKY_DEPTH".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SOURCE_SKY_DEPTH".into());
            }
        }
        Ok(())
    }

    fn prepass_vertex_shader() -> bevy::shader::ShaderRef {
        "shaders/source_prepass_vertex.wgsl".into()
    }
    fn prepass_fragment_shader() -> bevy::shader::ShaderRef {
        "shaders/source_prepass.wgsl".into()
    }

    fn vertex_shader() -> bevy::shader::ShaderRef {
        "shaders/source_vertex.wgsl".into()
    }
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "shaders/source_lighting.wgsl".into()
    }
}
pub(crate) type SourceMaterial = ExtendedMaterial<StandardMaterial, SourceLighting>;
/// World texture coordinates divided by the base texture's real size, as
/// the engine maps them (texinfo texels / material mapping width), instead of
/// the compile-time texdata size (c2m1's nature/blendrockcavewall texdata is
/// its 128x128 tool texture, so it tiled 4x too often).
pub(crate) fn world_uv(surface: &crate::bsp::Surface, mapping: Option<&[f32; 2]>) -> Vec<[f32; 2]> {
    match mapping {
        _ if surface.texdata_size == crate::bsp::OVERLAY_UV => surface.uv.clone(),
        Some(size) if *size != surface.texdata_size => {
            let scale = [surface.texdata_size[0] / size[0], surface.texdata_size[1] / size[1]];
            surface.uv.iter().map(|uv| [uv[0] * scale[0], uv[1] * scale[1]]).collect()
        }
        _ => surface.uv.clone(),
    }
}
/// Displacement blend weight (0 = $basetexture, 1 = $basetexture2).
/// Bumped lightmaps: atlas u distance between basis lightmaps (0: none).
pub(crate) const SOURCE_BUMP: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new("Source_Bump", 0x5ce0_b0b9, bevy::render::render_resource::VertexFormat::Float32);
pub(crate) const SOURCE_BLEND: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new("Source_Blend", 0x5ce0_b1e4, bevy::render::render_resource::VertexFormat::Float32);
#[cfg(test)]
mod world_uv_tests {
    #[test]
    fn world_uv_uses_the_real_base_texture_size() {
        let surface = crate::bsp::Surface {
            clusters: vec![],
            sky: false,
            model: 0,
            material: "nature/blendrockcavewall".into(),
            positions: vec![],
            uv: vec![[8., 4.]],
            light_uv: vec![],
            blend: vec![],
            bump: vec![],
            texdata_size: [128., 128.],
            hull: true,
        };
        // texdata from the 128x128 tool texture; rockwater is 1024x1024.
        assert_eq!(super::world_uv(&surface, Some(&[1024., 1024.])), vec![[1., 0.5]]);
        assert_eq!(super::world_uv(&surface, Some(&[128., 128.])), vec![[8., 4.]]);
        assert_eq!(super::world_uv(&surface, None), vec![[8., 4.]]);
    }
}
#[cfg(test)]
mod lighting_cache_tests {
    use super::*;
    #[test]
    fn batching_releases_private_meshes_and_keeps_shared_source_alive() {
        let mut meshes = Assets::<Mesh>::default();
        let mut source = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        source.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[1., 0., 0.]; 3]);
        source.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.2, 0.3, 0.4, 1.]; 3]);
        let shared = meshes.add(source);
        let mut batch = crate::static_geometry::empty_like(meshes.get(&shared).unwrap());
        assert_eq!(release_temporary_mesh(&mut meshes, &shared, &shared), 0);
        for i in 0..128 {
            let private = meshes.add(meshes.get(&shared).unwrap().clone());
            assert!(crate::static_geometry::append(&mut batch, meshes.get(&private).unwrap(),
                &Transform::from_translation(Vec3::X * i as f32)));
            assert_eq!(release_temporary_mesh(&mut meshes, &private, &shared), 3 * (12 + 16));
            assert!(meshes.get(&private).is_none());
            assert_eq!(meshes.len(), 1);
        }
        assert_eq!(batch.count_vertices(), 128 * 3);
        assert_eq!(meshes.get(&shared).unwrap().count_vertices(), 3);
        let (before, after) = crate::static_geometry::index(&mut batch);
        assert_eq!((before, after), (384, 128));
        assert_eq!(batch.indices().unwrap().len(), 384);
    }
    #[test]
    fn lightmap_upload_releases_float_pixels_and_preserves_exact_half_values() {
        let values = vec![[0., 0.5, 4., 1.], [8., 2., 0.25, 1.]];
        let expected: Vec<u8> = values.iter().flat_map(|p| p.iter().flat_map(|v| half::f16::from_f32(*v).to_le_bytes())).collect();
        let mut atlas = crate::bsp::LightAtlas { width: 2, height: 1, pixels: values };
        assert_eq!(take_lightmap_bytes(&mut atlas), expected);
        assert!(atlas.pixels.is_empty());
        assert_eq!(atlas.pixels.capacity(), 0);
        assert_eq!((atlas.width, atlas.height), (2, 1));
    }
    #[test]
    fn lighting_table_reuses_slots_and_only_uploads_changes() {
        let mut table = LightingTable {
            buffer: Handle::default(),
            data: vec![Vec4::ZERO; LIGHT_SLOTS * LIGHT_SLOT_VEC4S],
            owners: HashMap::new(),
            free: vec![],
            next: 0,
            dirty: false,
            variants: HashMap::new(),
        };
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert_eq!(table.slot(a), Some(0));
        assert_eq!(table.slot(b), Some(1));
        assert_eq!(table.slot(a), Some(0));
        let cube = [Vec4::ONE; 6];
        assert!(table.write(1, cube, Vec4::Y, Vec4::X));
        assert!(table.dirty);
        table.dirty = false;
        // Same lighting again must not trigger an upload.
        assert!(!table.write(1, cube, Vec4::Y, Vec4::X));
        assert!(!table.dirty);
        assert_eq!(
            table_bytes(&table.data).len(),
            LIGHT_SLOTS * LIGHT_SLOT_VEC4S * 16
        );
        // A despawned draw's slot goes to the next draw.
        table.release(b);
        let c = Entity::from_raw_u32(3).unwrap();
        assert_eq!(table.slot(c), Some(1));
        assert_eq!(table.owner(b), None);
        // A map change frees everything; survivors then re-acquire.
        table.reset();
        assert_eq!(table.owner(a), None);
        assert_eq!(table.slot(c), Some(0));
        // The table is bounded: overflowing draws fall back to legacy lighting.
        for i in 0..LIGHT_SLOTS as u32 {
            table.slot(Entity::from_raw_u32(100 + i).unwrap());
        }
        assert_eq!(table.slot(Entity::from_raw_u32(99_999).unwrap()), None);
    }
    #[test]
    fn world_surfaces_share_one_lightmapped_material_per_base_material() {
        let mut materials = Assets::<SourceMaterial>::default();
        let mut images = Assets::<Image>::default();
        let light = images.add(Image::default());
        let base = materials.add(SourceMaterial {
            base: StandardMaterial {
                lightmap_exposure: 1.5,
                ..default()
            },
            extension: SourceLighting {
                settings: Vec4::new(1., 0., 0., 0.),
                ambient_cube: [Vec4::ZERO; 6],
                sun_direction: Vec4::ZERO,
                sun_color: Vec4::ZERO,
                lightmap: None,
                lightmap_params: Vec4::ZERO,
                lighting_table: Handle::default(),
                emission: None,
                emission_tint: Vec4::ZERO,
                wound_rows: [Vec4::ZERO; 12],
                wound: WoundUniform::default(),
                wound_mask: None,
                infected_base: None,
                infected_palette: None,
                infected_lookup: Vec4::ZERO,
                infected_phong: InfectedPhongUniform::default(),
                infected_detail: None,
                infected_burn: None,
                cap_phong: CapPhongUniform::default(),
                glass_mask: None,
                envmap: None,
                envmap_tint: Vec4::ZERO,
                envmap_params: Vec4::ZERO,
                bump: Vec4::ZERO,
                bump_map: None,
                blend_texture: None,
                blend_modulate: None,
            },
        });
        let mut cache = HashMap::new();
        let first = world_material(&mut cache, &mut materials, &base, &light, false);
        // Thousands of brush entities reuse one material, not one slot each.
        for _ in 0..4000 {
            assert_eq!(
                world_material(&mut cache, &mut materials, &base, &light, false),
                first
            );
        }
        assert_eq!(materials.len(), 2);
        let lit = materials.get(&first).unwrap();
        assert_eq!(lit.extension.lightmap.as_ref(), Some(&light));
        assert_eq!(lit.extension.lightmap_params, Vec4::new(1., 1.5, 0., 0.));
        // Props/models keep the unlightmapped base material.
        assert!(materials.get(&base).unwrap().extension.lightmap.is_none());
        assert_eq!(
            world_material(&mut cache, &mut materials, &base, &light, true),
            base
        );
    }
    #[test]
    fn identical_lighting_reuses_materials_without_mutating_other_regions() {
        let mut materials = Assets::<SourceMaterial>::default();
        let base = materials.add(SourceMaterial {
            base: StandardMaterial::default(),
            extension: SourceLighting {
                settings: Vec4::new(1., 0., 0., 0.),
                ambient_cube: [Vec4::ZERO; 6],
                sun_direction: Vec4::ZERO,
                sun_color: Vec4::ZERO,
                lightmap: None,
                lightmap_params: Vec4::ZERO,
                lighting_table: Handle::default(),
                emission: None,
                emission_tint: Vec4::ZERO,
                wound_rows: [Vec4::ZERO; 12],
                wound: WoundUniform::default(),
                wound_mask: None,
                infected_base: None,
                infected_palette: None,
                infected_lookup: Vec4::ZERO,
                infected_phong: InfectedPhongUniform::default(),
                infected_detail: None,
                infected_burn: None,
                cap_phong: CapPhongUniform::default(),
                glass_mask: None,
                envmap: None,
                envmap_tint: Vec4::ZERO,
                envmap_params: Vec4::ZERO,
                bump: Vec4::ZERO,
                bump_map: None,
                blend_texture: None,
                blend_modulate: None,
            },
        });
        let mut cache = LightingMaterialCache::default();
        let cube = [Vec4::splat(0.2); 6];
        let first = cache
            .resolve(&mut materials, &base, false, cube, Vec4::X, Vec4::ONE, None)
            .unwrap();
        for _ in 0..1000 {
            assert_eq!(
                cache
                    .resolve(&mut materials, &base, false, cube, Vec4::X, Vec4::ONE, None)
                    .unwrap(),
                first
            );
        }
        assert_eq!(materials.len(), 2);
        let mut brighter = cube;
        brighter[0].x = f32::from_bits(brighter[0].x.to_bits() + 1);
        let different = cache
            .resolve(
                &mut materials,
                &base,
                false,
                brighter,
                Vec4::X,
                Vec4::ONE,
                None,
            )
            .unwrap();
        assert_ne!(
            different, first,
            "No quantization of native lighting is permitted"
        );
        assert_eq!(materials.get(&first).unwrap().extension.ambient_cube, cube);
        let view = cache
            .resolve(&mut materials, &base, true, cube, Vec4::X, Vec4::ONE, None)
            .unwrap();
        assert_ne!(view, first);
        assert_eq!(materials.get(&base).unwrap().extension.settings.y, 0.);
        cache.0.clear();
        assert_ne!(
            cache
                .resolve(&mut materials, &base, false, cube, Vec4::X, Vec4::ONE, None)
                .unwrap(),
            first
        );
    }
}
pub(crate) fn warm_material_key(
    m: &SourceMaterial,
) -> (bevy::pbr::StandardMaterialKey, SourceLightingKey) {
    (
        bevy::pbr::StandardMaterialKey::from(&m.base),
        SourceLightingKey::from(&m.extension),
    )
}
#[derive(Component)]
pub struct GameCamera;
#[derive(Resource)]
struct GameRender;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderSetup {
    Scene,
}
#[derive(Resource)]
pub struct RenderModels(pub HashMap<ModelKey, Vec<(Handle<Mesh>, Handle<SourceMaterial>)>>);
#[derive(Resource)]
pub struct SourceMaterials(pub HashMap<String,Handle<SourceMaterial>>);
/// Retain UI/core images while releasing outgoing 3D data before map loading.
pub(crate) fn release_mobile_map_assets(world: &mut World) {
    use std::collections::HashSet;
    // Most mobile meshes and textures live only on the GPU (no main-world
    // copy), so removing assets below never reaches them: they stayed alive
    // through their entities' handles until the next map was applied, and
    // the next map was prepared on top of them (iPhone memory kill on a
    // campaign change). Detach the handles now so the GPU copies are freed
    // during the release wait. Entities stay; the map apply despawns them.
    let drawn: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<Mesh3d>, With<MeshMaterial3d<SourceMaterial>>, With<MeshMaterial3d<StandardMaterial>>, With<Lightmap>)>>()
        .iter(world)
        .collect();
    for entity in &drawn {
        if let Ok(mut e) = world.get_entity_mut(*entity) {
            e.remove::<(Mesh3d, MeshMaterial3d<SourceMaterial>, MeshMaterial3d<StandardMaterial>, Lightmap)>();
        }
    }
    println!("Mobile map release: detached meshes/materials from {} entities", drawn.len());
    let (materials, images) = {
        let assets = world.resource::<Assets<SourceMaterial>>();
        let mut images = HashSet::new();
        let mut materials = Vec::new();
        for (id, material) in assets.iter() {
            materials.push(id);
            let b = &material.base;
            let e = &material.extension;
            for image in [
                &b.base_color_texture, &b.emissive_texture, &b.normal_map_texture,
                &b.metallic_roughness_texture, &b.occlusion_texture, &b.depth_map,
                &e.lightmap, &e.emission, &e.wound_mask, &e.infected_base,
                &e.infected_palette, &e.infected_detail, &e.infected_burn,
                &e.glass_mask, &e.blend_texture, &e.blend_modulate, &e.envmap,
            ].into_iter().flatten() { images.insert(image.id()); }
        }
        (materials, images)
    };
    let meshes: Vec<_> = world.resource::<Assets<Mesh>>().ids().collect();
    let counts = (materials.len(), images.len(), meshes.len());
    for id in materials { world.resource_mut::<Assets<SourceMaterial>>().remove(id); }
    for id in meshes { world.resource_mut::<Assets<Mesh>>().remove(id); }
    if let Some(mut pending) = world.get_resource_mut::<crate::graphics_options::PendingTextures>() {
        pending.0.retain(|(handle, _)| !images.contains(&handle.id()));
    }
    for id in images { world.resource_mut::<Assets<Image>>().remove(id); }
    if let Some(mut models) = world.get_resource_mut::<RenderModels>() { models.0.clear(); }
    if let Some(mut materials) = world.get_resource_mut::<SourceMaterials>() { materials.0.clear(); }
    println!("Mobile map release: {} materials, {} images, {} meshes; UI/core images retained", counts.0, counts.1, counts.2);
}
/// Reproduce the iPhone's real sampler budget on a desktop GPU for captures.
pub(crate) fn render_plugin() -> bevy::render::RenderPlugin {
    use bevy::render::settings::{RenderCreation, WgpuLimits, WgpuSettings};
    let mut plugin = bevy::render::RenderPlugin::default();
    if std::env::args().any(|a| a == "--profile-mobile-sampler-limit") {
        plugin.render_creation = RenderCreation::Automatic(Box::new(WgpuSettings {
            constrained_limits: Some(WgpuLimits {
                max_texture_dimension_1d: u32::MAX,
                max_texture_dimension_2d: u32::MAX,
                max_texture_dimension_3d: u32::MAX,
                max_texture_array_layers: u32::MAX,
                max_bind_groups: u32::MAX,
                max_bindings_per_bind_group: u32::MAX,
                max_dynamic_uniform_buffers_per_pipeline_layout: u32::MAX,
                max_dynamic_storage_buffers_per_pipeline_layout: u32::MAX,
                max_sampled_textures_per_shader_stage: u32::MAX,
                max_samplers_per_shader_stage: 16,
                max_storage_buffers_per_shader_stage: u32::MAX,
                max_storage_textures_per_shader_stage: u32::MAX,
                max_uniform_buffers_per_shader_stage: u32::MAX,
                max_binding_array_elements_per_shader_stage: u32::MAX,
                max_binding_array_acceleration_structure_elements_per_shader_stage: u32::MAX,
                max_binding_array_sampler_elements_per_shader_stage: u32::MAX,
                max_uniform_buffer_binding_size: u64::MAX,
                max_storage_buffer_binding_size: u64::MAX,
                max_vertex_buffers: u32::MAX,
                max_buffer_size: u64::MAX,
                max_vertex_attributes: u32::MAX,
                max_vertex_buffer_array_stride: u32::MAX,
                max_inter_stage_shader_variables: u32::MAX,
                min_uniform_buffer_offset_alignment: 0,
                min_storage_buffer_offset_alignment: 0,
                max_color_attachments: u32::MAX,
                max_color_attachment_bytes_per_sample: u32::MAX,
                max_compute_workgroup_storage_size: u32::MAX,
                max_compute_invocations_per_workgroup: u32::MAX,
                max_compute_workgroup_size_x: u32::MAX,
                max_compute_workgroup_size_y: u32::MAX,
                max_compute_workgroup_size_z: u32::MAX,
                max_compute_workgroups_per_dimension: u32::MAX,
                max_immediate_size: u32::MAX,
                max_non_sampler_bindings: u32::MAX,
                max_task_mesh_workgroup_total_count: u32::MAX,
                max_task_mesh_workgroups_per_dimension: u32::MAX,
                max_task_invocations_per_workgroup: u32::MAX,
                max_task_invocations_per_dimension: u32::MAX,
                max_mesh_invocations_per_workgroup: u32::MAX,
                max_mesh_invocations_per_dimension: u32::MAX,
                max_task_payload_size: u32::MAX,
                max_mesh_output_vertices: u32::MAX,
                max_mesh_output_primitives: u32::MAX,
                max_mesh_output_layers: u32::MAX,
                max_mesh_multiview_view_count: u32::MAX,
                max_blas_primitive_count: u32::MAX,
                max_blas_geometry_count: u32::MAX,
                max_tlas_instance_count: u32::MAX,
                max_acceleration_structures_per_shader_stage: u32::MAX,
                max_multiview_view_count: u32::MAX,
            }),
            ..default()
        }));
    }
    plugin
}

/// Survivor body/clothing materials draw both faces. Skinned clothing that
/// folds under a raised, twisted arm (Nick's jacket while aiming) turns a few
/// triangles away from the camera; culling them opened see-through holes.
/// Shader values for a material's envmap and the chosen capture.
fn envmap_values(env: &crate::material::EnvmapSettings, image: Option<Handle<Image>>) -> (Option<Handle<Image>>, Vec4, Vec4) {
    let on = image.is_some();
    // LightmappedGeneric sets $envmaptint gamma-to-linear
    // (SetEnvMapTintPixelShaderDynamicState(.., true)). L4D2R_ENVMAP_RAW_TINT:
    // debug, the authored value as-is.
    let tint = if std::env::var_os("L4D2R_ENVMAP_RAW_TINT").is_some() {
        env.tint
    } else {
        env.tint.map(|c| c.max(0.).powf(2.2))
    };
    (
        image,
        Vec4::new(tint[0], tint[1], tint[2], if on { 1. } else { 0. }),
        Vec4::new(env.saturation, env.contrast, env.base_alpha_mask as u8 as f32, env.normal_alpha_mask as u8 as f32),
    )
}
/// A GPU cube from a decoded HDR capture (all stored mips).
fn cubemap_image(c: &crate::vtf::Cubemap) -> Image {
    let size = Extent3d { width: c.size, height: c.size, depth_or_array_layers: 6 };
    let mut image = Image::new(
        size,
        TextureDimension::D2,
        vec![0; (c.size * c.size * 8 * 6) as usize],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(c.data.clone());
    image.texture_descriptor.mip_level_count = c.levels;
    image.texture_view_descriptor = Some(bevy::render::render_resource::TextureViewDescriptor {
        dimension: Some(bevy::render::render_resource::TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    image
}
/// A model/world material's SourceMaterial: the map loader passes its
/// preloaded texture tables, crate::runtime_props loads them on demand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn source_material(
    name: &str,
    m: &crate::material::Material,
    diffuse: &dyn Fn(&str) -> Option<Handle<Image>>,
    normal: &dyn Fn(&str) -> Option<Handle<Image>>,
    (envmap, envmap_tint, envmap_params): (Option<Handle<Image>>, Vec4, Vec4),
    palette_heights: &HashMap<String, f32>,
    lighting_buffer: Handle<bevy::render::storage::ShaderBuffer>,
) -> SourceMaterial {
    SourceMaterial {
            base: StandardMaterial {
                base_color: Color::linear_rgba(m.color[0], m.color[1], m.color[2], m.color[3]),
                base_color_texture: m.base.as_ref().filter(|_| m.shader != "infected").and_then(|p| diffuse(p)),
                normal_map_texture: m.normal.as_ref().and_then(|p| normal(p)),
                flip_normal_map_y: true,
                perceptual_roughness: 0.85,
                lightmap_exposure: 1.,
                // Source culls back faces unless $nocull, translucent or not:
                // two-sided glass models (Hotel's hotel_window_glass001, a
                // quad per side) otherwise blend twice and darken the view.
                double_sided: m.nocull || survivor_skin(name),
                cull_mode: if m.nocull || survivor_skin(name) {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                alpha_mode: match m.alpha {
                    _ if m.fence => AlphaMode::Blend,
                    1 => AlphaMode::Mask(m.cutoff),
                    2 => AlphaMode::Blend,
                    3 => AlphaMode::Add,
                    _ => AlphaMode::Opaque,
                },
                unlit: m.unlit,
                ..default()
            },
            extension: SourceLighting {
                settings: Vec4::new(1., 0., 0., 0.),
                ambient_cube: [Vec4::ZERO; 6],
                sun_direction: Vec4::ZERO,
                sun_color: Vec4::ZERO,
                lightmap: None,
                // z: the fence state's alpha-test reference (SOURCE_FENCE).
                lightmap_params: Vec4::new(
                    0.,
                    0.,
                    if m.fence { m.cutoff.max(1. / 255.) } else { 0. },
                    // w: $halflambert (SOURCE_HALF_LAMBERT).
                    if m.half_lambert { 1. } else { 0. },
                ),
                lighting_table: lighting_buffer,
                wound_rows: [Vec4::ZERO; 12],
                wound: m.wound.as_ref().map(WoundUniform::from_material).unwrap_or_default(),
                wound_mask: m.wound.as_ref().and_then(|w| normal(&w.texture)),
                infected_base: m.base.as_ref().filter(|_| m.shader == "infected").and_then(|p| normal(p)),
                infected_palette: m.palette.as_ref().and_then(|(p, _, _)| diffuse(p)),
                infected_lookup: m.palette.as_ref().map_or(Vec4::ZERO, |(p, skin, cloth)| {
                    let height = palette_heights.get(p).copied().unwrap_or(16.);
                    let sheet = m.wound.as_ref().map_or(0, |w| w.sheet);
                    Vec4::new((sheet / 2) as f32 * 0.5, (sheet % 2) as f32 * 0.5,
                        (*skin as f32 + 0.5) / height, (*cloth as f32 + 0.5) / height)
                }),
                infected_phong: m.infected_phong.as_ref().map(InfectedPhongUniform::from).unwrap_or_default(),
                infected_detail: m.infected_detail.as_ref().and_then(|p| normal(p)),
                infected_burn: m.infected_burn.as_ref().and_then(|p| diffuse(p)),
                cap_phong: m.cap_phong.as_ref().map(CapPhongUniform::from).unwrap_or_default(),
                glass_mask: None,
                envmap,
                envmap_tint,
                envmap_params,
                // y: $ssbump; x is set on the lightmapped world copy.
                bump: Vec4::new(0., if m.ssbump { 1. } else { 0. }, 0., 0.),
                // Models: the Phong exponent texture shares the slot.
                bump_map: m.bump.as_ref()
                    .or_else(|| m.cap_phong.as_ref().and_then(|c| c.exponent_texture.as_ref()))
                    .and_then(|p| normal(p)),
                // Debug: L4D2R_NO_BLEND drops $basetexture2; L4D2R_BLEND_SWAP
                // blends $basetexture with itself.
                blend_texture: if std::env::var_os("L4D2R_NO_BLEND").is_some() {
                    None
                } else if std::env::var_os("L4D2R_BLEND_SWAP").is_some() {
                    m.base2.as_ref().and(m.base.as_ref()).and_then(|p| diffuse(p))
                } else {
                    m.base2.as_ref().and_then(|p| diffuse(p))
                },
                blend_modulate: m.blend_modulate.as_ref().and_then(|p| normal(p)),
                emission: m.emission_mask.as_ref()
                    .or_else(|| m.cap_phong.as_ref().and_then(|c| c.lightwarp.as_ref()))
                    .and_then(|p| normal(p)),
                emission_tint: Vec4::new(m.emission_tint[0],m.emission_tint[1],m.emission_tint[2],if m.emission.is_some() {if m.emission_mask.is_some() {2.} else {1.}} else {0.}),
            },
    }
}

fn survivor_skin(material: &str) -> bool {
    material.to_ascii_lowercase().replace('\\', "/").contains("models/survivors/")
}
pub fn install_game(app: &mut App, scene: Scene, capture: Option<String>) {
    app.add_systems(Startup, |device: Option<Res<bevy::render::renderer::RenderDevice>>| {
        if cfg!(target_os = "ios") || std::env::args().any(|a| a == "--profile-mobile-sampler-limit") {
            if let Some(device) = device {
                eprintln!("iPhone sampler validation: limit={} compact={}",
                    device.limits().max_samplers_per_shader_stage,
                    cfg!(any(target_os = "ios", feature = "mobile-materials")));
            }
        }
    });
    app.add_plugins((
        MaterialPlugin::<SourceMaterial>::default(),
        MaterialPlugin::<crate::source_water::WaterMaterial>::default(),
        MaterialPlugin::<crate::source_sun::SunMaterial>::default(),
        AutoExposurePlugin,
    ))
    .add_systems(Startup, crate::native_tonemap::install_linear_lut)
    .add_systems(Startup, crate::native_tonemap::install_exposure_readback.before(RenderSetup::Scene))
    .add_plugins(crate::gpu_writes::GpuWritesPlugin)
    .init_resource::<LightingTable>()
    .init_resource::<crate::native_fog::Runtime>()
    .insert_resource(Map(scene))
    .init_resource::<Fly>()
    .insert_resource(GameRender)
    .add_systems(Startup, setup.in_set(RenderSetup::Scene))
    .add_systems(PostUpdate, crate::source_sun::update.before(bevy::transform::TransformSystems::Propagate))
    .add_systems(PostUpdate, (crate::native_tonemap::sync, crate::native_tonemap::share_world_exposure))
    .add_systems(PostUpdate, crate::native_fog::sync.after(bevy::transform::TransformSystems::Propagate))
    .add_systems(PostUpdate, crate::color_correction::sync.after(crate::native_fog::sync))
    .add_plugins(crate::engine_post::EnginePostPlugin)
    .add_systems(
        PostUpdate,
        cull_pvs
            .after(crate::native_fog::sync)
            .after(bevy::camera::visibility::VisibilitySystems::CalculateBounds)
            .after(bevy::transform::TransformSystems::Propagate)
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
    )
    .add_systems(
        PostUpdate,
        update_model_lighting.in_set(ModelLightingSystems::Write).after(bevy::transform::TransformSystems::Propagate).after(crate::native_fog::sync),
    );
    if let Some(path) = capture {
        app.insert_resource(Capture {
            path,
            origin: None,
            angles: Some([8., 45., 0.]),
        })
        .add_systems(Update, capture_frame);
    }
}
#[derive(Resource)]
pub(crate) struct Map(pub(crate) Scene);
pub(crate) fn replace_scene(world: &mut World, scene: Scene) {
    use bevy::ecs::system::RunSystemOnce;
    world.insert_resource(Map(scene));
    world
        .run_system_once(setup)
        .expect("Rebuild native render scene");
}
#[derive(Resource, Default)]
struct Fly {
    yaw: f32,
    pitch: f32,
}
#[derive(Resource)]
struct Capture {
    path: String,
    origin: Option<[f32; 3]>,
    angles: Option<[f32; 3]>,
}
#[derive(Resource)]
pub(crate) struct CaptureImage {
    pub(crate) image: Handle<Image>,
    frames: usize,
    requested: bool,
}
pub fn capture(scene: Scene, path: String, origin: Option<[f32; 3]>, angles: Option<[f32; 3]>) {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>(),
        )
        .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1. / 60.),
        ))
        .add_plugins(MaterialPlugin::<SourceMaterial>::default())
        .add_plugins(MaterialPlugin::<crate::source_water::WaterMaterial>::default())
        .add_plugins(MaterialPlugin::<crate::source_sun::SunMaterial>::default())
        .add_systems(PostUpdate, crate::source_sun::update.before(bevy::transform::TransformSystems::Propagate))
        .add_plugins(AutoExposurePlugin)
        .add_systems(Startup, crate::native_tonemap::install_linear_lut)
        .add_plugins(crate::gpu_writes::GpuWritesPlugin)
        .init_resource::<LightingTable>()
        .insert_resource(Map(scene))
        .init_resource::<Fly>()
        .insert_resource(Capture {
            path,
            origin,
            angles,
        })
        .insert_resource(GlobalAmbientLight {
            brightness: 80.,
            affects_lightmapped_meshes: false,
            ..default()
        })
        .insert_resource(ClearColor(Color::srgb(0.13, 0.17, 0.22)))
        .add_systems(Startup, setup)
        .add_systems(Update, capture_frame)
        .run();
}
#[derive(Default)]
struct CaptureSequence {
    configured: bool,
    directory: Option<std::path::PathBuf>,
    submitted: usize,
    completed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
fn capture_frame(
    mut commands: Commands,
    config: Res<Capture>,
    mut image: Option<ResMut<CaptureImage>>,
    mut limit: Local<usize>,
    mut sequence: Local<CaptureSequence>,
    mut exit: MessageWriter<bevy::app::AppExit>,
    warmup: Option<Res<crate::render_warmup::Warmup>>,
) {
    if warmup.is_some() {
        return;
    }
    let Some(ref mut image) = image else {
        return;
    };
    image.frames += 1;
    if *limit == 0 {
        let args = std::env::args().collect::<Vec<_>>();
        *limit = args
            .iter()
            .position(|a| a == "--capture-frames")
            .and_then(|i| args.get(i + 1))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(180)
            .clamp(180, 200_000); // long fixtures (finale runs) need minutes
    }
    if !sequence.configured {
        sequence.configured = true;
        let args = std::env::args().collect::<Vec<_>>();
        if let Some(path) = args.windows(2).find(|a| a[0] == "--record-frames") {
            let directory = std::path::PathBuf::from(&path[1]);
            if let Err(error) = std::fs::create_dir_all(&directory) {
                eprintln!("Cannot create frame output directory: {error}");
                exit.write(bevy::app::AppExit::error());
                return;
            }
            sequence.directory = Some(directory);
        }
    }
    if let Some(directory) = &sequence.directory {
        // Opt-in 30fps image sequence from the actual rendered target. With
        // --capture-simulate the game advances at 60Hz regardless of PNG I/O.
        if image.frames <= *limit && (image.frames - 1) % 2 == 0 {
            let file = directory.join(format!("{:06}.png", sequence.submitted));
            let completed = std::sync::Arc::clone(&sequence.completed);
            let failed = std::sync::Arc::clone(&sequence.failed);
            sequence.submitted += 1;
            commands.spawn(bevy::render::view::screenshot::Screenshot::image(image.image.clone()))
                .observe(move |captured: On<bevy::render::view::screenshot::ScreenshotCaptured>| {
                    let result = captured.image.clone().try_into_dynamic()
                        .map_err(|e| format!("{e:?}"))
                        .and_then(|img| img.to_rgb8().save(&file).map_err(|e| e.to_string()));
                    if let Err(error) = result {
                        eprintln!("Cannot save showcase frame: {error}");
                        failed.store(true, std::sync::atomic::Ordering::Release);
                    }
                    completed.fetch_add(1, std::sync::atomic::Ordering::Release);
                });
        }
        if image.frames > *limit && sequence.completed.load(std::sync::atomic::Ordering::Acquire) == sequence.submitted {
            println!("Frame sequence complete: {} frames", sequence.submitted);
            exit.write(if sequence.failed.load(std::sync::atomic::Ordering::Acquire) { bevy::app::AppExit::error() } else { bevy::app::AppExit::Success });
        } else if image.frames > *limit + 720 {
            eprintln!("Frame sequence readback timeout");
            exit.write(bevy::app::AppExit::error());
        }
        return;
    }
    if image.frames == *limit {
        image.requested = true;
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::image(
                image.image.clone(),
            ))
            .observe(bevy::render::view::screenshot::save_to_disk(
                config.path.clone(),
            ))
            .observe(
                |_: On<bevy::render::view::screenshot::ScreenshotCaptured>,
                 mut exit: MessageWriter<bevy::app::AppExit>| {
                    exit.write(bevy::app::AppExit::Success);
                },
            );
    }
    if image.frames > *limit + 720 {
        eprintln!("Capture timeout");
        exit.write(bevy::app::AppExit::error());
    }
}
pub fn run(scene: Scene) {
    let angles = scene
        .world
        .entities
        .iter()
        .find(|e| {
            e.get("classname")
                .is_some_and(|c| c == "info_survivor_position" || c == "info_player_start")
        })
        .map(|e| crate::kv::vector(e.get("angles")))
        .unwrap_or([0.; 3]);
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "L4D2 — native campaign viewer".into(),
                        resolution: (1280, 720).into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(MaterialPlugin::<SourceMaterial>::default())
        .add_plugins(MaterialPlugin::<crate::source_water::WaterMaterial>::default())
        .add_plugins(MaterialPlugin::<crate::source_sun::SunMaterial>::default())
        .add_systems(PostUpdate, crate::source_sun::update.before(bevy::transform::TransformSystems::Propagate))
        .add_plugins(AutoExposurePlugin)
        .add_systems(Startup, crate::native_tonemap::install_linear_lut)
        .add_plugins(crate::gpu_writes::GpuWritesPlugin)
        .init_resource::<LightingTable>()
        .insert_resource(Map(scene))
        .insert_resource(Fly {
            yaw: angles[1].to_radians() - std::f32::consts::FRAC_PI_2,
            pitch: -angles[0].to_radians(),
        })
        .insert_resource(GlobalAmbientLight {
            brightness: 80.,
            affects_lightmapped_meshes: false,
            ..default()
        })
        .insert_resource(ClearColor(Color::srgb(0.13, 0.17, 0.22)))
        .add_systems(Startup, setup)
        .add_systems(Update, fly)
        .run();
}
fn take_lightmap_bytes(atlas: &mut crate::bsp::LightAtlas) -> Vec<u8> {
    let pixels = std::mem::take(&mut atlas.pixels);
    let freed = pixels.len() * std::mem::size_of::<[f32; 4]>();
    let bytes = pixels.into_iter()
        .flat_map(|p| p.into_iter().flat_map(|c| half::f16::from_f32(c).to_le_bytes()))
        .collect();
    println!("Lightmap CPU atlas released: {} MiB; dimensions {}x{} retained", freed / (1024 * 1024), atlas.width, atlas.height);
    bytes
}

// Instance lighting and indexing create private copies. Once a batch has
// copied their attributes, do not retain them until Bevy's next asset sweep:
// scene construction runs in one system, so that sweep cannot relieve its peak.
fn release_temporary_mesh(meshes: &mut Assets<Mesh>, instance: &Handle<Mesh>, shared: &Handle<Mesh>) -> usize {
    if instance.id() == shared.id() {
        return 0;
    }
    meshes.remove(instance.id()).map_or(0, |mesh| {
        mesh.attributes().map(|(_, values)| values.get_bytes().len()).sum::<usize>()
    })
}

fn texture_image(texture: &crate::vtf::Texture, rgba: Vec<u8>, srgb: bool) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: texture.width,
            height: texture.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        // GPU-only: Bevy moves the texels to the render world instead of
        // keeping a CPU copy beside the upload (about 1.6 GB on c1m1).
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: if texture.clamp[0] {
            ImageAddressMode::ClampToEdge
        } else {
            ImageAddressMode::Repeat
        },
        address_mode_v: if texture.clamp[1] {
            ImageAddressMode::ClampToEdge
        } else {
            ImageAddressMode::Repeat
        },
        anisotropy_clamp: 8,
        lod_max_clamp: if texture.no_mip { 0. } else { 32. },
        ..ImageSamplerDescriptor::linear()
    });
    image.texture_descriptor.mip_level_count = texture.mip_count();
    match texture.blocks.as_ref().filter(|_| BLOCK_TEXTURES.load(std::sync::atomic::Ordering::Relaxed)) {
        Some(blocks) => {
            image.texture_descriptor.format = match (blocks.format, srgb) {
                (14, false) => TextureFormat::Bc2RgbaUnorm,
                (14, true) => TextureFormat::Bc2RgbaUnormSrgb,
                (15, false) => TextureFormat::Bc3RgbaUnorm,
                (15, true) => TextureFormat::Bc3RgbaUnormSrgb,
                (_, false) => TextureFormat::Bc1RgbaUnorm,
                (_, true) => TextureFormat::Bc1RgbaUnormSrgb,
            };
            image.data = Some(blocks.data.clone());
        }
        None => image.data = Some(texture.rgba_mips()),
    }
    skip_top_mips(&mut image, crate::graphics_options::texture_skip());
    image
}
/// Drops up to `skip` sharpest mips (texture detail below High). Keeps at
/// least one level, and stops where a block-compressed level would no
/// longer be a whole number of 4x4 blocks.
fn skip_top_mips(image: &mut Image, skip: u32) {
    let levels = image.texture_descriptor.mip_level_count;
    let (width, height) = (image.texture_descriptor.size.width, image.texture_descriptor.size.height);
    let format = image.texture_descriptor.format;
    let block = match format {
        TextureFormat::Bc1RgbaUnorm | TextureFormat::Bc1RgbaUnormSrgb => Some(8),
        TextureFormat::Bc2RgbaUnorm | TextureFormat::Bc2RgbaUnormSrgb
        | TextureFormat::Bc3RgbaUnorm | TextureFormat::Bc3RgbaUnormSrgb => Some(16),
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => None,
        _ => return,
    };
    let size = |level: u32| {
        let (w, h) = ((width >> level).max(1) as usize, (height >> level).max(1) as usize);
        match block {
            Some(b) => w.div_ceil(4) * h.div_ceil(4) * b,
            None => w * h * 4,
        }
    };
    let mut n = 0;
    while n < skip && n + 1 < levels {
        let (w, h) = ((width >> (n + 1)).max(1), (height >> (n + 1)).max(1));
        if block.is_some() && (w % 4 != 0 || h % 4 != 0) {
            break;
        }
        n += 1;
    }
    let Some(data) = image.data.as_mut().filter(|_| n > 0) else { return };
    let offset: usize = (0..n).map(size).sum();
    let expected: usize = (0..levels).map(size).sum();
    if data.len() != expected {
        return;
    }
    data.drain(..offset);
    image.texture_descriptor.size.width = (width >> n).max(1);
    image.texture_descriptor.size.height = (height >> n).max(1);
    image.texture_descriptor.mip_level_count = levels - n;
}
/// The GPU samples BC1-3 directly (TEXTURE_COMPRESSION_BC), so DXT textures
/// are uploaded as stored instead of as RGBA8.
static BLOCK_TEXTURES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn source_transform(origin: [f32; 3], angles: [f32; 3]) -> Transform {
    Transform {
        translation: Vec3::from_array(binary::source_position(origin)),
        rotation: Quat::from_rotation_y(angles[1].to_radians())
            * Quat::from_rotation_z(-angles[0].to_radians())
            * Quat::from_rotation_x(angles[2].to_radians()),
        ..default()
    }
}
fn sky_transform(scene: &Scene, transform: Transform, is_sky: bool) -> Transform {
    if !is_sky {
        return transform;
    }
    let Some(sky) = &scene.world.sky else {
        return transform;
    };
    let rotation = source_transform([0.; 3], sky.angles).rotation.inverse();
    Transform {
        translation: rotation
            * (transform.translation - Vec3::from_array(binary::source_position(sky.origin)))
            * sky.scale,
        rotation: rotation * transform.rotation,
        scale: transform.scale * sky.scale,
    }
}
fn sky_image(scene: &Scene) -> Option<Image> {
    let paths = scene.sky_textures.as_ref()?;
    let size = 512usize;
    let mut rgba = Vec::with_capacity(size * size * 8 * 6);
    let name = scene.world.entities.iter().find_map(|e| e.get("skyname"))?;
    for (i, path) in paths.iter().enumerate() {
        let material =
            &scene.materials[&format!("skybox/{name}{}", ["rt", "lf", "up", "dn", "bk", "ft"][i])];
        let texture = &scene.textures[path];
        for y in 0..size {
            for x in 0..size {
                // Source's "up" face is a quarter turn from the cubemap's +Y
                // layout. Unrotated, its edges did not meet the horizon faces
                // and the whole top of the sky showed as a hard square.
                let (fx, fy) = if i == 2 { (size - 1 - y, x) } else { (x, y) };
                let uv = material::transform_uv(
                    [
                        (fx as f32 + 0.5) / size as f32,
                        (fy as f32 + 0.5) / size as f32,
                    ],
                    material.uv,
                );
                let sx =
                    (uv[0] * texture.width as f32).clamp(0., texture.width as f32 - 1.) as usize;
                let sy =
                    (uv[1] * texture.height as f32).clamp(0., texture.height as f32 - 1.) as usize;
                let pixel = &texture.rgba[(sy * texture.width as usize + sx) * 4..][..4];
                for (k, channel) in pixel[..3].iter().enumerate() {
                    let c = *channel as f32 / 255.;
                    let value = if material.hdr_compressed {
                        c * pixel[3] as f32 / 255. * 8.
                    } else if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    };
                    // The Sky shaders multiply by `$color` (c7m1's river_hdr
                    // is .16: unscaled, its sky was ~6x retail's and held the
                    // exposure at 2 where retail reaches the map's 9).
                    let value = value * material.color[k];
                    rgba.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
                }
                rgba.extend_from_slice(&half::f16::from_f32(1.).to_le_bytes());
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    image.texture_view_descriptor = Some(bevy::render::render_resource::TextureViewDescriptor {
        dimension: Some(bevy::render::render_resource::TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    Some(image)
}
pub(crate) fn mesh(
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uv: Vec<[f32; 2]>,
    transform: [f32; 7],
) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        uv.into_iter()
            .map(|uv| material::transform_uv(uv, transform))
            .collect::<Vec<_>>(),
    );
    if let Some(normals) = normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_flat_normals();
    }
    // Degenerate UVs occur on non-textured surfaces and need no normal-map tangents.
    let _ = mesh.generate_tangents();
    mesh
}
fn map_entity_visibility(
    id: usize,
    scene: &Scene,
    logic: Option<&crate::l4d2r_cinematic::MapLogic>,
) -> Visibility {
    if scene.world.entities.get(id).is_some_and(|e| {
        e.get("rendermode").is_some_and(|v| v == "10")
            || (e.get("renderamt").is_some_and(|v| v == "0")
                && e.get("rendermode").is_some_and(|v| v != "0"))
    }) {
        return Visibility::Hidden;
    }
    let visible = if let Some(logic) = logic {
        logic.world.ents.get(id).is_some_and(|e| {
            e.alive
                && !matches!(
                    e.kind,
                    l4d2r_game_core::entities::Kind::Brush { enabled: false, .. }
                )
        })
    } else {
        scene
            .world
            .entities
            .get(id)
            .is_some_and(|e| e.get("startdisabled").is_none_or(|s| s != "1"))
    };
    if visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}
fn setup(
    mut commands: Commands,
    mut map: ResMut<Map>,
    fly: Res<Fly>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SourceMaterial>>,
    mut water_materials: ResMut<Assets<crate::source_water::WaterMaterial>>,
    (mut sun_materials, mut standard_materials): (
        ResMut<Assets<crate::source_sun::SunMaterial>>,
        ResMut<Assets<StandardMaterial>>,
    ),
    mut images: ResMut<Assets<Image>>,
    mut exposure_curves: ResMut<Assets<AutoExposureCompensationCurve>>,
    capture: Option<Res<Capture>>,
    game: Option<Res<GameRender>>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    lighting_table: Res<LightingTable>,
    mut pending: Option<ResMut<crate::graphics_options::PendingTextures>>,
    device: Option<Res<bevy::render::renderer::RenderDevice>>,
    exposure_readback: Option<Res<crate::native_tonemap::ExposureReadback>>,
) {
    // `--profile-mobile-memory` stands in for a phone, whose GPU lacks BC.
    BLOCK_TEXTURES.store(
        !crate::performance::mobile_memory_profile()
            && device.is_some_and(|d| d.features().contains(bevy::render::render_resource::WgpuFeatures::TEXTURE_COMPRESSION_BC)),
        std::sync::atomic::Ordering::Relaxed,
    );
    // The sky cube samples the decoded skybox faces, so build it before the
    // texels are moved into Bevy images below.
    let sky = sky_image(&map.0);
    // DecalModulate overlays (c4m3's saferoom graffiti): the framebuffer
    // times 2 x lerp(0.5, texel, alpha), as bullet_impacts' mod2x decals;
    // drawn as SourceMaterial they were opaque white sheets.
    let modulate: HashMap<String, Handle<StandardMaterial>> = map
        .0
        .materials
        .iter()
        .filter(|(_, m)| m.shader == "decalmodulate")
        .filter_map(|(name, m)| {
            let t = map.0.textures.get(m.base.as_ref()?)?;
            let mut rgba = t.rgba.clone();
            for px in rgba.chunks_exact_mut(4) {
                let a = px[3] as f32 / 255.;
                for c in &mut px[..3] {
                    let v = 0.5 + (*c as f32 / 255. - 0.5) * a;
                    *c = (v * 2. * 255.).round().min(255.) as u8;
                }
                px[3] = 255;
            }
            let image = images.add(Image::new(
                bevy::render::render_resource::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                bevy::render::render_resource::TextureDimension::D2,
                rgba,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            ));
            Some((name.clone(), standard_materials.add(StandardMaterial {
                base_color_texture: Some(image),
                unlit: true,
                alpha_mode: AlphaMode::Multiply,
                cull_mode: None,
                fog_enabled: false,
                depth_bias: 1.,
                ..default()
            })))
        })
        .collect();
    // env_cubemap captures (origin in Source units, material name, cube).
    let cubemaps: Vec<(Vec3, String, Handle<Image>)> = map
        .0
        .cubemaps
        .iter()
        .map(|c| (Vec3::from_array(c.origin), c.name.clone(), images.add(cubemap_image(&c.map))))
        .collect();
    {
        let wanted: Vec<_> = map.0.materials.values().filter_map(|m| m.envmap.as_ref()).collect();
        let named = wanted.iter().filter(|e| cubemaps.iter().any(|(_, n, _)| *n == e.name)).count();
        let nearest = wanted.iter().filter(|e| e.name == "env_cubemap").count();
        println!("Envmaps: {} captures (origins {}), {} materials ({} named and found, {} nearest-capture)",
            cubemaps.len(), map.0.world.cubemaps.len(), wanted.len(), named, nearest);
    }
    let mut glass_masks=crate::source_break_surfaces::GlassMasks::default();
    for (prefix,target) in [("glass",&mut glass_masks.glass),("tile",&mut glass_masks.tile)] {
        if let Some(texture)=crate::source_break_surfaces::mask_atlas(&map.0.textures,prefix) {
            *target=Some(images.add(texture_image(&texture,texture.rgba.clone(),false)));
        }
    }
    if std::env::args().any(|a|a=="--capture-game") {
        println!("Glass mask upload: glass={} tile={}",glass_masks.glass.is_some(),glass_masks.tile.is_some());
    }
    commands.insert_resource(glass_masks);
    // Build compact alpha-only foliage visibility before CPU texels/models
    // are released. Geometry is shared across repeated static instances.
    commands.insert_resource(crate::source_sun::SunOcclusion::prepare(&map.0));
    let textures = std::mem::take(&mut map.0.textures);
    // Runtime uses the half-float image and separate styled-face tiles.
    let lightmap_bytes = take_lightmap_bytes(&mut map.0.world.lightmap);
    crate::performance::memory_checkpoint("Lightmap CPU atlas released");
    // Full-resolution base texture sizes (Source's material mapping size).
    let mapping_sizes: HashMap<String, [f32; 2]> = textures
        .iter()
        .map(|(name, t)| (name.clone(), [t.full[0].max(1) as f32, t.full[1].max(1) as f32]))
        .collect();
    let scene = &map.0;
    let shadows = crate::lighting::WorldShadow::new(&scene.world.positions);
    crate::performance::memory_checkpoint("World lighting hierarchy ready");
    let mut diffuse = HashMap::new();
    let mut normal = HashMap::new();
    // Each texture becomes an sRGB image only if a material uses it as a base
    // (self-illumination reuses the base), and a linear image only if it is a
    // bump map or self-illumination mask. Texels are moved, not cloned, so a
    // map's textures are never held twice in RAM.
    let used = |pick: &dyn Fn(&crate::material::Material) -> bool| {
        scene.materials.values().any(pick)
    };
    let palette_heights: HashMap<_, _> = scene.materials.values().filter_map(|m| {
        let (path, _, _) = m.palette.as_ref()?;
        Some((path.clone(), textures.get(path)?.height.max(1) as f32))
    }).collect();
    let mut add = |image: Image| match pending.as_mut() {
        Some(pending) => {
            let handle = images.reserve_handle();
            pending.0.push_back((handle.clone(), image));
            handle
        }
        None => images.add(image),
    };
    for (name, mut texture) in textures {
        let base = used(&|m| (m.shader != "infected" && m.base.as_ref() == Some(&name)) || m.base2.as_ref() == Some(&name) || m.infected_burn.as_ref() == Some(&name)
            || m.palette.as_ref().is_some_and(|(p, _, _)| p == &name));
        let linear = used(&|m| {
            m.normal.as_ref() == Some(&name) || m.bump.as_ref() == Some(&name) || m.emission_mask.as_ref() == Some(&name)
                || m.cap_phong.as_ref().is_some_and(|c| c.exponent_texture.as_ref() == Some(&name) || c.lightwarp.as_ref() == Some(&name))
                || (m.shader == "infected" && m.base.as_ref() == Some(&name))
                || m.infected_detail.as_ref() == Some(&name)
                || m.wound.as_ref().is_some_and(|w| w.texture == name)
                || m.water.as_ref().is_some_and(|w| w.textures().any(|p| p == &name))
                || m.blend_modulate.as_ref() == Some(&name)
        });
        if linear {
            let rgba = if base {
                texture.rgba.clone()
            } else {
                std::mem::take(&mut texture.rgba)
            };
            normal.insert(name.clone(), add(texture_image(&texture, rgba, false)));
        }
        if base {
            let rgba = std::mem::take(&mut texture.rgba);
            diffuse.insert(name, add(texture_image(&texture, rgba, true)));
        }
    }
    let sky_handle = sky.map(&mut add);
    crate::source_sun::spawn(&mut commands, &mut meshes, &mut sun_materials, scene, &diffuse);
    let mut water_handles = HashMap::new();
    for (name, m) in &scene.materials {
        if let Some(w) = &m.water {
            for sky_surface in [false, true] {
                let handle = water_materials.add(crate::source_water::WaterMaterial {
                    base: StandardMaterial {
                        unlit: true,
                        specular_transmission: if w.refract && !w.force_cheap {1.} else {0.},
                        cull_mode: if m.nocull {None} else {Some(bevy::render::render_resource::Face::Back)},
                        double_sided: m.nocull,
                        ..default()
                    },
                    extension: crate::source_water::extension(w, &normal, sky_handle.clone(), lighting_table.buffer.clone(), sky_surface),
                });
                water_handles.insert((name.clone(), sky_surface), handle);
            }
        }
    }
    let mut material_handles = HashMap::new();
    let envmap_for = |m: &crate::material::Material| -> (Option<Handle<Image>>, Vec4, Vec4) {
        let Some(env) = &m.envmap else { return (None, Vec4::ZERO, Vec4::ZERO) };
        let image = cubemaps.iter().find(|(_, name, _)| *name == env.name).map(|(_, _, h)| h.clone());
        envmap_values(env, image)
    };
    for (name, m) in &scene.materials {
        let (envmap, envmap_tint, envmap_params) = envmap_for(m);
        let handle = materials.add(source_material(
            name,
            m,
            &|p: &str| diffuse.get(p).cloned(),
            &|p: &str| normal.get(p).cloned(),
            (envmap, envmap_tint, envmap_params),
            &palette_heights,
            lighting_table.buffer.clone(),
        ));
        material_handles.insert(name.clone(), handle);
    }
    // Keep 3D-sky fragments behind all playable geometry, as Source's
    // separate sky pass does. Scaling sky meshes alone can put them in front
    // of nearby alpha-tested fences and windows.
    let sky_material_handles: HashMap<_, _> = material_handles
        .iter()
        .map(|(name, handle)| {
            let mut material = materials.get(handle).unwrap().clone();
            material.extension.settings.w = 1.;
            (name.clone(), materials.add(material))
        })
        .collect();
    let atlas = &scene.world.lightmap;
    let mut light = Image::new(
        Extent3d {
            width: atlas.width as u32,
            height: atlas.height as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        lightmap_bytes,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    light.sampler = ImageSampler::linear();
    let light = images.add(light);
    // Switchable light styles rewrite styled texels in place (keeps CPU data).
    commands.insert_resource(crate::light_styles::Lightmap(light.clone()));
    for handle in water_handles.values() {
        if let Some(mut water) = water_materials.get_mut(handle) {
            water.extension.lightmap = Some(light.clone());
        }
    }
    let legacy_lightmap = std::env::args().any(|a| a == "--profile-legacy-lightmap");
    let mut lightmapped = HashMap::new();
    let mut replacement_materials=material_handles.clone();
    if let Some(logic)=logic.as_deref() {
        for path in logic.break_surfaces.values().filter_map(|s|s.crack_material.as_ref()) {
            if let Some(base)=material_handles.get(path) {
                let material=world_material(&mut lightmapped,&mut materials,base,&light,legacy_lightmap);
                replacement_materials.insert(path.clone(),material);
            }
        }
    }
    commands.insert_resource(SourceMaterials(replacement_materials));
    for surface in &scene.world.surfaces {
        let mut mesh = mesh(
            surface.positions.clone(),
            None,
            world_uv(surface, scene.materials.get(&surface.material).and_then(|m| m.base.as_ref()).and_then(|b| mapping_sizes.get(b))),
            scene.materials[&surface.material].uv,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, surface.light_uv.clone());
        // Every world mesh carries the stride: a bumped material's pipeline
        // reads it (0 where a face has no bump lightmaps, e.g. overlays).
        mesh.insert_attribute(
            SOURCE_BUMP,
            if surface.bump.len() == surface.positions.len() { surface.bump.clone() } else { vec![0.; surface.positions.len()] },
        );
        if scene.materials.get(&surface.material).is_some_and(|m| m.base2.is_some()) {
            mesh.insert_attribute(SOURCE_BLEND, surface.blend.clone());
        }
        if !std::env::args().any(|a| a == "--profile-legacy-static-geometry") {
            crate::static_geometry::index(&mut mesh);
        }
        let mesh = meshes.add(mesh);
        if surface.model == 0 {
            if let Some(water) = water_handles.get(&(surface.material.clone(), surface.sky)) {
                commands.spawn((
                    Name::new(format!("water/{}", surface.material)),
                    crate::visibility::Clusters(surface.clusters.clone()),
                    crate::source_water::WaterSurface { sky: surface.sky },
                    Mesh3d(mesh), MeshMaterial3d(water.clone()),
                    sky_transform(scene, Transform::default(), surface.sky),
                ));
                continue;
            }
            if let Some(handle) = modulate.get(&surface.material) {
                commands.spawn((
                    Name::new(format!("modulate/{}", surface.material)),
                    crate::visibility::Clusters(surface.clusters.clone()),
                    Mesh3d(mesh),
                    MeshMaterial3d(handle.clone()),
                    sky_transform(scene, Transform::default(), surface.sky),
                ));
                continue;
            }
            commands
                .spawn((
                    Name::new(format!("world/{}", surface.material)),
                    crate::visibility::Clusters(surface.clusters.clone()),
                    RenderLayers::layer(
                        if crate::special_infected::infected_only_material(&surface.material)
                            || scene.materials.get(&surface.material).is_some_and(|m| m.player_team == Some(3))
                        {
                            crate::special_infected::MARKER_LAYER
                        } else {
                            0
                        },
                    ),
                    Mesh3d(mesh),
                    MeshMaterial3d(world_material(
                        &mut lightmapped,
                        &mut materials,
                        if surface.sky {
                            &sky_material_handles[&surface.material]
                        } else {
                            &material_handles[&surface.material]
                        },
                        &light,
                        legacy_lightmap,
                    )),
                    sky_transform(scene, Transform::default(), surface.sky),
                ))
                .insert_if(
                    Lightmap {
                        image: light.clone(),
                        ..default()
                    },
                    || legacy_lightmap,
                );
        } else {
            for brush in scene.world.brushes.iter().filter(|b| {
                b.model == surface.model
                    && !b.classname.starts_with("trigger_")
                    && !matches!(
                        b.classname.as_str(),
                        "func_nav_blocker"
                            | "func_areaportal"
                            | "func_areaportalwindow"
                            | "func_occluder"
                            | "func_ladder"
                    )
            }) {
                if let Some(water) = water_handles.get(&(surface.material.clone(), brush.sky)) {
                    let mut draw = commands.spawn((
                        Name::new(brush.classname.clone()),
                        crate::source_water::WaterSurface { sky: brush.sky },
                        Mesh3d(mesh.clone()), MeshMaterial3d(water.clone()),
                        sky_transform(scene, source_transform(brush.origin, brush.angles), brush.sky),
                    ));
                    if game.is_some() {
                        draw.insert((crate::l4d2r_cinematic::MapEntity(brush.entity_index), map_entity_visibility(brush.entity_index, scene, logic.as_deref())));
                    }
                    continue;
                }
                let mut draw_entity = commands.spawn((
                    Name::new(brush.classname.clone()),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(world_material(
                        &mut lightmapped,
                        &mut materials,
                        if brush.sky {
                            &sky_material_handles[&surface.material]
                        } else {
                            &material_handles[&surface.material]
                        },
                        &light,
                        legacy_lightmap,
                    )),
                    sky_transform(
                        scene,
                        source_transform(brush.origin, brush.angles),
                        brush.sky,
                    ),
                ));
                if legacy_lightmap {
                    draw_entity.insert(Lightmap {
                        image: light.clone(),
                        ..default()
                    });
                }
                if crate::special_infected::infected_only_brush(&brush.classname)
                    || crate::special_infected::infected_only_material(&surface.material)
                    || scene.materials.get(&surface.material).is_some_and(|m| m.player_team == Some(3))
                {
                    draw_entity.insert(RenderLayers::layer(crate::special_infected::MARKER_LAYER));
                }
                if game.is_some() {
                    draw_entity.insert((
                        crate::l4d2r_cinematic::MapEntity(brush.entity_index),
                        map_entity_visibility(brush.entity_index, scene, logic.as_deref()),
                    ));
                }
            }
        }
    }
    crate::performance::memory_checkpoint("World surface meshes ready");
    let mut model_handles: HashMap<ModelKey, Vec<(Handle<Mesh>, String)>> = HashMap::new();
    // Phones: model templates stay CPU-only until something draws them
    // (render_warmup::upload_drawn_meshes). Most are only ever copied (lit
    // props, pickups, gore), and CPU and GPU share one memory pool there.
    let template_usage = if crate::performance::mobile_memory_profile() {
        RenderAssetUsages::MAIN_WORLD
    } else {
        RenderAssetUsages::default()
    };
    for (key, draws) in &scene.models {
        let handles = draws
            .iter()
            .map(|d| {
                let mut template = mesh(
                    d.positions.clone(),
                    Some(d.normals.clone()),
                    d.uv.clone(),
                    scene.materials[&d.material].uv,
                );
                template.asset_usage = template_usage;
                (meshes.add(template), d.material.clone())
            })
            .collect();
        model_handles.insert(key.clone(), handles);
    }
    crate::performance::memory_checkpoint("Shared prop meshes ready");
    let mut tinted: HashMap<(String, [u8; 4]), Handle<SourceMaterial>> = HashMap::new();
    // "env_cubemap" on models: the capture nearest the prop (Source binds
    // the closest env_cubemap to each static/dynamic prop).
    let mut env_variants: HashMap<(AssetId<SourceMaterial>, usize), Handle<SourceMaterial>> = HashMap::new();
    // Shared prop meshes every placement copied (lit) or packed: drawn by
    // nothing, so removed after placement (CPU and GPU memory).
    let mut shared_seen = std::collections::HashSet::<AssetId<Mesh>>::new();
    let mut shared_needed = std::collections::HashSet::<AssetId<Mesh>>::new();
    let pack_static = !std::env::args().any(|a| a == "--profile-legacy-static-geometry");
    let legacy_lighting =
        std::env::args().any(|a| a == "--profile-legacy-lighting" || a == "--shared-lighting");
    let mut static_batches = HashMap::<
        (AssetId<SourceMaterial>, Vec<u16>, [i32; 3]),
        (Mesh, Handle<SourceMaterial>, usize),
    >::new();
    let mut static_draws = 0usize;
    let mut released_mesh_bytes = 0usize;
    for (instance, p) in scene.world.props.iter().enumerate() {
        if instance != 0 && instance % 512 == 0 {
            crate::performance::memory_checkpoint(&format!("Prop meshes {instance}/{}", scene.world.props.len()));
        }
        let key = (p.model.clone(), p.skin, p.body);
        let Some(draws) = model_handles.get(&key) else {
            continue;
        };
        let transform = source_transform(p.origin, p.angles);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for d in &scene.models[&key] {
            for position in &d.positions {
                let v = Vec3::from_array(*position);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        let light_origin = p
            .lighting_origin
            .map(|o| Vec3::from_array(binary::source_position(o)))
            .unwrap_or_else(|| transform.transform_point((lo + hi) * 0.5) + Vec3::Y * 0.04);
        let ambient = model_ambient(&scene.world, &shadows, light_origin);
        let dynamic =
            game.is_some() && (p.static_index.is_none() || p.lighting_flags & 0x40 != 0) && !p.sky;
        let suns: Vec<_> = scene
            .world
            .suns
            .iter()
            .filter(|sun| {
                !dynamic
                    && !scene.prop_lighting.contains_key(&instance)
                    && !shadows.blocked(light_origin.to_array(), sun.direction)
            })
            .collect();
        let dynamic_sun = if dynamic {
            scene.world.suns.first().filter(|sun| {
                !shadows.blocked(
                    light_origin.to_array(),
                    Vec3::from_array(sun.direction)
                        .normalize_or_zero()
                        .to_array(),
                )
            })
        } else {
            None
        };
        for (draw_index, (mesh, name)) in draws.iter().enumerate() {
            let mut lit_mesh = if scene.materials[name].unlit || dynamic {
                mesh.clone()
            } else {
                let mut lit_mesh = meshes.get(mesh).unwrap().clone();
                let colors = if let Some(colors) = scene.prop_lighting.get(&instance) {
                    colors[draw_index].clone()
                } else {
                    let rotation = source_transform([0.; 3], p.angles).rotation;
                    scene.models[&key][draw_index]
                        .normals
                        .iter()
                        .map(|n| {
                            let n = rotation * Vec3::from_array(*n);
                            let mut rgb = crate::lighting::ambient(&ambient, [n.x, -n.z, n.y]);
                            for sun in &suns {
                                let cosine = n
                                    .normalize_or_zero()
                                    .dot(Vec3::from_array(sun.direction).normalize_or_zero())
                                    .max(0.);
                                for i in 0..3 {
                                    rgb[i] += sun.intensity[i] * cosine;
                                }
                            }
                            rgb
                        })
                        .collect()
                };
                lit_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                meshes.add(lit_mesh)
            };
            let mat = if p.tint == [255; 4] {
                material_handles[name].clone()
            } else {
                tinted
                    .entry((name.clone(), p.tint))
                    .or_insert_with(|| {
                        let mut m = materials.get(&material_handles[name]).unwrap().clone();
                        let c = scene.materials[name].color;
                        m.base.base_color = Color::linear_rgba(
                            c[0] * p.tint[0] as f32 / 255.,
                            c[1] * p.tint[1] as f32 / 255.,
                            c[2] * p.tint[2] as f32 / 255.,
                            c[3] * p.tint[3] as f32 / 255.,
                        );
                        materials.add(m)
                    })
                    .clone()
            };
            let mat = match scene.materials[name].envmap.as_ref().filter(|e| e.name == "env_cubemap") {
                Some(env) => {
                    let origin = Vec3::from_array(p.origin);
                    let nearest = cubemaps
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1 .0.distance_squared(origin).total_cmp(&b.1 .0.distance_squared(origin)))
                        .map(|(i, _)| i);
                    match nearest {
                        Some(i) => env_variants
                            .entry((mat.id(), i))
                            .or_insert_with(|| {
                                let mut m = materials.get(&mat).unwrap().clone();
                                let (envmap, tint, params) = envmap_values(env, Some(cubemaps[i].2.clone()));
                                m.extension.envmap = envmap;
                                m.extension.envmap_tint = tint;
                                m.extension.envmap_params = params;
                                materials.add(m)
                            })
                            .clone(),
                        None => mat,
                    }
                }
                None => mat,
            };
            // Table lighting computes dynamic props on their first update and
            // keeps the shared material; only the legacy path bakes a copy.
            let mat = if dynamic && legacy_lighting && !materials.get(&mat).unwrap().base.unlit {
                let mut native = materials.get(&mat).unwrap().clone();
                native.extension.settings.y = 1.;
                native.extension.ambient_cube = ambient.map(|v| Vec3::from_array(v).extend(0.));
                native.extension.sun_direction = dynamic_sun.map_or(Vec4::ZERO, |s| {
                    Vec3::from_array(s.direction).normalize_or_zero().extend(0.)
                });
                native.extension.sun_color =
                    dynamic_sun.map_or(Vec4::ZERO, |s| Vec3::from_array(s.intensity).extend(0.));
                materials.add(native)
            } else {
                mat
            };
            let dynamic_handle = mat.clone();
            let mat = if p.sky {
                let mut sky_material = materials.get(&mat).unwrap().clone();
                sky_material.extension.settings.w = 1.;
                materials.add(sky_material)
            } else {
                mat
            };
            if pack_static
                && p.static_index.is_some()
                && !p.sky
                && scene.materials[name].alpha <= 1
                && !dynamic
            {
                let source = meshes.get(&lit_mesh).unwrap();
                let cell = transform
                    .translation
                    .to_array()
                    .map(|v| (v / 32.).floor() as i32);
                let key = (mat.id(), p.clusters.clone(), cell);
                let entry = static_batches.entry(key).or_insert_with(|| {
                    (crate::static_geometry::empty_like(source), mat.clone(), 0)
                });
                if crate::static_geometry::append(&mut entry.0, source, &transform) {
                    shared_seen.insert(mesh.id());
                    entry.2 += 1;
                    static_draws += 1;
                    released_mesh_bytes += release_temporary_mesh(&mut meshes, &lit_mesh, mesh);
                    continue;
                }
            }
            if pack_static && p.static_index.is_some() && !dynamic {
                // Transparency keeps its original entity/triangle order, but
                // can still share identical vertices within each draw.
                let mut indexed = meshes.get(&lit_mesh).unwrap().clone();
                let (before, after) = crate::static_geometry::index(&mut indexed);
                if after < before {
                    released_mesh_bytes += release_temporary_mesh(&mut meshes, &lit_mesh, mesh);
                    lit_mesh = meshes.add(indexed);
                }
            }
            let draw_entity_mesh_private = lit_mesh.id() != mesh.id();
            shared_seen.insert(mesh.id());
            if !draw_entity_mesh_private || p.static_index.is_none() || dynamic {
                shared_needed.insert(mesh.id());
            }
            let mut draw_entity = commands.spawn((
                Name::new(format!("{} / {}", p.classname, p.model)),
                Mesh3d(lit_mesh),
                MeshMaterial3d(mat),
                sky_transform(scene, source_transform(p.origin, p.angles), p.sky),
                RenderLayers::layer(1),
            ));
            if p.static_index.is_some() && !p.sky {
                draw_entity.insert(crate::visibility::Clusters(p.clusters.clone()));
            }
            // A prop_static's private (lit or re-indexed) copy is read by
            // nothing after upload, like the packed batches: drop its CPU
            // copy on phones. Shared model meshes stay (pickups clone them).
            if p.static_index.is_some() && !dynamic && draw_entity_mesh_private {
                draw_entity.insert(crate::render_warmup::ReleaseCpuMesh::default());
            }
            if dynamic {
                draw_entity.insert(DynamicModelLighting {
                    appearance: None,
                    material: Some(dynamic_handle),
                    sample_position: legacy_lighting.then_some(light_origin),
                    slot: None,
                    center: Some(
                        transform.rotation.inverse()
                            * (light_origin - transform.translation - Vec3::Y * 0.04),
                    ),
                });
            }
            if game.is_some() && p.static_index.is_none() {
                if let Some(index) = p.entity_index {
                    draw_entity.insert((
                        crate::l4d2r_cinematic::MapEntity(index),
                        map_entity_visibility(index, scene, logic.as_deref()),
                    ));
                    if (p.classname.starts_with("prop_dynamic") || p.classname == "prop_health_cabinet")
                        && !p.model.ends_with("ghostanim.mdl")
                    {
                        draw_entity.insert(crate::l4d2r_cinematic::ScriptedDraw {
                            model: p.model.clone(),
                            draw: draw_index,
                            ent: index,
                        });
                    }
                }
            }
        }
    }
    println!("Temporary prop meshes released: {:.1} MiB", released_mesh_bytes as f64 / 1048576.);
    let mut unused_shared = 0usize;
    for id in shared_seen.difference(&shared_needed) {
        if meshes.remove(*id).is_some() {
            unused_shared += 1;
        }
    }
    println!("Unused shared prop meshes removed: {unused_shared}");
    if game.is_some() {
        commands.insert_resource(RenderModels(
            model_handles
                .iter()
                .filter(|(_, draws)| draws.iter().any(|(mesh, _)| !shared_seen.contains(&mesh.id()) || shared_needed.contains(&mesh.id())))
                .map(|(key, draws)| {
                    (
                        key.clone(),
                        draws
                            .iter()
                            .map(|(mesh, name)| (mesh.clone(), material_handles[name].clone()))
                            .collect(),
                    )
                })
                .collect(),
        ));
    }
    crate::performance::memory_checkpoint("Prop batch assembly ready");
    let mut packed_draws = 0usize;
    let mut original_vertices = 0usize;
    let mut packed_vertices = 0usize;
    for ((_, clusters, _), (mut mesh, material, count)) in static_batches {
        if count == 0 {
            continue;
        }
        let (before, after) = crate::static_geometry::index(&mut mesh);
        original_vertices += before;
        packed_vertices += after;
        packed_draws += 1;
        commands.spawn((
            Name::new("static/native-material-batch"),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            RenderLayers::layer(1),
            crate::visibility::Clusters(clusters),
            crate::render_warmup::ReleaseCpuMesh::default(),
        ));
    }
    if pack_static {
        println!(
            "Static geometry: {static_draws} prop draws -> {packed_draws} batches; vertices {original_vertices} -> {packed_vertices}"
        );
    }
    crate::performance::memory_checkpoint("Packed prop meshes ready");
    commands.insert_resource(shadows);
    let bounds = crate::native_tonemap::Bounds::from_map(
        logic.as_ref().map(|l| l.world.tonemap).unwrap_or_default(),
    );
    println!(
        "Map exposure: {}..{}; rate={}",
        bounds.min, bounds.max, bounds.rate
    );
    let curve = exposure_curves.add(bounds.curve());
    let mut camera = commands.spawn((
        Camera3d::default(),
        // AutoExposure updates GPU colour grading after the main pass. That
        // value is consumed by the HDR tonemapping pass, which Bevy skips
        // entirely for an LDR camera (in-shader tonemapping runs too early).
        bevy::camera::Hdr,
        Msaa::Off,
        AutoExposure {
            compensation_curve: curve,
            filter: bounds.filter(),
            metering_mask: images.add(crate::native_tonemap::metering_mask()),
            readback: exposure_readback.as_ref().map(|r| r.0.clone()),
            speed_brighten: bounds.rate,
            speed_darken: bounds.rate,
            initial_ev: bounds.initial_ev(),
            ..default()
        },
        crate::native_tonemap::SourceMetering,
        bevy::camera::Exposure { ev100: 0. },
        // Source scales by exposure and clips; it has no tone curve.
        crate::native_tonemap::WORLD_TONEMAPPING,
        Transform {
            translation: Vec3::from_array(scene.world.spawn),
            rotation: Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.),
            ..default()
        },
        // Layer model_shadows::LAYER only carries the model-shadow light to
        // this view (Bevy matches light and view layers); nothing is drawn
        // on it alone.
        RenderLayers::from_layers(&[0, 1, crate::model_shadows::LAYER]),
    ));
    // The depth prepass also feeds SpriteCard $DEPTHBLEND (soft particles),
    // which retail applies on every map: without it c1m1's inferno_wall
    // flames cut off in hard lines against the floor and debris (25 Nov).
    // Phones skip it unless the map has water, for memory and fill rate.
    let mobile = cfg!(any(target_os = "ios", target_os = "android"));
    if !water_handles.is_empty() || !mobile || std::env::args().any(|a| a == "--capture-depth-prepass") {
        camera.insert(bevy::core_pipeline::prepass::DepthPrepass);
    }
    if let Some(image) = sky_handle {
        camera.insert(bevy::light::Skybox {
            image: Some(image),
            brightness: 1.,
            rotation: Quat::IDENTITY,
        });
    }
    if let Some(fog) = crate::native_fog::from_entities(&scene.world.entities) {
        println!("Map fog: {:?}", fog.falloff);
        camera.insert(fog);
    }
    if game.is_some() {
        camera.insert(GameCamera);
    }
    camera.insert(Projection::Perspective(PerspectiveProjection {
        far: 10000.,
        fov: if game.is_some() {
            2. * (1f32 / (4. / 3.)).atan()
        } else {
            std::f32::consts::FRAC_PI_4
        },
        ..default()
    }));
    if let Some(config) = capture {
        let image = Image::new_target_texture(
            crate::game_loading::dimensions().0,
            crate::game_loading::dimensions().1,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        );
        let image = images.add(image);
        camera.insert(bevy::camera::RenderTarget::Image(image.clone().into()));
        let origin = config
            .origin
            .map(binary::source_position)
            .unwrap_or(scene.world.spawn);
        let angles = config.angles.unwrap_or([0., 0., 0.]);
        camera.insert(Transform {
            translation: Vec3::from_array(origin),
            rotation: Quat::from_euler(
                EulerRot::YXZ,
                angles[1].to_radians() - std::f32::consts::FRAC_PI_2,
                -angles[0].to_radians(),
                0.,
            ),
            ..default()
        });
        commands.insert_resource(CaptureImage {
            image,
            frames: 0,
            requested: false,
        });
    }
    if game.is_none() {
        commands.spawn((
        Text::new(format!(
            "{}\nClick: mouse aim • Esc: release • WASD: fly • Space/Ctrl: vertical • Shift: fast",
            scene.summary()
        )),
        TextFont {
            font_size: FontSize::Px(16.),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            left: px(20.),
            top: px(20.),
            ..default()
        },
    ));
    }
    // The decoded textures and model meshes now live in Bevy assets; later
    // systems only read `map.0.world`. Keeping the scene copies held another
    // ~1.6 GB of texels plus every model's vertices for the whole map.
    let scene = &mut map.0;
    scene.textures = Default::default();
    scene.models = Default::default();
}
fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mouse: Res<AccumulatedMouseMotion>,
    mut cursor: Single<&mut CursorOptions>,
    window: Single<&Window>,
    mut state: ResMut<Fly>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
) {
    if keys.just_pressed(KeyCode::Escape) || !window.focused {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    if buttons.just_pressed(MouseButton::Left) && window.focused {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if cursor.grab_mode != CursorGrabMode::Locked {
        return;
    }
    state.yaw -= mouse.delta.x * 0.002;
    state.pitch = (state.pitch - mouse.delta.y * 0.002).clamp(-1.5, 1.5);
    camera.rotation = Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.);
    let mut direction = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        direction += *camera.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        direction -= *camera.forward();
    }
    if keys.pressed(KeyCode::KeyD) {
        direction += *camera.right();
    }
    if keys.pressed(KeyCode::KeyA) {
        direction -= *camera.right();
    }
    if keys.pressed(KeyCode::Space) {
        direction += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        direction -= Vec3::Y;
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) {
        35.
    } else {
        8.
    };
    camera.translation += direction.normalize_or_zero() * speed * time.delta_secs().min(0.05);
}

pub(crate) fn cull_pvs(
    map: Option<Res<Map>>,
    fog_state: Option<Res<crate::native_fog::Runtime>>,
    camera: Query<&Transform, With<GameCamera>>,
    materials: Res<Assets<SourceMaterial>>,
    mut draws: Query<(
        &crate::visibility::Clusters,
        &mut Visibility,
        Option<Ref<bevy::camera::primitives::Aabb>>,
        Ref<GlobalTransform>,
        Option<&MeshMaterial3d<SourceMaterial>>,
        Option<&crate::source_water::WaterSurface>,
    ), Or<(With<MeshMaterial3d<SourceMaterial>>, With<crate::source_water::WaterSurface>)>>,
    mut previous: Local<Option<(Vec<Option<usize>>, Vec3, Vec3, f32)>>,
) {
    // No scene while a phone map change is between release and apply.
    let Some(map) = map else { return };
    let Ok(camera) = camera.single() else {
        return;
    };
    let cluster = if std::env::args().any(|a| a == "--no-pvs") {
        None
    } else {
        map.0.world.visibility.cluster_at(camera.translation)
    };
    let views = if std::env::args().any(|a| a == "--no-pvs" || a == "--legacy-pvs-fallback") {
        vec![cluster]
    } else {
        map.0.world.visibility.camera_clusters(camera.translation)
    };
    let far = if std::env::args().any(|a| a == "--profile-legacy-far-culling") {
        f32::INFINITY
    } else {
        fog_state.as_ref().and_then(|s| s.far()).map(|far| if far > 0. {far} else {f32::INFINITY})
            .unwrap_or_else(|| crate::native_fog::far_distance(&map.0.world.entities).unwrap_or(f32::INFINITY))
    };
    let direction = *camera.forward();
    let key = (views.clone(), camera.translation, direction, far);
    let view_changed = previous.as_ref() != Some(&key) || map.is_changed();
    let views_changed = previous.as_ref().is_none_or(|p| p.0 != views);
    *previous = Some(key);
    let mut visible = 0;
    let mut total = 0;
    for (clusters, mut visibility, bounds, transform, material, water) in &mut draws {
        // Recheck moving/new geometry without reevaluating thousands of
        // unchanged materials and bounds in a stationary view.
        if !view_changed
            && !transform.is_changed()
            && !bounds.as_ref().is_some_and(|bounds| bounds.is_changed())
        {
            visible += usize::from(*visibility != Visibility::Hidden);
            total += 1;
            continue;
        }
        let sky = water.map_or_else(|| material.and_then(|m| materials.get(&m.0)).is_some_and(|m| m.extension.settings.w > 0.5), |w| w.sky);
        let in_range = sky
            || !far.is_finite()
            || bounds.is_none_or(|aabb| {
                bounds_before_far(&aabb, &transform, camera.translation, direction, far)
            });
        let show = in_range
            && views
                .iter()
                .any(|c| map.0.world.visibility.visible(*c, &clusters.0));
        let next = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != next {
            *visibility = next;
        }
        visible += usize::from(show);
        total += 1;
    }
    if views_changed && std::env::args().any(|a| a == "--profile-frames") {
        println!(
            "BSP PVS: camera cluster={cluster:?}; {visible}/{total} static/world draws potentially visible"
        );
    }
}
fn bounds_before_far(
    aabb: &bevy::camera::primitives::Aabb,
    transform: &GlobalTransform,
    eye: Vec3,
    forward: Vec3,
    far: f32,
) -> bool {
    let affine = transform.affine();
    let center = affine.transform_point3(Vec3::from(aabb.center));
    let half = Vec3::from(aabb.half_extents);
    let projected = affine.matrix3.x_axis.dot(forward.into()).abs() * half.x
        + affine.matrix3.y_axis.dot(forward.into()).abs() * half.y
        + affine.matrix3.z_axis.dot(forward.into()).abs() * half.z;
    (center - eye).dot(forward) - projected <= far + 0.05
}

#[cfg(test)]
mod far_culling_tests {
    use super::*;
    #[test]
    fn far_culling_keeps_intersecting_and_transformed_bounds() {
        use bevy::camera::primitives::Aabb;
        let bounds = Aabb::from_min_max(Vec3::splat(-1.), Vec3::splat(1.));
        let transform = |z| GlobalTransform::from(Transform::from_xyz(0., 0., z));
        assert!(!bounds_before_far(
            &bounds,
            &transform(-12.),
            Vec3::ZERO,
            Vec3::NEG_Z,
            10.
        ));
        assert!(bounds_before_far(
            &bounds,
            &transform(-10.5),
            Vec3::ZERO,
            Vec3::NEG_Z,
            10.
        ));
        let stretched = GlobalTransform::from(Transform {
            translation: Vec3::new(0., 0., -12.),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::new(4., 1., 1.),
        });
        assert!(bounds_before_far(
            &bounds,
            &stretched,
            Vec3::ZERO,
            Vec3::NEG_Z,
            10.
        ));
    }
}
#[cfg(test)]
mod texture_detail_tests {
    use super::*;
    fn image(width: u32, height: u32, format: TextureFormat, levels: u32, bytes: impl Fn(u32, u32) -> usize) -> Image {
        let data: Vec<u8> = (0..levels)
            .flat_map(|l| vec![l as u8; bytes((width >> l).max(1), (height >> l).max(1))])
            .collect();
        let mut image = Image::new(Extent3d { width, height, depth_or_array_layers: 1 }, TextureDimension::D2,
            vec![0; bytes(width, height)], format, RenderAssetUsages::RENDER_WORLD);
        image.texture_descriptor.mip_level_count = levels;
        image.data = Some(data);
        image
    }
    #[test]
    fn lower_texture_detail_leaves_out_the_sharpest_mips() {
        let rgba = |w: u32, h: u32| (w * h * 4) as usize;
        // Medium: the 256x128 level is dropped and the 128x64 one is first.
        let mut t = image(256, 128, TextureFormat::Rgba8UnormSrgb, 9, rgba);
        skip_top_mips(&mut t, 1);
        assert_eq!((t.texture_descriptor.size.width, t.texture_descriptor.size.height, t.texture_descriptor.mip_level_count), (128, 64, 8));
        assert_eq!(t.data.as_ref().unwrap()[0], 1);
        // A texture without mips keeps its only level.
        let mut single = image(64, 64, TextureFormat::Rgba8UnormSrgb, 1, rgba);
        skip_top_mips(&mut single, 2);
        assert_eq!(single.texture_descriptor.size.width, 64);
        // BC1 stops before a level that is not whole 4x4 blocks (8x4 -> 4x2).
        let bc1 = |w: u32, h: u32| (w.div_ceil(4) * h.div_ceil(4) * 8) as usize;
        let mut dxt = image(16, 8, TextureFormat::Bc1RgbaUnormSrgb, 4, bc1);
        skip_top_mips(&mut dxt, 2);
        assert_eq!((dxt.texture_descriptor.size.width, dxt.texture_descriptor.size.height, dxt.texture_descriptor.mip_level_count), (8, 4, 3));
        assert_eq!(dxt.data.as_ref().unwrap().len(), bc1(8, 4) + bc1(4, 2) + bc1(2, 1));
    }
}
