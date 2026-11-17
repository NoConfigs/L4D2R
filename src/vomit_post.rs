//! Retail Boomer-vomit screen post effect (engine_post.vmt / engine_post
//! pixel shader "vomit" path), reconstructed:
//!
//! The boomer_vomit_screeneffect sprites (particle/VomitScreenSplash, a
//! refraction map: RG the offset around 0.5, B the colour blend, A the
//! coverage) are drawn into an offscreen buffer (bile_screen.rs draws them
//! with a UI camera whose target is `VomitBuffer`). This pass then refracts
//! the frame by that buffer and tints it:
//!
//!   offset = (buffer.rg * 2 - 1) * $vomitRefractScale (0.22) * buffer.a
//!   color  = frame(uv + offset)
//!   tint   = mix($vomitColor2, $vomitColor1, buffer.b)
//!            ($vomitColor1 0.675 0.735 0, $vomitColor2 0.599 0.646 0)
//!   out    = mix(color, color * tint, buffer.a)
//!
//! It runs on the world camera before tonemapping, so the HUD stays sharp
//! as in retail. Only while biled.
use bevy::{
    core_pipeline::{schedule::Core3d, tonemapping::tonemapping, Core3dSystems, FullscreenShader},
    prelude::*,
    render::{
        extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{sampler, texture_2d, uniform_buffer},
            BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState,
            ColorWrites, FragmentState, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
            RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType,
            TextureFormat, TextureSampleType,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        texture::GpuImage,
        view::{ExtractedView, ViewTarget},
        Render, RenderApp, RenderStartup, RenderSystems,
    },
};
use std::collections::HashMap;

/// engine_post.vmt.
pub const REFRACT_SCALE: f32 = 0.22;
pub const COLOR_1: [f32; 3] = [0.675, 0.735, 0.0];
pub const COLOR_2: [f32; 3] = [0.599, 0.646, 0.0];

/// On the world camera while the vomit buffer has anything in it.
#[derive(Component, Clone, Copy, Default, ShaderType, ExtractComponent)]
pub struct VomitPost {
    pub color_1: Vec4,
    pub color_2: Vec4,
    /// x: refract scale.
    pub params: Vec4,
}
impl VomitPost {
    pub fn retail() -> Self {
        // Source tints the gamma-space frame; this pass runs on linear colour.
        let linear = |c: [f32; 3]| {
            let l = Color::srgb(c[0], c[1], c[2]).to_linear();
            Vec4::new(l.red, l.green, l.blue, 1.)
        };
        Self {
            color_1: linear(COLOR_1),
            color_2: linear(COLOR_2),
            params: Vec4::new(REFRACT_SCALE, 0., 0., 0.),
        }
    }
}

/// The offscreen buffer the vomit sprites are drawn into.
#[derive(Resource, Clone, ExtractResource)]
pub struct VomitBuffer(pub Handle<Image>);

pub struct VomitPostPlugin;
impl Plugin for VomitPostPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<VomitPost>::default(),
            UniformComponentPlugin::<VomitPost>::default(),
            ExtractResourcePlugin::<VomitBuffer>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, vomit_pass.in_set(Core3dSystems::PostProcess).before(tonemapping));
    }
}

#[derive(Resource)]
struct VomitPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<bevy::shader::Shader>,
    vertex: bevy::render::render_resource::VertexState,
    /// Pipelines per target format.
    pipelines: HashMap<TextureFormat, CachedRenderPipelineId>,
}

fn init_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    asset_server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "vomit_post_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                uniform_buffer::<VomitPost>(true),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        mag_filter: bevy::render::render_resource::FilterMode::Linear,
        min_filter: bevy::render::render_resource::FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(VomitPipeline {
        layout,
        sampler,
        shader: asset_server.load("shaders/vomit_post.wgsl"),
        vertex: fullscreen.to_vertex_state(),
        pipelines: HashMap::new(),
    });
}

#[derive(Component)]
struct VomitPipelineId(CachedRenderPipelineId);

fn prepare_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipeline: ResMut<VomitPipeline>,
    views: Query<(Entity, &ExtractedView, Option<&VomitPost>)>,
) {
    for (entity, view, post) in &views {
        if post.is_none() {
            commands.entity(entity).remove::<VomitPipelineId>();
            continue;
        }
        let format = view.target_format;
        let VomitPipeline { layout, shader, vertex, pipelines, .. } = &mut *pipeline;
        let id = *pipelines.entry(format).or_insert_with(|| {
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("vomit_post_pipeline".into()),
                layout: vec![layout.clone()],
                vertex: vertex.clone(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                    ..default()
                }),
                ..default()
            })
        });
        commands.entity(entity).insert(VomitPipelineId(id));
    }
}

fn vomit_pass(
    view: ViewQuery<(&ViewTarget, &DynamicUniformIndex<VomitPost>, &VomitPipelineId)>,
    pipeline: Res<VomitPipeline>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<VomitPost>>,
    buffer: Option<Res<VomitBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (view_target, index, pipeline_id) = view.into_inner();
    let (Some(render_pipeline), Some(binding), Some(buffer)) = (
        pipeline_cache.get_render_pipeline(pipeline_id.0),
        uniforms.uniforms().binding(),
        buffer,
    ) else {
        return;
    };
    let Some(gpu) = images.get(&buffer.0) else {
        return;
    };
    let post = view_target.post_process_write();
    let bind_group = render_device.create_bind_group(
        "vomit_post_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((post.source, &pipeline.sampler, &gpu.texture_view, binding)),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("vomit_post_pass"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, &bind_group, &[index.index()]);
    pass.draw(0..3, 0..1);
}

