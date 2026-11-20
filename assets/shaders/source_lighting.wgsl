#import bevy_pbr::{

    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT,
}
#import bevy_pbr::pbr_bindings
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::mesh_functions
#ifdef DISTANCE_FOG
#import bevy_pbr::mesh_view_bindings::fog
#endif
#import bevy_pbr::mesh_view_bindings::clustered_lights
#import bevy_pbr::mesh_view_bindings::{lights, directional_shadow_textures}
#import bevy_pbr::shadows::{fetch_spot_shadow, get_cascade_index, world_to_directional_light_local}
#import bevy_pbr::mesh_view_types::{POINT_LIGHT_FLAGS_SPOT_LIGHT_BIT, POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE, POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT}

// Adapted from Bevy 0.19.1 render/forward_io.wgsl (MIT); see BEVY_LICENSE.txt.
#import bevy_pbr::forward_io
#import "shaders/source_uniform.wgsl"::source_uniform
struct SourceVertexOutput {
#ifdef SOURCE_GLASS_MASK
    @location(9) glass_mask_uv: vec2<f32>,
#endif
#ifdef SOURCE_BLEND
    @location(13) source_blend: f32,
#endif
#ifdef SOURCE_BUMPED
    @location(14) source_bump: f32,
#endif
    // This is `clip position` when the struct is used as a vertex stage output
    // and `frag coord` when used as a fragment stage input
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) world_tangent: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(7) @interpolate(flat) visibility_range_dither: i32,
#endif

#ifdef SOURCE_WOUNDS
    @location(10) wound_uv0: vec4<f32>,
    @location(11) wound_uv1: vec4<f32>,
    @location(12) wound_clip: vec2<f32>,
#endif
}
fn to_native(v: SourceVertexOutput) -> forward_io::VertexOutput {
    var out: forward_io::VertexOutput;
    out.position = v.position;
    out.world_position = v.world_position;
    out.world_normal = v.world_normal;
#ifdef VERTEX_UVS_A
    out.uv = v.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = v.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = v.world_tangent;
#endif
#ifdef VERTEX_COLORS
    out.color = v.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = v.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = v.visibility_range_dither;
#endif
    return out;
}

#ifdef SOURCE_WOUNDS
#import "shaders/source_wounds.wgsl"::wound_blood
#import "shaders/source_wounds.wgsl"::wound_mask_value
#endif
#ifdef SOURCE_INFECTED
#import "shaders/source_infected.wgsl"::{infected_surface, infected_specular, infected_palette}
#endif

#ifdef SOURCE_CAP_PHONG
#import "shaders/source_cap_phong.wgsl"::{cap_surface, cap_diffuse, cap_specular}
#endif

// Shared world lightmap atlas (replaces Bevy's per-entity lightmap slots).
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var lightmap_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var lightmap_sampler: sampler;
// x: enabled, y: exposure.
// Per-draw model lighting (MeshTag = slot + 1), shared by all materials so
// dynamically lit draws can batch.
struct SourceLight {
    cube: array<vec4<f32>, 6>,
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<storage, read> lighting_table: array<SourceLight>;

@group(#{MATERIAL_BIND_GROUP}) @binding(134) var bump_texture: texture_2d<f32>;
#ifndef SOURCE_MOBILE_SAMPLERS
@group(#{MATERIAL_BIND_GROUP}) @binding(135) var bump_sampler: sampler;
#endif
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var emission_texture: texture_2d<f32>;
#ifndef SOURCE_MOBILE_SAMPLERS
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var emission_sampler: sampler;
#endif
#ifdef SOURCE_CAP_PHONG
// Phong diffuse for one light; `$lightwarptexture` (fresnel.w, bound in the
// emission slot) replaces it with 2 x the ramp at the half-Lambert value.
fn cap_light(ndotl: f32) -> vec3<f32> {
    if source_uniform.cap_phong.fresnel.w > 0.5 {
        let uv = vec2<f32>(clamp(ndotl * 0.5 + 0.5, 0.0, 1.0), 0.5);
#ifdef SOURCE_MOBILE_SAMPLERS
        return 2.0 * textureSampleLevel(emission_texture, pbr_bindings::base_color_sampler, uv, 0.0).rgb;
#else
        return 2.0 * textureSampleLevel(emission_texture, emission_sampler, uv, 0.0).rgb;
#endif
    }
    return vec3<f32>(cap_diffuse(ndotl));
}
#endif


#ifdef SOURCE_BLEND
@group(#{MATERIAL_BIND_GROUP}) @binding(128) var blend_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(129) var blend_sampler: sampler;
#ifdef SOURCE_BLEND_MODULATE
@group(#{MATERIAL_BIND_GROUP}) @binding(130) var blend_modulate: texture_2d<f32>;
#ifndef SOURCE_MOBILE_SAMPLERS
@group(#{MATERIAL_BIND_GROUP}) @binding(131) var blend_modulate_sampler: sampler;
#endif
#endif
#endif
#ifdef SOURCE_ENVMAP
@group(#{MATERIAL_BIND_GROUP}) @binding(132) var envmap_texture: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(133) var envmap_sampler: sampler;
#endif
#ifdef SOURCE_GLASS_MASK
@group(#{MATERIAL_BIND_GROUP}) @binding(126) var glass_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(127) var glass_mask_sampler: sampler;
#endif
struct FragmentOutput {
    @location(0) color: vec4<f32>,
}

// Source render-to-texture model shadows (crate::model_shadows). The only
// directional light renders a depth map from dynamic models. Returns the
// shadow amount 0..1: occluded taps between the surface and the light, each
// fading linearly to nothing at the shadow_control cast distance (metres).
fn model_shadow(world_position: vec3<f32>, normal: vec3<f32>, max_distance: f32) -> f32 {
    if lights.n_directional_lights == 0u || max_distance <= 0.0 {
        return 0.0;
    }
    let light = &lights.directional_lights[0];
    if ((*light).flags & 1u) == 0u {
        return 0.0;
    }
    let to_light = (*light).direction_to_light.xyz;
    if dot(normal, to_light) <= 0.0 {
        return 0.0;
    }
    let view_z = dot(
        vec4<f32>(view.view_from_world[0].z, view.view_from_world[1].z, view.view_from_world[2].z, view.view_from_world[3].z),
        vec4<f32>(world_position, 1.0),
    );
    let cascade = get_cascade_index(0u, view_z);
    if cascade >= (*light).num_cascades {
        return 0.0;
    }
    let here = world_to_directional_light_local(0u, cascade, vec4<f32>(world_position, 1.0));
    let toward = world_to_directional_light_local(0u, cascade, vec4<f32>(world_position + to_light, 1.0));
    if here.w == 0.0 || toward.w == 0.0 {
        return 0.0;
    }
    // Shadow-map depth change per metre toward the light (either sign).
    let per_metre = toward.z - here.z;
    if abs(per_metre) < 1e-7 {
        return 0.0;
    }
    let size = vec2<i32>(textureDimensions(directional_shadow_textures).xy);
    let centre = vec2<i32>(here.xy * vec2<f32>(size));
    let index = i32((*light).depth_texture_base_index + cascade);
    var amount = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let coords = clamp(centre + vec2<i32>(dx, dy), vec2<i32>(0), size - vec2<i32>(1));
#ifdef NO_ARRAY_TEXTURES_SUPPORT
            let occluder = textureLoad(directional_shadow_textures, coords, 0);
#else
            let occluder = textureLoad(directional_shadow_textures, coords, index, 0);
#endif
            let distance = (occluder - here.z) / per_metre;
            if distance > 0.02 && distance < max_distance {
                amount += 1.0 - distance / max_distance;
            }
        }
    }
    return amount / 9.0;
}

@fragment
fn fragment(in: SourceVertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Per-map globals in the reserved last lighting slot (native_fog::sky_globals).
    let globals = lighting_table[4095u];
    let view_distance = length(in.world_position.xyz - view.world_position.xyz);
    let far_clip = globals.cube[2];
    if source_uniform.settings.w > 0.5 {
        // 3D skybox: Source starts its near plane at the world farz on maps
        // that ask for it, so scaled scenery never pokes into the world.
        if far_clip.y > 0.5 && far_clip.x > 0.0 && view_distance < far_clip.x + far_clip.z {
            discard;
        }
    } else if source_uniform.settings.z < 0.5 && far_clip.x > 0.0 && view_distance > far_clip.x {
        // World geometry is clipped at the fog controller's farz; the 3D
        // skybox drawn behind it shows through, as in Source.
        discard;
    }
    var surface = to_native(in);
    var input = pbr_input_from_standard_material(surface, is_front);
#ifdef SOURCE_BLEND
#ifdef VERTEX_UVS_A
    // WorldVertexTransition: displacement alpha blends $basetexture toward
    // $basetexture2 at the same texture coordinates.
    let second = textureSample(blend_texture, blend_sampler, in.uv);
    var blend = clamp(in.source_blend, 0.0, 1.0);
#ifdef SOURCE_BLEND_MODULATE
    // Source BLENDMODULATETEXTURE: the mask's green is the threshold and red
    // the softness, sampled at the base coordinates ($blendmasktransform
    // identity).
#ifdef SOURCE_MOBILE_SAMPLERS
    let modulate = textureSample(blend_modulate, blend_sampler, in.uv);
#else
    let modulate = textureSample(blend_modulate, blend_modulate_sampler, in.uv);
#endif
    let lo = clamp(modulate.g - modulate.r, 0.0, 1.0);
    let hi = clamp(modulate.g + modulate.r, 0.0, 1.0);
    blend = select(step(lo, blend), smoothstep(lo, hi, blend), hi - lo > 0.0001);
#endif
    input.material.base_color = vec4<f32>(
        mix(input.material.base_color.rgb, second.rgb * pbr_bindings::material.base_color.rgb,
            blend),
        input.material.base_color.a);
#endif
#endif
#ifdef SOURCE_INFECTED
    var projected_blood = 0.0;
#ifdef SOURCE_WOUNDS
    projected_blood = wound_mask_value(in.wound_uv0, in.wound_uv1, in.wound_clip);
#endif
#ifdef VERTEX_UVS_A
    var lookup = source_uniform.infected_lookup;
    var burn_strength = 0.0;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    let appearance_tag = mesh_functions::get_tag(in.instance_index);
    if appearance_tag > 0u {
        let appearance = lighting_table[appearance_tag - 1u];
        if appearance.cube[4].w > 0.5 {
            burn_strength = appearance.cube[3].w / 65535.0;
            let sheet = u32(appearance.cube[2].w);
            let height = f32(textureDimensions(infected_palette).y);
            lookup = vec4<f32>(f32(sheet / 2u) * 0.5, f32(sheet % 2u) * 0.5,
                (appearance.cube[0].w + 0.5) / height, (appearance.cube[1].w + 0.5) / height);
        }
    }
#endif
    let infected = infected_surface(in.uv, projected_blood, lookup, burn_strength);
    input.material.base_color = vec4<f32>(input.material.base_color.rgb *
        infected.color, input.material.base_color.a);
#endif
#else
#ifdef SOURCE_WOUNDS
    input.material.base_color = vec4<f32>(input.material.base_color.rgb *
        wound_blood(in.wound_uv0, in.wound_uv1, in.wound_clip), input.material.base_color.a);
#endif
#endif
#ifdef SOURCE_GLASS_MASK
    // Keep the cracked material's original mapping and baked lightmap;
    // only pane coverage comes from the rotated native edge artwork.
    let glass_coverage = textureSample(glass_mask, glass_mask_sampler, in.glass_mask_uv).a;
    if glass_coverage < (1.0 / 255.0) { discard; }
    input.material.base_color.a *= glass_coverage;
#endif
    input.material.base_color = alpha_discard(input.material, input.material.base_color);
#ifdef SOURCE_FENCE
    // Fence render state: the alpha test of the blended pass.
    if input.material.base_color.a < source_uniform.lightmap_params.z { discard; }
#endif
#ifdef SOURCE_CAP_PHONG
#ifdef VERTEX_UVS_A
#ifdef SOURCE_MOBILE_SAMPLERS
    let cap_exponent_map = textureSample(bump_texture, pbr_bindings::base_color_sampler, in.uv);
#else
    let cap_exponent_map = textureSample(bump_texture, bump_sampler, in.uv);
#endif
    let cap = cap_surface(in.uv, cap_exponent_map);
    let cap_view = normalize(view.world_position.xyz - input.world_position.xyz);
#endif
#endif
    var lighting = vec3<f32>(1.0);
    var reflected_light = vec3<f32>(0.0);
#ifdef SOURCE_INFECTED
#ifdef VERTEX_UVS_A
    let infected_view = normalize(view.world_position.xyz - input.world_position.xyz);
#endif
#endif
    if source_uniform.settings.y > 0.5 && (input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        var cube = source_uniform.ambient_cube;
        var sun_dir = source_uniform.sun_direction;
        var sun_col = source_uniform.sun_color;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
        let tag = mesh_functions::get_tag(in.instance_index);
        if tag > 0u {
            let light = lighting_table[tag - 1u];
            cube = light.cube;
            sun_dir = light.sun_direction;
            sun_col = light.sun_color;
        }
#endif
        let world_n = normalize(input.N);
        let n = vec3<f32>(world_n.x, -world_n.z, world_n.y);
        lighting = cube[select(0u, 1u, n.x < 0.0)].rgb * n.x * n.x
            + cube[select(2u, 3u, n.y < 0.0)].rgb * n.y * n.y
            + cube[select(4u, 5u, n.z < 0.0)].rgb * n.z * n.z;
#ifdef SOURCE_CAP_PHONG
        lighting += sun_col.rgb * cap_light(dot(world_n, sun_dir.xyz));
#ifdef VERTEX_UVS_A
        reflected_light += sun_col.rgb * cap_specular(cap, world_n, cap_view, sun_dir.xyz);
#endif
#else
#ifdef SOURCE_HALF_LAMBERT
        // $halflambert: Source's wrapped diffuse, (N.L * 0.5 + 0.5)^2.
        let wrapped = clamp(dot(world_n, sun_dir.xyz) * 0.5 + 0.5, 0.0, 1.0);
        lighting += sun_col.rgb * wrapped * wrapped;
#else
        lighting += sun_col.rgb * max(dot(world_n, sun_dir.xyz), 0.0);
#endif
#endif
#ifdef SOURCE_INFECTED
#ifdef VERTEX_UVS_A
        reflected_light += sun_col.rgb * infected_specular(infected, world_n, infected_view, sun_dir.xyz);
#endif
#endif
    }
#ifdef LIGHTMAP
    if (input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        lighting = input.lightmap_light;
    }
#endif
#ifdef VERTEX_UVS_B
    // Same sample as Bevy's lightmap(): identity uv_rect, mip 0, x exposure.
    if source_uniform.lightmap_params.x > 0.5 && (input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        lighting = textureSampleLevel(lightmap_texture, lightmap_sampler, in.uv_b, 0.0).rgb * source_uniform.lightmap_params.y;
#ifdef SOURCE_BUMPED
#ifdef VERTEX_UVS_A
        // Bumped lightmaps (SURF_BUMPLIGHT): vrad's three bump-basis
        // lightmaps sit beside the flat one in the atlas, combined through
        // the bump map as Source's LightmappedGeneric does.
        if in.source_bump > 0.0 {
            let stride = vec2<f32>(in.source_bump, 0.0);
            let l1 = textureSampleLevel(lightmap_texture, lightmap_sampler, in.uv_b + stride, 0.0).rgb;
            let l2 = textureSampleLevel(lightmap_texture, lightmap_sampler, in.uv_b + stride * 2.0, 0.0).rgb;
            let l3 = textureSampleLevel(lightmap_texture, lightmap_sampler, in.uv_b + stride * 3.0, 0.0).rgb;
#ifdef SOURCE_MOBILE_SAMPLERS
            let bump = textureSample(bump_texture, pbr_bindings::base_color_sampler, in.uv).rgb;
#else
            let bump = textureSample(bump_texture, bump_sampler, in.uv).rgb;
#endif
            var basis_light: vec3<f32>;
            if source_uniform.bump.y > 0.5 {
                // $ssbump: the channels are the basis weights.
                basis_light = l1 * bump.x + l2 * bump.y + l3 * bump.z;
            } else {
                // Normal map: squared, normalised dot with the tangent-space
                // bump basis (Source's g_localBumpBasis).
                let n = normalize(bump * 2.0 - 1.0);
                var dp = vec3<f32>(
                    saturate(dot(n, vec3<f32>(0.81649658, 0.0, 0.57735027))),
                    saturate(dot(n, vec3<f32>(-0.40824829, 0.70710678, 0.57735027))),
                    saturate(dot(n, vec3<f32>(-0.40824829, -0.70710678, 0.57735027))),
                );
                dp = dp * dp;
                basis_light = (l1 * dp.x + l2 * dp.y + l3 * dp.z) / max(dp.x + dp.y + dp.z, 0.0001);
            }
            lighting = basis_light * source_uniform.lightmap_params.y;
        }
#endif
#endif
    }
#endif
    // Bounded dynamic lights: the flashlight and infected foot light.
    // Native world lights remain baked. Do not assume the spotlight is slot zero.
    if source_uniform.settings.w < 0.5 && source_uniform.settings.z < 0.5 {
#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3
        let light_count = min(2u,arrayLength(&clustered_lights.data));
#else
        let light_count = 2u;
#endif
        for (var light_index=0u; light_index<light_count; light_index++) {
        let torch = clustered_lights.data[light_index];
        if (torch.flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_BIT) != 0u && dot(torch.color_inverse_square_range.rgb,vec3<f32>(1.0)) > 0.0 {
            let delta=input.world_position.xyz-torch.position_radius.xyz;
            let distance2=max(dot(delta,delta),0.05);
            var direction=vec3<f32>(torch.light_custom_data.x,0.0,torch.light_custom_data.y);
            direction.y=sqrt(max(0.0,1.0-direction.x*direction.x-direction.z*direction.z));
            if (torch.flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u {direction.y=-direction.y;}
            let cone=clamp(dot(normalize(delta),direction)*torch.light_custom_data.z+torch.light_custom_data.w,0.0,1.0);
            var shadow=1.0;
            if cone>0.0 && (torch.flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
                shadow=fetch_spot_shadow(light_index,input.world_position,input.N,torch.shadow_map_near_z,in.position.xy);
            }
            let falloff=pow(max(0.0,1.0-distance2*torch.color_inverse_square_range.w),2.0)/distance2;
#ifdef SOURCE_CAP_PHONG
            lighting+=torch.color_inverse_square_range.rgb*0.3*cone*cone*falloff*shadow*cap_light(dot(input.N,-normalize(delta)));
#ifdef VERTEX_UVS_A
            reflected_light+=torch.color_inverse_square_range.rgb*0.3*cone*cone*falloff*shadow*cap_specular(cap,normalize(input.N),cap_view,-normalize(delta));
#endif
#else
            lighting+=torch.color_inverse_square_range.rgb*0.3*cone*cone*falloff*shadow*max(dot(input.N,-normalize(delta)),0.0);
#endif
#ifdef SOURCE_INFECTED
#ifdef VERTEX_UVS_A
            reflected_light += torch.color_inverse_square_range.rgb*0.3*cone*cone*falloff*shadow
                * infected_specular(infected, normalize(input.N), infected_view, -normalize(delta));
#endif
#endif
        } else if dot(torch.color_inverse_square_range.rgb,vec3<f32>(1.0))>0.0 {
            let delta= torch.position_radius.xyz-input.world_position.xyz;
            let distance2=max(dot(delta,delta),0.05);
            let falloff=pow(max(0.0,1.0-distance2*torch.color_inverse_square_range.w),2.0)/distance2;
#ifdef SOURCE_CAP_PHONG
            lighting+=torch.color_inverse_square_range.rgb*0.3*falloff*cap_light(dot(input.N,normalize(delta)));
#ifdef VERTEX_UVS_A
            reflected_light+=torch.color_inverse_square_range.rgb*0.3*falloff*cap_specular(cap,normalize(input.N),cap_view,normalize(delta));
#endif
#else
            lighting+=torch.color_inverse_square_range.rgb*0.3*falloff*max(dot(input.N,normalize(delta)),0.0);
#endif
#ifdef SOURCE_INFECTED
#ifdef VERTEX_UVS_A
            reflected_light += torch.color_inverse_square_range.rgb*0.3*falloff
                * infected_specular(infected, normalize(input.N), infected_view, normalize(delta));
#endif
#endif
        }
        }
    }
    // Dynamic model shadows fall on the lightmapped world and on static props
    // (Source's shadow manager projects its render-to-texture shadows onto
    // brushes and static-prop receivers, not onto other models).
    var shadow_receiver = source_uniform.lightmap_params.x > 0.5;
#ifdef VERTEX_COLORS
    // Static props: baked irradiance in vertex colours, no model lighting,
    // not the view model.
    shadow_receiver = shadow_receiver || (source_uniform.settings.y < 0.5 && source_uniform.settings.z < 0.5);
#endif
    if shadow_receiver && source_uniform.settings.w < 0.5 && globals.cube[3].w > 0.5 {
        let amount = model_shadow(input.world_position.xyz, normalize(input.N), globals.cube[4].x);
        lighting *= mix(vec3<f32>(1.0), globals.cube[3].xyz, amount);
    }
    // Source's baked diffuse is albedo * irradiance, not an additional PBR light.
    // Props carry their baked or ambient irradiance in linear vertex colours.
    var out: FragmentOutput;
    var diffuse = input.material.base_color.rgb * lighting;
#ifdef VERTEX_UVS_A
    if source_uniform.emission_tint.w > 0.5 {
        // Sample albedo independently: static vertex colours contain baked
        // irradiance and must not darken the self-illuminated part.
        let albedo = textureSample(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, in.uv);
        var mask = vec3(albedo.a);
        if source_uniform.emission_tint.w > 1.5 {
#ifdef SOURCE_MOBILE_SAMPLERS
            mask = textureSample(emission_texture, pbr_bindings::base_color_sampler, in.uv).rgb;
#else
            mask = textureSample(emission_texture, emission_sampler, in.uv).rgb;
#endif
        }
        diffuse = mix(diffuse, albedo.rgb * pbr_bindings::material.base_color.rgb * source_uniform.emission_tint.rgb, mask);
    }
#endif
#ifdef SOURCE_ENVMAP
    // $envmap: the capture along the reflected view ray (Source axes), with
    // $envmapcontrast (lerp toward the square), $envmapsaturation and
    // $envmaptint, masked by the base or normal-map alpha when asked.
    if source_uniform.envmap_tint.w > 0.5 {
        let env_n = normalize(input.N);
        let env_v = normalize(input.world_position.xyz - view.world_position.xyz);
        let env_r = reflect(env_v, env_n);
        var spec = textureSample(envmap_texture, envmap_sampler, vec3<f32>(env_r.x, -env_r.z, env_r.y)).rgb;
        let params = source_uniform.envmap_params;
        spec = mix(spec, spec * spec, params.y);
        spec = mix(vec3<f32>(dot(spec, vec3<f32>(0.299, 0.587, 0.114))), spec, params.x);
        spec = spec * source_uniform.envmap_tint.rgb;
        var env_mask = 1.0;
#ifdef VERTEX_UVS_A
        if params.z > 0.5 {
            env_mask = textureSample(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, in.uv).a;
        } else if params.w > 0.5 {
            env_mask = textureSample(pbr_bindings::normal_map_texture, pbr_bindings::normal_map_sampler, in.uv).a;
            // $ssbump materials bind no tangent normal map: their bump map
            // (and its alpha mask) is in the bump slot. Reading the empty
            // normal slot gave a full mask; c2m2's saferoom plaster showed a
            // bright cyan reflection of the room light.
            if source_uniform.bump.y > 0.5 {
#ifdef SOURCE_MOBILE_SAMPLERS
                env_mask = textureSample(bump_texture, pbr_bindings::base_color_sampler, in.uv).a;
#else
                env_mask = textureSample(bump_texture, bump_sampler, in.uv).a;
#endif
            }
        }
#endif
        reflected_light += spec * env_mask;
    }
#endif
    out.color = vec4<f32>((diffuse + reflected_light) * source_uniform.settings.x * view.exposure, input.material.base_color.a);
#ifdef DISTANCE_FOG
    // Source CalcRangeFog caps the ramp at maxdensity. BlendPixelFog squares
    // that factor, and FinalOutput blends authored fog after scene exposure.
    // Keeping the ramp linear or exposing fog again washes out nearby detail.
    if source_uniform.settings.z < 0.5 && source_uniform.settings.w < 0.5 && (input.material.flags & STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT) != 0u {
        let distance = length(input.world_position.xyz - view.world_position.xyz);
        let start = fog.be.x;
        let end = fog.be.y;
        let fog_color = fog.base_color;
        let factor = clamp((distance - start) / max(end - start, 0.0001), 0.0, fog_color.a);
        out.color = vec4<f32>(mix(out.color.rgb, fog_color.rgb, factor * factor), out.color.a);
    }
    input.material.flags &= ~STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
#endif
    if source_uniform.settings.w > 0.5 && globals.cube[0].w > 0.5 {
        // The 3D skybox uses the sky_camera's own fog (scaled to world units),
        // not the world fog that would wash scaled scenery out completely.
        let sky_fog = globals.cube[0];
        let factor = clamp((view_distance - sky_fog.x) / max(sky_fog.y - sky_fog.x, 0.0001), 0.0, sky_fog.z);
        out.color = vec4<f32>(mix(out.color.rgb, globals.cube[1].rgb, factor * factor), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(input, out.color);
    return out;
}

