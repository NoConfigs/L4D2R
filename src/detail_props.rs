//! Detail sprites: the grass tufts and weeds vbsp scatters over terrain
//! (game lump `dprp`, worldspawn `detailmaterial`). None were drawn; c7m1's
//! dock yard alone has 5,039.
//!
//! Each sprite is drawn as two crossed quads (retail turns orientation-2
//! sprites about z toward the viewer; a fixed cross reads the same from any
//! side without per-frame work), alpha tested, lit by the baked light each
//! sprite carries through the vertex-lit Source material. Sprites are
//! batched into map cells that hide beyond retail's detail distance
//! (cl_detaildist 1200 + cl_detailfade 400 units). Not reproduced: the
//! distance fade itself, wind sway, detail models and shapes.
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::RenderLayers,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

const UNIT: f32 = crate::collision::UNIT;
/// Cell edge, Source units.
const CELL: f32 = 1024.;
/// Retail cl_detaildist + cl_detailfade.
const DRAW_DISTANCE: f32 = 1600.;

#[derive(Component)]
struct DetailCell {
    center: Vec3,
    radius: f32,
}

#[derive(Default)]
struct State {
    map: usize,
}

pub fn install(app: &mut App) {
    app.add_systems(Update, (spawn, cull));
}

/// The two crossed quads of one sprite: positions, uvs (4 + 4 vertices).
pub(crate) fn sprite_quads(s: &crate::bsp::DetailSprite) -> Vec<([f32; 3], [f32; 2])> {
    let origin = Vec3::from_array(s.origin);
    let yaw = s.yaw.to_radians();
    let [left, top, right, bottom] = s.rect;
    let [u0, v0, u1, v1] = s.uv;
    let mut out = Vec::with_capacity(8);
    for turn in [0., std::f32::consts::FRAC_PI_2] {
        // Source (cos, sin, 0) -> renderer (cos, 0, -sin).
        let side = Vec3::new((yaw + turn).cos(), 0., -(yaw + turn).sin());
        for (x, y, u, v) in [(left, top, u0, v0), (right, top, u1, v0), (right, bottom, u1, v1), (left, bottom, u0, v1)] {
            let p = origin + side * (x * UNIT) + Vec3::Y * (y * UNIT);
            out.push((p.to_array(), [u, v]));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    mut commands: Commands,
    map: Option<Res<crate::viewer::Map>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    table: Option<Res<crate::viewer::LightingTable>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    cells: Query<Entity, With<DetailCell>>,
    mut state: Local<State>,
) {
    let (Some(map), Some(source), Some(table)) = (map, source, table) else { return };
    let world = &map.0.world;
    let id = world.entities.as_ptr() as usize ^ world.entities.len();
    if state.map == id {
        return;
    }
    state.map = id;
    for entity in &cells {
        commands.entity(entity).try_despawn();
    }
    if world.detail_sprites.is_empty() {
        return;
    }
    let name = world
        .entities
        .iter()
        .find(|e| e.get("classname").is_some_and(|c| c == "worldspawn"))
        .and_then(|e| e.get("detailmaterial"))
        .map(|s| s.replace('\\', "/").to_ascii_lowercase())
        .unwrap_or_else(|| "detail/detailsprites".into());
    let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) else { return };
    let path = crate::material::load(&mut assets, &name)
        .ok()
        .and_then(|m| m.base)
        .unwrap_or_else(|| format!("materials/{name}.vtf"));
    let Some(texture) = assets.read(&path).ok().and_then(|b| crate::vtf::decode(&b, 1024).ok()) else {
        println!("Detail sprites: no texture {path}");
        return;
    };
    let image = images.add(Image::new(
        Extent3d { width: texture.width, height: texture.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        texture.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    let material = materials.add(crate::viewer::vertex_lit_material(
        StandardMaterial {
            base_color_texture: Some(image),
            alpha_mode: AlphaMode::Mask(0.4),
            cull_mode: None,
            double_sided: true,
            ..default()
        },
        &table,
    ));
    let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, s) in world.detail_sprites.iter().enumerate() {
        let key = ((s.origin[0] / (CELL * UNIT)).floor() as i32, (s.origin[2] / (CELL * UNIT)).floor() as i32);
        buckets.entry(key).or_default().push(i);
    }
    for (_, list) in buckets {
        let (mut positions, mut normals, mut uvs, mut colors, mut indices) = (vec![], vec![], vec![], vec![], vec![]);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for i in list {
            let s = &world.detail_sprites[i];
            let quads = sprite_quads(s);
            for quad in quads.chunks_exact(4) {
                let base = positions.len() as u32;
                for (p, uv) in quad {
                    lo = lo.min(Vec3::from_array(*p));
                    hi = hi.max(Vec3::from_array(*p));
                    positions.push(*p);
                    normals.push([0., 1., 0.]);
                    uvs.push(*uv);
                    colors.push([s.light[0], s.light[1], s.light[2], 1.]);
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        // Main-world copy kept: the render warmup repacks meshes after their
        // first upload, and a RENDER_WORLD-only mesh then never draws.
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));
        commands.spawn((
            DetailCell { center: (lo + hi) * 0.5, radius: (hi - lo).length() * 0.5 },
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(1),
            bevy::light::NotShadowCaster,
        ));
    }
    println!("Detail sprites: {} ({name})", world.detail_sprites.len());
}

fn cull(
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    mut cells: Query<(&DetailCell, &mut Visibility)>,
) {
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    for (cell, mut visibility) in &mut cells {
        let near = eye.distance(cell.center) - cell.radius < DRAW_DISTANCE * UNIT;
        visibility.set_if_neq(if near { Visibility::Inherited } else { Visibility::Hidden });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_sprite_is_two_crossed_upright_quads() {
        let s = crate::bsp::DetailSprite {
            origin: [1., 0., 2.],
            yaw: 0.,
            orientation: 2,
            rect: [-5., 10., 5., 0.],
            uv: [0.1, 0.2, 0.3, 0.4],
            light: [0.1; 3],
        };
        let q = sprite_quads(&s);
        assert_eq!(q.len(), 8);
        // Top-left of the first quad: 5 units left along Source +x, 10 up.
        assert!((q[0].0[0] - (1. - 5. * UNIT)).abs() < 1e-5 && (q[0].0[1] - 10. * UNIT).abs() < 1e-5);
        assert_eq!(q[0].1, [0.1, 0.2]);
        // The second quad lies along Source +y (renderer -z).
        assert!((q[4].0[2] - (2. + 5. * UNIT)).abs() < 1e-5 && (q[4].0[0] - 1.).abs() < 1e-5);
    }
}
