#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;
@group(1) @binding(2) var<uniform> controls: vec4<f32>;
@group(1) @binding(3) var<uniform> tint: vec4<f32>;
@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    if controls.y > 0.5 {
        // Colours sampled from the retail green health fill, converted to linear RGB.
        return vec4<f32>(mix(vec3<f32>(0.0012,0.402,0.0273),vec3<f32>(0.0001,0.2307,0.013),in.uv.y),1.0);
    }
    if controls.x <= 0.0 { return vec4<f32>(0.0); }
    let p=in.uv-vec2<f32>(0.5);
    let angle=fract((atan2(p.x,-p.y)+6.28318530718)/6.28318530718)*6.28318530718;
    let pixel=textureSample(image,image_sampler,in.uv);
    if angle > controls.x*6.28318530718 { return vec4<f32>(0.0); }
    return pixel*tint;
}
