// Shared world particles: preserve native SpriteCard frames and blend settings.
#import bevy_pbr::{mesh_functions, forward_io::Vertex, view_transformations::{position_world_to_clip, depth_ndc_to_view_z}}
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif
#import bevy_pbr::mesh_view_bindings::{view, view_transmission_texture, view_transmission_sampler}
#ifdef DISTANCE_FOG
#import bevy_pbr::mesh_view_bindings::fog
#endif
struct ParticleVertex {
    position: vec4<f32>,
    uv: vec4<f32>,
    color: vec4<f32>,
    blend: vec4<f32>,
    second_uv: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<storage, read> particle_vertices: array<ParticleVertex>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var sprite: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var sprite_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var<uniform> settings: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var<uniform> add_self: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> dual: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> effects: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var spitter_noise: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var spitter_noise_sampler: sampler;
// SpriteCard $DEPTHBLEND: fade distance in metres ($DEPTHBLENDSCALE units), 0 off.
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var<uniform> depth_blend: f32;
// 3D-skybox copies (effects.y 3): the sky_camera fog colour, already scaled
// like the sky meshes' (native_fog::Runtime::sky_color).
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var<uniform> sky_fog: vec4<f32>;
// Refract with $basetexture: added over the refraction (particle_pull.rs).
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var refract_base: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var refract_base_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var<uniform> refract_base_on: f32;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) current_uv: vec2<f32>,
    @location(1) next_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) blend: vec4<f32>,
    @location(4) second_uv: vec4<f32>,
    @location(5) world_position: vec3<f32>,
}
@vertex
fn vertex(v: Vertex) -> Output {
    var out: Output;
    let tag = mesh_functions::get_tag(v.instance_index);
    let local = u32(v.uv.x);
    if local / 4u >= (tag >> 16u) {
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let p = particle_vertices[(tag & 0xffffu) * 4u + local];
    out.position = position_world_to_clip(p.position.xyz);
    out.world_position = p.position.xyz;
    out.current_uv = p.uv.xy;
    out.next_uv = p.uv.zw;
    out.color = vec4(pow(max(p.color.rgb, vec3(0.0)), vec3(2.2)), p.color.a) * p.blend.z;
    // Native Refract VS COLORMODULATE passes the vertex colour through.
    // SpriteCard gamma handling must not darken the refraction tint.
    if effects.x != 0.0 { out.color = p.color * p.blend.z; }
    // Debug (L4D2R_PFX_RAW, effects.z 2): vertex colour without the gamma
    // decode.
    if effects.x == 0.0 && effects.z > 1.5 { out.color = p.color * p.blend.z; }
    out.blend = p.blend;
    out.second_uv = p.second_uv;
    return out;
}
fn sprite_color(v: Output) -> vec4<f32> {
    let current = textureSample(sprite, sprite_sampler, v.current_uv);
    let next = textureSample(sprite, sprite_sampler, v.next_uv);
    let t = clamp(v.blend.x, 0.0, 1.0);
    var frame = current;
    if settings.x != 0.0 { frame = mix(current, next, t); }
    if settings.y != 0.0 {
        let brightness = vec3(0.3, 0.59, 0.11);
        frame = select(next, current, dot(current.rgb, brightness) * (1.0-t) > dot(next.rgb, brightness) * t);
    }
    if dual.x != 0.0 {
        let a = textureSample(sprite, sprite_sampler, v.second_uv.xy);
        let b = textureSample(sprite, sprite_sampler, v.second_uv.zw);
        let t2 = clamp(v.blend.y, 0.0, 1.0);
        var second = mix(a, b, t2);
        if dual.z != 0.0 {
            let brightness = vec3(0.3, 0.59, 0.11);
            second = select(b, a, dot(a.rgb, brightness)*(1.0-t2) > dot(b.rgb, brightness)*t2);
        }
        if dual.y < 0.5 { frame = (frame + second)*0.5; }
        else if dual.y < 1.5 { frame = vec4(second.rgb, frame.a); }
        else { frame = vec4(mix(frame.rgb, second.rgb, second.a), frame.a); }
    }
    if effects.x != 0.0 {
        let screen_uv = v.position.xy / vec2<f32>(textureDimensions(view_transmission_texture));
        let offset = (frame.rg * 2.0 - 1.0) * effects.x * frame.a * v.color.a;
        let background = textureSampleLevel(view_transmission_texture, view_transmission_sampler, clamp(screen_uv + offset, vec2(0.001), vec2(0.999)), 0.0);
        let tint = mix(vec3(1.0), v.color.rgb, effects.z);
        var refracted = background.rgb * tint;
        if effects.w != 0.0 {
            // Recovered from owned refract_ps20b.vcs SPITTERNOISETEXTURE
            // combo 160: world Source XY * .005, sampler 8 red channel.
            let noise_uv = v.world_position.xz * vec2(1.0, -1.0) * (0.005 / 0.0254);
            let noise = textureSample(spitter_noise, spitter_noise_sampler, noise_uv).r * 3.0;
            let mean = dot(refracted, vec3(1.0 / 3.0));
            let contrast = 4.0 - 3.0 * clamp(mean * 50.0, 0.0, 1.0);
            let lift = 0.0125 * (1.0 - clamp(mean * 5.0, 0.0, 1.0));
            refracted.g = refracted.g * max(1.0, contrast * noise) + noise * lift;
        }
        if refract_base_on > 0.5 {
            let base = textureSample(refract_base, refract_base_sampler, v.current_uv);
            refracted += base.rgb * base.a * tint * v.color.a;
        }
        return vec4(refracted, clamp(frame.a * v.color.a, 0.0, 1.0));
    }
    let alpha = frame.a * v.color.a;
#ifdef PARTICLE_MOD2X
    // $MOD2X: Source blends DST_COLOR:SRC_COLOR in its gamma-space target,
    // the sprite fading to neutral grey (0.5) with alpha. In this linear
    // target the same multiply is (2 x gamma colour) ^ 2.2.
    let gamma = pow(max(frame.rgb * v.color.rgb, vec3(0.0)), vec3(1.0 / 2.2)) * settings.z;
    let modulate = mix(vec3(0.5), gamma, clamp(alpha, 0.0, 1.0));
    return vec4(pow(2.0 * modulate, vec3(2.2)), 1.0);
#endif
    if add_self == 0.0 && alpha <= 0.01 { discard; }
    let rgb = frame.rgb * settings.z * v.color.rgb;
    if add_self < 0.0 {
        // Debug variant (L4D2R_ADDSELF_TEX): add the texture x $ADDSELF,
        // without the vertex colour, over the alpha-weighted sprite.
        return vec4(rgb * alpha - add_self * frame.rgb * frame.a * settings.z, alpha);
    }
    if add_self != 0.0 {
        return vec4(rgb * alpha * (1.0 + settings.z * add_self * v.color.a), alpha);
    }
    // Bevy's Add pass shares its premultiplied blend state. Zero output alpha
    // preserves the framebuffer while the source contribution is alpha weighted.
    if settings.w != 0.0 { return vec4(rgb * alpha, 0.0); }
    return vec4(rgb, alpha);
}
@fragment
fn fragment(input: Output) -> @location(0) vec4<f32> {
    var v = input;
#ifdef DEPTH_PREPASS
    // Source DepthFeathering: alpha *= saturate(|scene depth - sprite depth|
    // x constant), the scene depth read behind the sprite.
    if depth_blend > 0.0 && effects.x == 0.0 {
        let scene = depth_ndc_to_view_z(prepass_depth(v.position, 0u));
        let sprite = depth_ndc_to_view_z(v.position.z);
        v.color.a *= clamp(abs(scene - sprite) / depth_blend, 0.0, 1.0);
    }
#endif
    var color = sprite_color(v);
#ifdef PARTICLE_MOD2X
    // A multiplier: exposure and fog already apply to the scene below.
    return color;
#endif
    if effects.x == 0.0 { color = vec4(color.rgb * view.exposure, color.a); }
#ifdef DISTANCE_FOG
    // effects.y: 1 world pixel fog; 2 SpriteCard vertex fog, which Source's
    // HDR world pass skips (material::particle_fog_mode).
    if effects.y == 1.0 {
        let distance = length(v.world_position - view.world_position.xyz);
        let ramp = clamp((distance - fog.be.x) / max(fog.be.y - fog.be.x, 0.0001), 0.0, fog.base_color.a);
        var fog_target = fog.base_color.rgb;
        if add_self != 0.0 { fog_target *= color.a; }
        if settings.w != 0.0 { fog_target = vec3(0.0); }
        color = vec4(mix(color.rgb, fog_target, ramp * ramp), color.a);
    }
#endif
    // 3D skybox: the sky_camera fog, its squared ramp per vertex in blend.w,
    // blended after exposure like the sky meshes (source_lighting).
    if effects.y == 3.0 {
        var fog_target = sky_fog.rgb;
        if add_self != 0.0 { fog_target *= color.a; }
        if settings.w != 0.0 { fog_target = vec3(0.0); }
        color = vec4(mix(color.rgb, fog_target, clamp(v.blend.w, 0.0, 1.0)), color.a);
    }
    return color;
}
