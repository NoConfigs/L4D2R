// Retail engine_post vomit path (vomit_post.rs): refract the frame by the
// screen-space vomit buffer and tint it with $vomitColor1/2.
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var vomit: texture_2d<f32>;
struct VomitPost {
    color_1: vec4<f32>,
    color_2: vec4<f32>,
    params: vec4<f32>,
}
@group(0) @binding(3) var<uniform> settings: VomitPost;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let v = textureSample(vomit, screen_sampler, in.uv);
    let amount = clamp(v.a, 0.0, 1.0);
    let offset = (v.rg * 2.0 - 1.0) * settings.params.x * amount;
    let color = textureSample(screen, screen_sampler, clamp(in.uv + offset, vec2<f32>(0.001), vec2<f32>(0.999)));
    let tint = mix(settings.color_2.rgb, settings.color_1.rgb, v.b);
    return vec4<f32>(mix(color.rgb, color.rgb * tint, amount), color.a);
}
