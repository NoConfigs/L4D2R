// Every SourceLighting uniform in one binding. Bevy's AsBindGroup combines
// #[uniform(100)] fields into one buffer in declaration order (viewer.rs);
// eleven separate uniform bindings made eleven tiny GPU buffers per material
// (~11k buffers), which inflated wgpu's per-pass resource tracking.
struct WoundUniform {
    params: array<vec4<f32>, 2>,
    atlas: array<vec4<f32>, 2>,
    blood: vec4<f32>,
    blood_range: vec4<f32>,
}
struct InfectedPhong {
    exponents: vec4<f32>,
    fresnel: vec4<f32>,
    tint: vec4<f32>,
    detail: vec4<f32>,
}
struct CapPhongUniform {
    params: vec4<f32>,
    fresnel: vec4<f32>,
    controls: vec4<f32>,
    tint: vec4<f32>,
}
struct SourceUniform {
    settings: vec4<f32>,
    ambient_cube: array<vec4<f32>, 6>,
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,
    lightmap_params: vec4<f32>,
    emission_tint: vec4<f32>,
    wound_rows: array<vec4<f32>, 12>,
    wound: WoundUniform,
    infected_lookup: vec4<f32>,
    infected_phong: InfectedPhong,
    cap_phong: CapPhongUniform,
    envmap_tint: vec4<f32>,
    envmap_params: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> source_uniform: SourceUniform;
