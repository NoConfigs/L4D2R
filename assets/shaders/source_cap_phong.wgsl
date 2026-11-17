// Reconstructed from owned retail phong_ps30 static-zero bytecode. Scope:
// numeric-exponent VertexLitGeneric wound-cap/gib materials, no texture warps.
#import bevy_pbr::pbr_bindings
#import "shaders/source_uniform.wgsl"::source_uniform
struct CapSurface { mask: f32, }
fn cap_surface(uv: vec2<f32>) -> CapSurface {
    let base = textureSample(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, uv);
    let normal_alpha = textureSample(pbr_bindings::normal_map_texture, pbr_bindings::normal_map_sampler, uv).a;
    let alpha_mask = mix(normal_alpha, base.a, source_uniform.cap_phong.controls.x);
    var result: CapSurface;
    result.mask = mix(alpha_mask, dot(base.rgb, vec3<f32>(0.3, 0.59, 0.11)), source_uniform.cap_phong.controls.y);
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
    return pow(max(ndotl, 0.0), source_uniform.cap_phong.params.y);
}
fn cap_specular(surface: CapSurface, normal: vec3<f32>, view_dir: vec3<f32>, light_dir: vec3<f32>) -> vec3<f32> {
    let ndotl = max(dot(normal, light_dir), 0.0);
    let reflected = reflect(-view_dir, normal);
    let highlight = pow(max(dot(reflected, light_dir), 0.0), source_uniform.cap_phong.params.x);
    return source_uniform.cap_phong.tint.rgb * (surface.mask * source_uniform.cap_phong.params.z * ndotl * highlight * cap_fresnel(dot(normal, view_dir)));
}
