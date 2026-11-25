// Adapted from Bevy 0.19.1 pbr_prepass.wgsl (MIT); see BEVY_LICENSE.txt.
// Adapted from Bevy 0.19.1 prepass/prepass_io.wgsl (MIT); see BEVY_LICENSE.txt.
#import bevy_pbr::prepass_io
struct SourcePrepassOutput {
    // This is `clip position` when the struct is used as a vertex stage output
    // and `frag coord` when used as a fragment stage input
    @builtin(position) position: vec4<f32>,

#ifdef VERTEX_UVS_A
    @location(0) uv: vec2<f32>,
#endif

#ifdef VERTEX_UVS_B
    @location(1) uv_b: vec2<f32>,
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    @location(2) world_normal: vec3<f32>,
#ifdef VERTEX_TANGENTS
    @location(3) world_tangent: vec4<f32>,
#endif
#endif // NORMAL_PREPASS_OR_DEFERRED_PREPASS

    @location(4) world_position: vec4<f32>,
#ifdef MOTION_VECTOR_PREPASS
    @location(5) previous_world_position: vec4<f32>,
#endif

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(6) unclipped_depth: f32,
#endif // UNCLIPPED_DEPTH_ORTHO_EMULATION
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(7) instance_index: u32,
#endif

#ifdef VERTEX_COLORS
    @location(8) color: vec4<f32>,
#endif

#ifdef VISIBILITY_RANGE_DITHER
    @location(9) @interpolate(flat) visibility_range_dither: i32,
#endif  // VISIBILITY_RANGE_DITHER

#ifdef SOURCE_WOUNDS
    @location(10) wound_uv0: vec4<f32>,
    @location(11) wound_uv1: vec4<f32>,
    @location(12) wound_clip: vec2<f32>,
#endif
}
fn to_native(v: SourcePrepassOutput) -> prepass_io::VertexOutput {
    var out: prepass_io::VertexOutput;
    out.position = v.position;
#ifdef VERTEX_UVS_A
    out.uv = v.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = v.uv_b;
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = v.world_normal;
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_TANGENTS
    out.world_tangent = v.world_tangent;
#endif
#endif
    out.world_position = v.world_position;
#ifdef MOTION_VECTOR_PREPASS
    out.previous_world_position = v.previous_world_position;
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = v.unclipped_depth;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = v.instance_index;
#endif
#ifdef VERTEX_COLORS
    out.color = v.color;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = v.visibility_range_dither;
#endif
    return out;
}

#ifdef SOURCE_WOUNDS
#import "shaders/source_wounds.wgsl"::wound_blood
#endif
#import bevy_pbr::{
    pbr_prepass_functions,
    pbr_bindings,
    pbr_bindings::material,
    pbr_types,
    pbr_functions,
    pbr_functions::SampleBias,
    prepass_io,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}
#import "shaders/source_uniform.wgsl"::source_uniform

// The same per-map globals as source_lighting.wgsl (native_fog::sky_globals,
// reserved last slot). The prepass must clip at the world farz too: a far
// tree discarded only in the main pass kept its prepass depth, hid the 3D
// skybox behind it, and showed as a blue sky-coloured silhouette (c4m2).
struct SourceLight {
    cube: array<vec4<f32>, 6>,
    sun_direction: vec4<f32>,
    sun_color: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<storage, read> lighting_table: array<SourceLight>;
fn source_far_clip(world_position: vec3<f32>) {
    let far_clip = lighting_table[4095u].cube[2];
    let view_distance = length(world_position - view.world_position.xyz);
    if source_uniform.settings.w > 0.5 {
        if far_clip.y > 0.5 && far_clip.x > 0.0 && view_distance < far_clip.x + far_clip.z {
            discard;
        }
    } else if source_uniform.settings.z < 0.5 && far_clip.x > 0.0 && view_distance > far_clip.x {
        discard;
    }
}

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::resolve_vertex_output
#endif

#ifdef BINDLESS
#import bevy_pbr::pbr_bindings::material_indices
#endif  // BINDLESS

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(
#ifdef MESHLET_MESH_MATERIAL_PASS
    @builtin(position) frag_coord: vec4<f32>,
#else
    wound_in: SourcePrepassOutput,
    @builtin(front_facing) is_front: bool,
#endif
) -> prepass_io::FragmentOutput {
#ifdef MESHLET_MESH_MATERIAL_PASS
    let in = resolve_vertex_output(frag_coord);
    let is_front = true;
#else   // MESHLET_MESH_MATERIAL_PASS
    let in = to_native(wound_in);
#ifdef SOURCE_WOUNDS
    let blood = wound_blood(wound_in.wound_uv0, wound_in.wound_uv1, wound_in.wound_clip);
#endif

#ifdef BINDLESS
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    let flags = pbr_bindings::material_array[material_indices[slot].material].flags;
    let uv_transform = pbr_bindings::material_array[material_indices[slot].material].uv_transform;
#else   // BINDLESS
    let flags = pbr_bindings::material.flags;
    let uv_transform = pbr_bindings::material.uv_transform;
#endif  // BINDLESS

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
#ifdef VISIBILITY_RANGE_DITHER
    pbr_functions::visibility_range_dither(in.position, in.visibility_range_dither);
#endif  // VISIBILITY_RANGE_DITHER

    source_far_clip(in.world_position.xyz);
    pbr_prepass_functions::prepass_alpha_discard(in);
#endif  // MESHLET_MESH_MATERIAL_PASS

    var out: prepass_io::FragmentOutput;

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif // UNCLIPPED_DEPTH_ORTHO_EMULATION

#ifdef NORMAL_PREPASS
    // NOTE: Unlit bit not set means == 0 is true, so the true case is if lit
    if (flags & pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        let double_sided = (flags & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;

        let world_normal = pbr_functions::prepare_world_normal(
            in.world_normal,
            double_sided,
            is_front,
        );

        var normal = world_normal;

#ifdef VERTEX_UVS
#ifdef VERTEX_TANGENTS
#ifdef STANDARD_MATERIAL_NORMAL_MAP

// TODO: Transforming UVs mean we need to apply derivative chain rule for meshlet mesh material pass
#ifdef STANDARD_MATERIAL_NORMAL_MAP_UV_B
        let uv = (uv_transform * vec3(in.uv_b, 1.0)).xy;
#else
        let uv = (uv_transform * vec3(in.uv, 1.0)).xy;
#endif

        // Fill in the sample bias so we can sample from textures.
        var bias: SampleBias;
#ifdef MESHLET_MESH_MATERIAL_PASS
        bias.ddx_uv = in.ddx_uv;
        bias.ddy_uv = in.ddy_uv;
#else   // MESHLET_MESH_MATERIAL_PASS
        bias.mip_bias = view.mip_bias;
#endif  // MESHLET_MESH_MATERIAL_PASS

        let Nt =
#ifdef MESHLET_MESH_MATERIAL_PASS
            textureSampleGrad(
#else   // MESHLET_MESH_MATERIAL_PASS
            textureSampleBias(
#endif  // MESHLET_MESH_MATERIAL_PASS
#ifdef BINDLESS
                bindless_textures_2d[material_indices[slot].normal_map_texture],
                bindless_samplers_filtering[material_indices[slot].normal_map_sampler],
#else   // BINDLESS
                pbr_bindings::normal_map_texture,
                pbr_bindings::normal_map_sampler,
#endif  // BINDLESS
                uv,
#ifdef MESHLET_MESH_MATERIAL_PASS
                bias.ddx_uv,
                bias.ddy_uv,
#else   // MESHLET_MESH_MATERIAL_PASS
                bias.mip_bias,
#endif  // MESHLET_MESH_MATERIAL_PASS
            ).rgb;
        let TBN = pbr_functions::calculate_tbn_mikktspace(normal, in.world_tangent);

        normal = pbr_functions::apply_normal_mapping(
            flags,
            TBN,
            double_sided,
            is_front,
            Nt,
        );

#endif  // STANDARD_MATERIAL_NORMAL_MAP
#endif  // VERTEX_TANGENTS
#endif  // VERTEX_UVS

        out.normal = vec4(normal * 0.5 + vec3(0.5), 1.0);
    } else {
        out.normal = vec4(in.world_normal * 0.5 + vec3(0.5), 1.0);
    }
#endif // NORMAL_PREPASS

#ifdef MOTION_VECTOR_PREPASS
#ifdef MESHLET_MESH_MATERIAL_PASS
    out.motion_vector = in.motion_vector;
#else
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(in.world_position, in.previous_world_position);
#endif
#endif

    return out;
}
#else
@fragment
fn fragment(wound_in: SourcePrepassOutput) {
    let in = to_native(wound_in);
#ifdef SOURCE_WOUNDS
    let blood = wound_blood(wound_in.wound_uv0, wound_in.wound_uv1, wound_in.wound_clip);
#endif
    source_far_clip(in.world_position.xyz);
    pbr_prepass_functions::prepass_alpha_discard(in);
}
#endif // PREPASS_FRAGMENT
