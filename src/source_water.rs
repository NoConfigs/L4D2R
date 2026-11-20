//! Water has its own bindings/pass: ordinary surfaces pay no water sampling cost.
//! Authored flow/fog/tints are retained; planar entity reflections remain a gap.
use bevy::{pbr::{ExtendedMaterial, MaterialExtension}, prelude::*, render::render_resource::AsBindGroup};

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(WaterKey)]
pub struct Water {
    #[texture(100)] #[sampler(101)]
    pub normal: Option<Handle<Image>>,
    #[texture(102)] #[sampler(103)]
    pub flow: Option<Handle<Image>>,
    #[texture(104)] #[sampler(105)]
    pub noise: Option<Handle<Image>>,
    #[texture(106, dimension = "cube")] #[sampler(107)]
    pub sky: Option<Handle<Image>>,
    #[uniform(108)] pub fog: Vec4,
    #[uniform(109)] pub ranges: Vec4,
    #[uniform(110)] pub reflect: Vec4,
    #[uniform(111)] pub refract: Vec4,
    #[uniform(112)] pub flow_params: Vec4,
    #[uniform(113)] pub flow_time: Vec4,
    #[uniform(114)] pub scroll: Vec4,
    #[uniform(115)] pub flags: Vec4,
    #[storage(116, read_only)]
    pub lighting_table: Handle<bevy::render::storage::ShaderBuffer>,
    /// The map's lightmap atlas (set once it exists) for `$lightmapwaterfog`.
    #[texture(117)] #[sampler(118)]
    pub lightmap: Option<Handle<Image>>,
    /// x: `$lightmapwaterfog`.
    #[uniform(119)] pub lightmap_fog: Vec4,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaterKey { sky: bool, lightmap_fog: bool }
impl From<&Water> for WaterKey {
    fn from(w: &Water) -> Self {
        Self { sky: w.flags.w > 0.5, lightmap_fog: w.lightmap_fog.x > 0.5 && w.lightmap.is_some() }
    }
}
impl MaterialExtension for Water {
    fn specialize(
        _: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if key.bind_group_data.sky {
            descriptor.vertex.shader_defs.push("SOURCE_SKY_DEPTH".into());
        }
        if key.bind_group_data.lightmap_fog {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("WATER_LIGHTMAP_FOG".into());
            }
        }
        Ok(())
    }
    fn vertex_shader() -> bevy::shader::ShaderRef { "shaders/source_vertex.wgsl".into() }
    fn fragment_shader() -> bevy::shader::ShaderRef { "shaders/source_water.wgsl".into() }
    // Sample opaque depth for water fog, without putting the water in that depth.
    fn enable_prepass() -> bool { false }
    fn enable_shadows() -> bool { false }
}
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, Water>;
#[derive(Component)]
pub struct WaterSurface { pub sky: bool }

fn linear(rgb: [f32;3]) -> Vec3 { Color::srgb(rgb[0],rgb[1],rgb[2]).to_linear().to_vec3() }
pub fn extension(
    m: &crate::material::WaterSettings,
    textures: &std::collections::HashMap<String, Handle<Image>>,
    sky: Option<Handle<Image>>,
    lighting_table: Handle<bevy::render::storage::ShaderBuffer>,
    sky_surface: bool,
) -> Water {
    let get = |name: &Option<String>| name.as_ref().and_then(|p| textures.get(p)).cloned();
    Water {
        normal: get(&m.normal), flow: get(&m.flow), noise: get(&m.noise), sky,
        fog: linear(m.fog_color).extend(if m.fog {1.} else {0.}),
        ranges: Vec4::new(m.fog_start * 0.0254, m.fog_end * 0.0254, if m.above {1.} else {0.}, if m.reflect {1.} else {0.}),
        reflect: linear(m.reflect_tint).extend(m.reflect_amount),
        refract: linear(m.refract_tint).extend(m.refract_amount),
        flow_params: Vec4::new(m.normal_scale, m.world_scale, m.interval, m.scroll_distance),
        flow_time: Vec4::new(m.strength, m.time_scale, m.noise_scale, if m.flow.is_some() {1.} else {0.}),
        scroll: Vec4::new(m.scroll[0][0],m.scroll[0][1],m.scroll[1][0],m.scroll[1][1]),
        flags: Vec4::new(if m.normal.is_some() {1.} else {0.},if m.noise.is_some() {1.} else {0.}, if m.refract && !m.force_cheap {1.} else {0.},if sky_surface {1.} else {0.}),
        lighting_table,
        lightmap: None,
        lightmap_fog: Vec4::new(if m.lightmap_fog { 1. } else { 0. }, 0., 0., 0.),
    }
}
