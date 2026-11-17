//! World particle batches draw static, never-modified capacity meshes and pull
//! their live vertices from one shared storage buffer written once per frame.
//! Rewriting a `Mesh` asset per visible batch made Bevy free, reallocate and
//! upload every batch separately (about 1.4 ms of render thread per frame in a
//! Hotel horde). Native sprite frames and blend controls share that upload.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    render::{render_resource::AsBindGroup, storage::ShaderBuffer},
    shader::ShaderRef,
};

/// Shared 80-byte slots: position, current/next UV, colour, blend, secondary UV.
pub(crate) const VERTEX_CAPACITY: usize = 1 << 16;
/// Static capacity meshes hold 1, 2, 4 ... 2^MAX_QUAD_BITS quads.
pub(crate) const MAX_QUAD_BITS: u32 = 13;
const VERTEX_BYTES: usize = 80;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(ParticlePullKey)]
pub struct ParticlePull {
    #[storage(100, read_only)]
    pub vertices: Handle<ShaderBuffer>,
    #[texture(101)]
    #[sampler(102)]
    pub texture: Handle<Image>,
    #[uniform(103)]
    pub settings: Vec4,
    #[uniform(104)]
    pub add_self: f32,
    #[uniform(105)]
    pub dual: Vec4,
    #[uniform(106)]
    pub effects: Vec4,
    #[texture(107)]
    #[sampler(108)]
    pub spitter_noise: Handle<Image>,
    /// SpriteCard $DEPTHBLEND fade distance in metres (0 when off).
    #[uniform(109)]
    pub depth_blend: f32,
    /// SpriteCard $MOD2X: the sprite multiplies the scene by twice its
    /// colour (mid-grey leaves it unchanged), e.g. fire glows.
    pub mod2x: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParticlePullKey {
    mod2x: bool,
}
impl From<&ParticlePull> for ParticlePullKey {
    fn from(value: &ParticlePull) -> Self {
        Self { mod2x: value.mod2x }
    }
}
impl MaterialExtension for ParticlePull {
    fn specialize(
        _: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Refract uses the transmissive pass but remains a depth-tested decal,
        // not a solid surface. Sprite particles likewise never write depth.
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        // Keep scene-copy scheduling in the transmissive phase, but composite
        // the sampled colour using the sprite mask. Opaque compositing erases
        // earlier pool sprites even where this sprite is transparent.
        if key
            .mesh_key
            .contains(bevy::pbr::MeshPipelineKey::READS_VIEW_TRANSMISSION_TEXTURE)
        {
            if let Some(target) = descriptor
                .fragment
                .as_mut()
                .and_then(|f| f.targets.first_mut())
                .and_then(Option::as_mut)
            {
                target.blend = Some(bevy::render::render_resource::BlendState::ALPHA_BLENDING);
            }
        }
        if key.bind_group_data.mod2x {
            use bevy::render::render_resource::{BlendComponent, BlendFactor, BlendOperation, BlendState};
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("PARTICLE_MOD2X".into());
                if let Some(target) = fragment.targets.first_mut().and_then(Option::as_mut) {
                    // The shader outputs the scene multiplier; alpha is kept.
                    target.blend = Some(BlendState {
                        color: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::Src, operation: BlendOperation::Add },
                        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
                    });
                }
            }
        }
        Ok(())
    }
    fn vertex_shader() -> ShaderRef {
        "shaders/particle_pull.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/particle_pull.wgsl".into()
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
}
pub type ParticleMaterial = ExtendedMaterial<StandardMaterial, ParticlePull>;

#[derive(Resource)]
pub struct ParticleVertices {
    pub buffer: Handle<ShaderBuffer>,
    /// Capacity mesh `k` holds `1 << k` quads.
    pub meshes: Vec<Handle<Mesh>>,
    /// This frame's packed vertices, uploaded as one prefix write.
    bytes: Vec<u8>,
    overflow_logged: bool,
}
impl FromWorld for ParticleVertices {
    fn from_world(world: &mut World) -> Self {
        let buffer = world
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(ShaderBuffer::with_size(
                VERTEX_CAPACITY * VERTEX_BYTES,
                RenderAssetUsages::default(),
            ));
        let mut assets = world.resource_mut::<Assets<Mesh>>();
        let meshes = (0..=MAX_QUAD_BITS)
            .map(|k| assets.add(capacity_mesh(1 << k)))
            .collect();
        Self {
            buffer,
            meshes,
            bytes: Vec::with_capacity(VERTEX_CAPACITY * VERTEX_BYTES / 8),
            overflow_logged: false,
        }
    }
}
/// `uv.x` carries the local vertex index. Positions stay at the origin: Bevy
/// sorts transparent draws by the mesh asset's AABB centre, so a zero-centred
/// mesh lets each batch's `Transform` (its particle-cloud centre, as the old
/// rewritten meshes had) decide the blend order. UV and colour attributes keep
/// the StandardMaterial shader defs (`VERTEX_UVS_A`, `VERTEX_COLORS`).
fn capacity_mesh(quads: usize) -> Mesh {
    let count = quads * 4;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0f32; 3]; count]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        (0..count).map(|i| [i as f32, 0.]).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0f32; 4]; count]);
    mesh.insert_indices(Indices::U32(
        (0..quads as u32)
            .flat_map(|q| {
                let b = q * 4;
                [b, b + 1, b + 2, b, b + 2, b + 3]
            })
            .collect(),
    ));
    mesh
}
impl ParticleVertices {
    pub fn clear(&mut self) {
        self.bytes.clear();
    }
    /// Hands this frame's packed vertices to the render world as one write.
    pub fn flush(&mut self, writes: &mut crate::gpu_writes::GpuWrites) {
        let capacity = self.bytes.capacity();
        let bytes = std::mem::replace(&mut self.bytes, Vec::with_capacity(capacity));
        writes.write(self.buffer.id(), 0, bytes);
    }
    /// Packs one batch's quads and returns (capacity mesh, MeshTag value), or
    /// None when the shared buffer is full this frame.
    pub fn push(
        &mut self,
        positions: &[[f32; 3]],
        uv: &[[f32; 2]],
        colors: &[[f32; 4]],
    ) -> Option<(Handle<Mesh>, u32)> {
        self.push_animated(positions, uv, colors, None)
    }
    pub fn push_animated(
        &mut self,
        positions: &[[f32; 3]],
        uv: &[[f32; 2]],
        colors: &[[f32; 4]],
        animation: Option<(&[[f32; 2]], &[[f32; 3]], &[[f32; 4]])>,
    ) -> Option<(Handle<Mesh>, u32)> {
        let quads = (positions.len() / 4).min(1 << MAX_QUAD_BITS);
        if quads == 0 {
            return None;
        }
        let first = self.bytes.len() / VERTEX_BYTES;
        if first + quads * 4 > VERTEX_CAPACITY {
            if !self.overflow_logged {
                self.overflow_logged = true;
                eprintln!("Particle vertex buffer full; hiding further batches this frame");
            }
            return None;
        }
        for i in 0..quads * 4 {
            let p = positions[i];
            let t = uv[i];
            let c = colors[i];
            let (next, blend, second) = animation.map_or(
                (t, [0., 0., 1.], [t[0], t[1], t[0], t[1]]),
                |(next, blend, second)| (next[i], blend[i], second[i]),
            );
            for v in [
                p[0], p[1], p[2], 0., t[0], t[1], next[0], next[1], c[0], c[1], c[2], c[3],
                blend[0], blend[1], blend[2], 0., second[0], second[1], second[2], second[3],
            ] {
                self.bytes.extend_from_slice(&f32::to_le_bytes(v));
            }
        }
        let bits = quads.next_power_of_two().trailing_zeros() as usize;
        // Low 16 bits: first quad; high 16 bits: live quads.
        Some((
            self.meshes[bits].clone(),
            (first / 4) as u32 | ((quads as u32) << 16),
        ))
    }
}

pub fn install(app: &mut App) {
    app.add_plugins((
        MaterialPlugin::<ParticleMaterial>::default(),
        crate::gpu_writes::GpuWritesPlugin,
    ))
    .init_resource::<ParticleVertices>();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batches_pack_contiguously_and_pick_power_of_two_capacity_meshes() {
        let mut assets = Assets::<Mesh>::default();
        let meshes = (0..=MAX_QUAD_BITS)
            .map(|k| assets.add(capacity_mesh(1 << k)))
            .collect::<Vec<_>>();
        let mut v = ParticleVertices {
            buffer: Handle::default(),
            meshes: meshes.clone(),
            bytes: vec![],
            overflow_logged: false,
        };
        let quad = |n: usize| {
            (
                vec![[1f32; 3]; n * 4],
                vec![[0.5f32; 2]; n * 4],
                vec![[1f32; 4]; n * 4],
            )
        };
        let (p, t, c) = quad(3);
        let (mesh, tag) = v.push(&p, &t, &c).unwrap();
        assert_eq!(mesh, meshes[2]); // 3 quads -> 4-quad mesh
        assert_eq!(tag, 3 << 16);
        let (p, t, c) = quad(1);
        let (mesh, tag) = v.push(&p, &t, &c).unwrap();
        assert_eq!(mesh, meshes[0]);
        assert_eq!(tag, 3 | (1 << 16)); // starts after the first batch
        assert_eq!(v.bytes.len(), 16 * VERTEX_BYTES);
        assert!(v.push(&[], &[], &[]).is_none());
        // Full buffer: further batches are refused, not wrapped.
        v.bytes.resize(VERTEX_CAPACITY * VERTEX_BYTES, 0);
        assert!(v.push(&p, &t, &c).is_none());
        v.clear();
        assert!(v.push(&p, &t, &c).is_some());
        v.clear();
        let next = vec![[0.25, 0.75]; 4];
        let blends = vec![[0.3, 0.7, 1.]; 4];
        let secondary = vec![[0.1, 0.2, 0.8, 0.9]; 4];
        v.push_animated(&p, &t, &c, Some((&next, &blends, &secondary)))
            .unwrap();
        let floats = v.bytes[..VERTEX_BYTES]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(&floats[6..8], &[0.25, 0.75]);
        assert_eq!(&floats[12..15], &[0.3, 0.7, 1.]);
        assert_eq!(&floats[16..20], &[0.1, 0.2, 0.8, 0.9]);
    }
    #[test]
    fn capacity_mesh_indexes_local_vertices_per_quad() {
        let mesh = capacity_mesh(2);
        assert_eq!(mesh.count_vertices(), 8);
        // Zero-centred so the batch Transform alone sets the sort position.
        assert_eq!(
            bevy::camera::primitives::MeshAabb::compute_aabb(&mesh).map(|a| Vec3::from(a.center)),
            Some(Vec3::ZERO)
        );
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("u32 indices");
        };
        assert_eq!(indices, &vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }
}
