#import bevy_pbr::mesh_functions
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var sprite: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var sprite_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> settings: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> add_self: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> dual: vec4<f32>;
struct Vertex {
    @builtin(instance_index) instance: u32,
    @location(0) position: vec3<f32>,
    @location(1) current_uv: vec2<f32>,
    @location(2) next_uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) blend: vec3<f32>,
    @location(5) second_uv: vec4<f32>,
}
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) current_uv: vec2<f32>,
    @location(1) next_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) blend: vec3<f32>,
    @location(4) second_uv: vec4<f32>,
}
@vertex
fn vertex(v: Vertex) -> Output {
    var o: Output;
    o.position = mesh_functions::mesh_position_local_to_clip(mesh_functions::get_world_from_local(v.instance), vec4(v.position, 1.0));
    o.current_uv = v.current_uv;
    o.next_uv = v.next_uv;
    o.color = vec4(pow(max(v.color.rgb, vec3(0.0)), vec3(2.2)), v.color.a) * v.blend.z;
    o.blend = v.blend;
    o.second_uv = v.second_uv;
    return o;
}
@fragment
fn fragment(v: Output) -> @location(0) vec4<f32> {
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
    let alpha = frame.a * v.color.a;
    if add_self == 0.0 && alpha <= 0.01 { discard; }
    let rgb = frame.rgb * settings.z * v.color.rgb;
    if add_self != 0.0 {
        return vec4(rgb * alpha * (1.0 + settings.z * add_self * v.color.a), alpha);
    }
    // Bevy's Add pass shares its premultiplied blend state. Zero output alpha
    // preserves the framebuffer while the source contribution is alpha weighted.
    if settings.w != 0.0 { return vec4(rgb * alpha, 0.0); }
    return vec4(rgb, alpha);
}
