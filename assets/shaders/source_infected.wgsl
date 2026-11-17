// Reconstructed from the installed infected_ps30 bytecode. Packed RGB is
// linear data; the palette is sRGB and is decoded by its texture view.
#import "shaders/source_wounds.wgsl"::blood_tint
#import "shaders/source_uniform.wgsl"::source_uniform
@group(#{MATERIAL_BIND_GROUP}) @binding(115) var infected_base: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(116) var infected_base_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(117) var infected_palette: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(118) var infected_palette_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var infected_detail: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(122) var infected_detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(123) var infected_burn: texture_2d<f32>;
#ifndef SOURCE_MOBILE_SAMPLERS
@group(#{MATERIAL_BIND_GROUP}) @binding(124) var infected_burn_sampler: sampler;
#endif

struct InfectedSurface {
    color: vec3<f32>,
    exponent: f32,
    specular_mask: f32,
}

fn infected_surface(uv: vec2<f32>, projected_blood: f32, lookup: vec4<f32>, burn_strength: f32) -> InfectedSurface {
    let packed = textureSample(infected_base, infected_base_sampler, uv);
    let flags = u32(source_uniform.infected_phong.detail.w);
    if (flags & 4u) != 0u {
        // DISABLEVARIATION uses ordinary base-colour pixels and alpha specular,
        // not the modern packed palette/quarter-sheet encoding. Retail performs
        // this sRGB conversion explicitly because base sampler zero is linear.
        let low = packed.rgb / 12.92;
        let high = pow((packed.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
        let linear = select(high, low, packed.rgb <= vec3<f32>(0.04045));
        let blood = projected_blood * source_uniform.infected_phong.exponents.w;
        var legacy: InfectedSurface;
        legacy.color = linear * mix(vec3<f32>(1.0), source_uniform.wound.blood.rgb, clamp(blood, 0.0, 1.0));
        legacy.exponent = source_uniform.infected_phong.exponents.x;
        legacy.specular_mask = max(packed.a, blood * dot(packed.rgb, vec3<f32>(0.3, 0.59, 0.11)));
        return legacy;
    }
    let skin = clamp(packed.b * 2.0 - 1.0, 0.0, 1.0);
    let cloth = clamp(1.0 - packed.b * 2.0, 0.0, 1.0);
    let skin_color = textureSample(infected_palette, infected_palette_sampler,
        vec2<f32>(packed.a, lookup.z)).rgb;
    let cloth_color = textureSample(infected_palette, infected_palette_sampler,
        vec2<f32>(packed.a, lookup.w)).rgb;
    let gray = vec3<f32>(packed.a * packed.a);
    var albedo = mix(mix(gray, skin_color, skin), cloth_color, cloth);
    // Native samples green in one quarter of the same base texture. Blue at
    // full UV selects skin/clothing, and must not be reused as a blood mask.
    let quarter = textureSample(infected_base, infected_base_sampler, uv * 0.5 + lookup.xy);
    let authored = clamp(quarter.g * 2.0 - 1.0, 0.0, 1.0);
    let blood = smoothstep(source_uniform.wound.blood_range.x, source_uniform.wound.blood_range.y, authored + projected_blood);
    let detail_weight = clamp(quarter.r * 2.0 - 1.0, 0.0, 1.0);
    let inverse_red = clamp(1.0 - quarter.r * 2.0, 0.0, 1.0);
    let detail_uv = uv * source_uniform.infected_phong.detail.yz;
    var detail_texel = vec4<f32>(0.0);
    if (flags & 1u) != 0u {
        // Native no-sRGB detail sampler uses swapped UV components.
        detail_texel = textureSample(infected_detail, infected_detail_sampler, detail_uv.yx);
        albedo = mix(albedo, gray * detail_texel.rgb * 2.0, detail_weight);
    }
    var result: InfectedSurface;
    result.color = albedo * blood_tint(authored + projected_blood);
    var skin_exponent = source_uniform.infected_phong.exponents.y;
    if (flags & 1u) != 0u {
        skin_exponent = mix(skin_exponent, source_uniform.infected_phong.detail.x, detail_weight);
    }
    result.exponent = mix(mix(source_uniform.infected_phong.exponents.x, skin_exponent, skin), source_uniform.infected_phong.exponents.z, blood);
    // Retail no-detail permutation uses inverse quarter-sheet red, not the
    // alpha used for palette luminance, and not the blood mask itself.
    result.specular_mask = inverse_red;
    if (flags & 1u) != 0u {
        let base_specular = clamp(mix(inverse_red, detail_texel.a, detail_weight), 0.0, 1.0);
        result.specular_mask = mix(base_specular, source_uniform.infected_phong.exponents.w, blood * detail_texel.a);
    }
    if (flags & 2u) != 0u {
        // Burn reads its own sRGB texture using unswapped detail UVs.
#ifdef SOURCE_MOBILE_SAMPLERS
        let burn = textureSample(infected_burn, infected_base_sampler, detail_uv);
#else
        let burn = textureSample(infected_burn, infected_burn_sampler, detail_uv);
#endif
        let amount = clamp(burn_strength * (2.0 - skin) + burn.a - 1.0, 0.0, 1.0);
        result.color = mix(result.color, burn.rgb, amount);
    }
    return result;
}
fn infected_color(uv: vec2<f32>, projected_blood: f32) -> vec3<f32> {
    return infected_surface(uv, projected_blood, source_uniform.infected_lookup, 0.0).color;
}

// Recovered g_FresnelSpecParams arithmetic from retail infected_ps30, including
// its specular-mask-dependent front endpoint. This is squared inverse N.V,
// with a midpoint at 0.5; it is not Schlick's fifth-power Fresnel.
fn infected_fresnel(ndotv: f32, mask: f32) -> f32 {
    let inverse = 1.0 - clamp(ndotv, 0.0, 1.0);
    let squared = inverse * inverse;
    if squared <= 0.5 {
        return mix(mask * source_uniform.infected_phong.fresnel.x, source_uniform.infected_phong.fresnel.y, squared * 2.0);
    }
    return mix(source_uniform.infected_phong.fresnel.y, source_uniform.infected_phong.fresnel.z, squared * 2.0 - 1.0);
}

// Returns reflected-light response; caller supplies the same authored light
// colour, attenuation and visibility used for that light's diffuse term.
fn infected_specular(surface: InfectedSurface, normal: vec3<f32>, view_dir: vec3<f32>, light_dir: vec3<f32>) -> vec3<f32> {
    if source_uniform.infected_phong.tint.w < 0.5 { return vec3<f32>(0.0); }
    let ndotl = max(dot(normal, light_dir), 0.0);
    let reflection = reflect(-view_dir, normal);
    let highlight = pow(max(dot(reflection, light_dir), 0.0), max(surface.exponent, 0.0001));
    let fresnel = infected_fresnel(dot(normal, view_dir), surface.specular_mask);
    return source_uniform.infected_phong.tint.rgb * source_uniform.infected_phong.fresnel.w * surface.specular_mask * ndotl * highlight * fresnel;
}
