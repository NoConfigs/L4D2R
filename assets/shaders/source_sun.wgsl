#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var sprite: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var sprite_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> colour: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(sprite, sprite_sampler, in.uv);
    // Pure additive contribution: zero alpha preserves the scene beneath.
    // Camera HDR tonemapping processes this together with the world/sky.
    return vec4(texel.rgb * texel.a * colour.rgb * colour.a * view.exposure, 0.0);
}
