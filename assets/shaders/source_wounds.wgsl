// Independently reconstructed from the installed infected_vs30/ps30 bytecode.
#import "shaders/source_uniform.wgsl"::source_uniform
// Projection happens per vertex, before skinning; saturation happens per pixel.
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var wound_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var wound_sampler: sampler;

struct Projection { uv: vec4<f32>, clip: f32, }
fn wound_position(p: vec4<f32>, row: u32) -> vec3<f32> {
    return vec3<f32>(dot(source_uniform.wound_rows[row], p), dot(source_uniform.wound_rows[row+1u], p), dot(source_uniform.wound_rows[row+2u], p));
}
fn project_wound(p: vec4<f32>, slot: u32) -> Projection {
    var result: Projection;
    result.uv = vec4<f32>(0.0);
    result.clip = 2.0;
    // Retail has two projectors in total, not two independent pairs. A second
    // requested wound replaces the first wound's secondary limb envelope.
    // Keep the existing vertex interface, but only its first pair is active.
    if slot == 0u && source_uniform.wound.params[0].z > 0.5 {
        let q = wound_position(p, 0u);
        let two_wounds = source_uniform.wound.params[1].z > 0.5;
        var other = q;
        if source_uniform.wound.params[0].w > 0.5 { other = wound_position(p, 3u); }
        if two_wounds { other = wound_position(p, 6u); }
        // Cull type zero: project primary twice and clip inside
        // secondary. Cull type one: project both and bypass envelope clipping.
        // The shader API inverts the client's secondary-envelope flag; a
        // missing secondary duplicates primary with cull type one.
        let dual_projection = two_wounds || source_uniform.wound.params[0].w < 0.5;
        var projected_other = q;
        if dual_projection { projected_other = other; }
        let denominator = sqrt(max(vec2<f32>(1.0-q.z*q.z, 1.0-projected_other.z*projected_other.z),
            vec2<f32>(0.0000000001))) * source_uniform.wound.params[0].x;
        // client 10316ed0: bullet bias (.5,.5), slash bias (.5,.75).
        let first_bias = vec2<f32>(0.5, select(0.5, 0.75, source_uniform.wound.atlas[0].z > 0.25));
        var second_bias = first_bias;
        if two_wounds { second_bias = vec2<f32>(0.5, select(0.5, 0.75, source_uniform.wound.atlas[1].z > 0.25)); }
        result.uv = vec4<f32>(vec2<f32>(q.x,-q.y)/denominator.x + first_bias,
            vec2<f32>(projected_other.x,-projected_other.y)/denominator.y + second_bias);
        result.clip = length(other) + select(0.0, 1.0, dual_projection);
    }
    return result;
}
fn wound_sample(uv: vec4<f32>, secondary: f32, slot: u32) -> f32 {
    if slot != 0u || source_uniform.wound.params[0].z < 0.5 { return 0.0; }
    let atlas = source_uniform.wound.atlas[slot];
    var other_atlas = atlas;
    if source_uniform.wound.params[1].z > 0.5 { other_atlas = source_uniform.wound.atlas[1]; }
    let coords = clamp(uv, vec4<f32>(0.0), vec4<f32>(1.0));
    let a = textureSample(wound_mask, wound_sampler, coords.xy * atlas.xy + atlas.zw).r;
    let b = textureSample(wound_mask, wound_sampler, coords.zw * other_atlas.xy + other_atlas.zw).r;
    let mask = max(a,b);
    if mask > source_uniform.wound.params[slot].y || secondary < 1.0 { discard; }
    return mask;
}
fn wound_blood(uv0: vec4<f32>, uv1: vec4<f32>, secondary: vec2<f32>) -> vec3<f32> {
    return blood_tint(wound_mask_value(uv0, uv1, secondary));
}
fn wound_mask_value(uv0: vec4<f32>, uv1: vec4<f32>, secondary: vec2<f32>) -> f32 {
    return max(wound_sample(uv0, secondary.x, 0u), wound_sample(uv1, secondary.y, 1u));
}
fn blood_tint(mask: f32) -> vec3<f32> {
    return mix(vec3<f32>(1.0), source_uniform.wound.blood.rgb,
        smoothstep(source_uniform.wound.blood_range.x, source_uniform.wound.blood_range.y, mask));
}
