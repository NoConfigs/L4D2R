// Source Water reconstruction. Sky cubemap reflection is a fallback for the
// native planar reflection target; no claim of native flow shader equivalence.
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::{view, globals, view_transmission_texture, view_transmission_sampler}
#import bevy_pbr::view_transformations::position_ndc_to_world
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif
#ifdef DISTANCE_FOG
#import bevy_pbr::mesh_view_bindings::fog
#endif
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var normal_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var flow_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var flow_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var noise_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var noise_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var sky_map: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var sky_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var<uniform> water_fog: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var<uniform> ranges: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var<uniform> reflect_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var<uniform> refract_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var<uniform> flow_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var<uniform> flow_time: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var<uniform> scroll: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(115) var<uniform> flags: vec4<f32>;
struct SourceLight { cube: array<vec4<f32>,6>, sun_direction: vec4<f32>, sun_color: vec4<f32> }
@group(#{MATERIAL_BIND_GROUP}) @binding(116) var<storage, read> lighting_table: array<SourceLight>;
@group(#{MATERIAL_BIND_GROUP}) @binding(117) var lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(118) var lightmap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(119) var<uniform> lightmap_fog: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let map_globals = lighting_table[4095u];
    let distance = length(in.world_position.xyz - view.world_position.xyz);
    let far_clip = map_globals.cube[2];
    if flags.w > 0.5 {
        if far_clip.y > 0.5 && far_clip.x > 0.0 && distance < far_clip.x + far_clip.z { discard; }
    } else if far_clip.x > 0.0 && distance > far_clip.x { discard; }

    let time = globals.time * flow_time.y;
    var uv = in.uv;
    var offset = vec2(0.0);
    var phase = fract(time / flow_params.z);
    if flow_time.w > 0.5 {
        // World coordinates are converted back to Source XY before sampling.
        let xy = in.world_position.xz * vec2(1.0, -1.0) / 0.0254;
        uv = xy / flow_params.x;
        offset = (textureSample(flow_map, flow_sampler, in.uv * flow_params.y).rg * 2.0 - 1.0) * flow_params.w;
        if flags.y > 0.5 { phase = fract(phase + textureSample(noise_map, noise_sampler, xy * flow_time.z).r); }
    }
    let phase2 = fract(phase + 0.5);
    let a = textureSample(normal_map, normal_sampler, uv + offset * phase + scroll.xy * time).xyz * 2.0 - 1.0;
    let b = textureSample(normal_map, normal_sampler, uv + offset * phase2 + scroll.zw * time).xyz * 2.0 - 1.0;
    var weight = 0.5;
    if flow_time.w > 0.5 { weight = abs(phase * 2.0 - 1.0); }
    var tangent_normal = normalize(mix(a, b, weight));
    if flags.x < 0.5 { tangent_normal = vec3(0.0, 0.0, 1.0); }
    let n = normalize(vec3(tangent_normal.x * flow_time.x, max(tangent_normal.z, 0.1), -tangent_normal.y * flow_time.x));
    let eye = normalize(view.world_position.xyz - in.world_position.xyz);
    let reflection_normal = normalize(vec3(n.x * reflect_params.w, n.y, n.z * reflect_params.w));
    // Match Bevy's visible Skybox: its cubemap is left handed and its output
    // already bypasses scene exposure. Exposing it again crushes the ripples.
    let reflection_direction = reflect(-eye, reflection_normal) * vec3(1.0, 1.0, -1.0);
    let reflection = textureSample(sky_map, sky_sampler, reflection_direction).rgb * reflect_params.rgb;
    var body = water_fog.rgb * view.exposure;
#ifdef WATER_LIGHTMAP_FOG
#ifdef VERTEX_UVS_B
    // $lightmapwaterfog: the fog volume is lit by the surface's lightmap.
    body *= textureSampleLevel(lightmap, lightmap_sampler, in.uv_b, 0.0).rgb;
#endif
#endif
    var depth_factor = 1.0;
    if flags.z > 0.5 {
        let screen_uv = (in.position.xy - view.viewport.xy) / view.viewport.zw;
        var path = max(ranges.y, 0.001);
#ifdef DEPTH_PREPASS
        let depth = prepass_depth(in.position, 0u);
        if depth > 0.0 && flags.w < 0.5 {
            let behind = position_ndc_to_world(vec3(screen_uv * vec2(2.0,-2.0) + vec2(-1.0,1.0), depth));
            path = length(behind - in.world_position.xyz);
        }
#endif
        if ranges.z < 0.5 { path = distance; }
        depth_factor = clamp((path - ranges.x) / max(ranges.y - ranges.x, 0.0001), 0.0, 1.0);
        let distort = tangent_normal.xy * refract_params.w * depth_factor;
        var sample_uv = clamp(screen_uv + distort, vec2(0.001), vec2(0.999));
#ifdef DEPTH_PREPASS
        // A distorted lookup must not drag an above-water pier/actor into the
        // water. Reverse-Z: a larger opaque depth is nearer than this surface.
        let sample_pixel = vec4(sample_uv * view.viewport.zw + view.viewport.xy, 0.0, 1.0);
        if flags.w < 0.5 && prepass_depth(sample_pixel, 0u) > in.position.z { sample_uv = screen_uv; }
#endif
        let background = textureSampleLevel(view_transmission_texture, view_transmission_sampler, sample_uv, 0.0).rgb;
        body = mix(background * refract_params.rgb, body, depth_factor * water_fog.w);
    }
    // Source's above-water Fresnel tends to full reflection at grazing angles.
    let fresnel = pow(1.0 - clamp(dot(eye,n),0.0,1.0),5.0) * clamp((depth_factor - 0.05) * 20.0,0.0,1.0);
    var color = mix(body, reflection, fresnel * ranges.w);
#ifdef DISTANCE_FOG
    if flags.w < 0.5 {
        let ramp = clamp((distance-fog.be.x)/max(fog.be.y-fog.be.x,0.0001),0.0,fog.base_color.a);
        color = mix(color,fog.base_color.rgb,ramp*ramp);
    }
#endif
    if flags.w > 0.5 && map_globals.cube[0].w > 0.5 {
        let f = map_globals.cube[0];
        let ramp = clamp((distance-f.x)/max(f.y-f.x,0.0001),0.0,f.z);
        color = mix(color,map_globals.cube[1].rgb,ramp*ramp);
    }
    return vec4(color,1.0);
}
