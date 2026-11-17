#import bevy_ui::ui_vertex_output::UiVertexOutput
@group(1) @binding(0) var<uniform> controls:vec4<f32>;
@fragment
fn fragment(in:UiVertexOutput)->@location(0) vec4<f32> {
    let seed=dot(floor(in.position.xy),vec2<f32>(12.9898,78.233))+floor(controls.y*30.0)*37.719;
    let noise=fract(sin(seed)*43758.5453);
    return vec4<f32>(vec3<f32>(noise),controls.x*0.14);
}
