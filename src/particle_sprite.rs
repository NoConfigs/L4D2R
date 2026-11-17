//! Frame-interpolated SpriteCard subset for owned first-person muzzle particles.
use bevy::{
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError, VertexFormat,
    },
    shader::ShaderRef,
};

pub const FRAME_BLEND: MeshVertexAttribute =
    MeshVertexAttribute::new("SourceFrameBlend", 91231, VertexFormat::Float32x3);
pub const SECOND_UV: MeshVertexAttribute =
    MeshVertexAttribute::new("SourceSecondUV", 91232, VertexFormat::Float32x4);

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SpriteMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub texture: Handle<Image>,
    #[uniform(2)]
    pub settings: Vec4,
    #[uniform(3)]
    pub add_self: f32,
    #[uniform(4)]
    pub dual: Vec4,
    pub additive: bool,
    pub sizes: Vec4,
}
impl Material for SpriteMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/particle_sprite.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/particle_sprite.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        if self.add_self != 0. {
            AlphaMode::Premultiplied
        } else if self.additive {
            AlphaMode::Add
        } else {
            AlphaMode::Blend
        }
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_1.at_shader_location(2),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(3),
            FRAME_BLEND.at_shader_location(4),
            SECOND_UV.at_shader_location(5),
        ])?];
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}
