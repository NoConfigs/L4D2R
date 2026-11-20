// Reconstructed from owned retail phong_ps30 static-zero bytecode. Scope:
// numeric-exponent VertexLitGeneric wound-cap/gib materials, no texture warps.
#import bevy_pbr::pbr_bindings
#import "shaders/source_uniform.wgsl"::source_uniform
struct CapSurface { mask: f32, exponent: f32, tint: vec3<f32>, }
// `exponent_map`: the $phongexponenttexture sample (controls.w), else unused.
fn cap_surface(uv: vec2<f32>, exponent_map: vec4<f32>) -> CapSurface {
    let base = textureSample(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, uv);
    let normal_alpha = textureSample(pbr_bindings::normal_map_texture, pbr_bindings::normal_map_sampler, uv).a;
    let alpha_mask = mix(normal_alpha, base.a, source_uniform.cap_phong.controls.x);
    var result: CapSurface;
    result.mask = mix(alpha_mask, dot(base.rgb, vec3<f32>(0.3, 0.59, 0.11)), source_uniform.cap_phong.controls.y);
    // Skin shader: an explicit $phongexponent wins, else 1 + 149 R.
    result.exponent = source_uniform.cap_phong.params.x;
    if result.exponent <= 0.0 && source_uniform.cap_phong.controls.w > 0.5 {
        result.exponent = 1.0 + 149.0 * exponent_map.r;
    }
    result.exponent = max(result.exponent, 1.0);
    // $phongalbedotint: lerp(1, albedo, G); otherwise $phongtint.
    result.tint = source_uniform.cap_phong.tint.rgb;
    if source_uniform.cap_phong.tint.w > 0.5 {
        let amount = select(1.0, exponent_map.g, source_uniform.cap_phong.controls.w > 0.5);
        result.tint = mix(vec3<f32>(1.0), base.rgb, amount);
    }
    return result;
}
fn cap_fresnel(ndotv: f32) -> f32 {
    let inverse = 1.0 - clamp(ndotv, 0.0, 1.0);
    let squared = inverse * inverse;
    // Ordinary Phong uses a fixed front endpoint; Infected multiplies that
    // endpoint by its specular mask, so sharing the Infected helper is wrong.
    if squared <= 0.5 {
        return mix(source_uniform.cap_phong.fresnel.x, source_uniform.cap_phong.fresnel.y, squared * 2.0);
    }
    return mix(source_uniform.cap_phong.fresnel.y, source_uniform.cap_phong.fresnel.z, squared * 2.0 - 1.0);
}
fn cap_diffuse(ndotl: f32) -> f32 {
    if source_uniform.cap_phong.controls.z > 0.5 {
        // $halflambert: (N.L * 0.5 + 0.5)^2.
        let wrapped = clamp(ndotl * 0.5 + 0.5, 0.0, 1.0);
        return pow(wrapped * wrapped, source_uniform.cap_phong.params.y);
    }
    return pow(max(ndotl, 0.0), source_uniform.cap_phong.params.y);
}
fn cap_specular(surface: CapSurface, normal: vec3<f32>, view_dir: vec3<f32>, light_dir: vec3<f32>) -> vec3<f32> {
    let ndotl = max(dot(normal, light_dir), 0.0);
    let reflected = reflect(-view_dir, normal);
    let highlight = pow(max(dot(reflected, light_dir), 0.0), surface.exponent);
    return surface.tint * (surface.mask * source_uniform.cap_phong.params.z * ndotl * highlight * cap_fresnel(dot(normal, view_dir)));
}
