#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var mask: texture_2d<f32>;
@group(1) @binding(1) var mask_sampler: sampler;
@group(1) @binding(2) var<uniform> effects: vec4<f32>;
@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Preserve the previous halo width in screen pixels after doubling mask resolution.
    let step = 2.0 / vec2<f32>(textureDimensions(mask));
    let center = textureSample(mask, mask_sampler, in.uv);
    var halo = vec4<f32>(0.0);
    var total_weight = 0.0;
    let directions = array<vec2<f32>,8>(vec2(1.0,0.0),vec2(-1.0,0.0),vec2(0.0,1.0),vec2(0.0,-1.0),vec2(0.707,0.707),vec2(-0.707,0.707),vec2(0.707,-0.707),vec2(-0.707,-0.707));
    for (var ring: i32 = 1; ring <= 3; ring += 1) {
        let radius = f32(ring)*effects.x/3.0;
        let weight = 1.0-f32(ring)*0.25;
        for (var i: i32 = 0; i < 8; i += 1) {
            let sample = textureSample(mask,mask_sampler,in.uv+directions[i]*step*radius)*weight;
            halo += sample;
            total_weight += weight;
        }
    }
    halo /= total_weight;
    let strength = max(halo.a-center.a,0.0);
    if strength < 0.005 { return vec4<f32>(0.0); }
    // Transparent mask stores premultiplied colour. Preserve the actual
    // survivor/item/status palette rather than choosing only blue or orange.
    let tint = halo.rgb/max(halo.a,0.0001);
    // Retail has a bright inner rim and a soft coloured outer halo.
    let core = clamp(strength*2.5,0.0,1.0);
    let rim = mix(tint,vec3<f32>(1.0),core*core);
    return vec4<f32>(rim,clamp(strength*8.0*effects.y,0.0,1.0));
}
