//! Map ropes and cables: `move_rope` / `keyframe_rope` chains (c1m2's power
//! lines between the wooden posts). Reconstructed from the public Source SDK
//! 2013 rope code (CRopeKeyframe, CBaseRopePhysics, CSimplePhysics,
//! C_RopeKeyframe):
//!
//! * a keyframe links to its `NextKey`; `Type` 0/1/other gives 10/4/2 nodes
//!   (ROPE_MAX_SEGMENTS, ROPE_TYPE1/2_NUMSEGMENTS); `Dangling` 1 frees the end;
//! * every spring rests at (span + Slack + ROPESLACK_FUDGEFACTOR -100) /
//!   (nodes - 1) and is corrected only when stretched (a rope constraint),
//!   3 iterations per step, endpoints relocked after each iteration;
//! * Verlet at 1/50 s with energy 0.98 and ROPE_GRAVITY (0, 0, -1500);
//! * drawn as a camera-facing strip, Catmull-Rom subdivided `Subdiv` times
//!   (255 = rope_subdiv 2), `Width` units wide, texture V advancing
//!   4 / TextureScale texels per unit; nodes lit from the map lighting with
//!   the brightest ambient-cube face, clamped to 1 (rope_averagelight 0).
//!
//! Not reproduced: wind (`UseWind`/rope_wind_dist gusts), `Collide`,
//! `Breakable`, impulses, ropes attached to moving entities or attachments,
//! barbed ropes, the Cable shader's bump lighting, and native fog on ropes.
//! The anti-aliased screen-space minimum width is approximated by widening
//! sub-pixel ropes to one pixel with reduced alpha.
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

const UNIT: f32 = 0.0254;
const ROPE_MAX_SEGMENTS: usize = 10;
const ROPE_TYPE1_NUMSEGMENTS: usize = 4;
const ROPE_TYPE2_NUMSEGMENTS: usize = 2;
const ROPESLACK_FUDGEFACTOR: f32 = -100.;
const ROPE_GRAVITY: f32 = -1500.;
const TIME_STEP: f32 = 1. / 50.;
const ENERGY: f32 = 0.98;
const ITERATIONS: usize = 3;
/// rope_subdiv default, used for `Subdiv` 255.
const DEFAULT_SUBDIV: usize = 2;
/// CalcRopeStartingConditions settles a hanging rope with 3 s of simulation.
const PREWARM: f32 = 3.;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rope {
    pub nodes: Vec<Vec3>,
    previous: Vec<Vec3>,
    pub start: Vec3,
    /// `None`: dangling end.
    pub end: Option<Vec3>,
    /// Spring rest length (metres).
    pub spring: f32,
    pub width: f32,
    pub subdiv: usize,
    pub texture_scale: f32,
    pub material: String,
    light: Vec<Vec3>,
}

/// Ropes from a map's entity list: one per keyframe whose `NextKey` exists.
pub(crate) fn from_entities(entities: &[std::collections::BTreeMap<String, String>]) -> Vec<Rope> {
    // Map entity keys arrive lower-cased (kv::entities).
    let entities: Vec<std::collections::BTreeMap<String, String>> = entities
        .iter()
        .map(|e| e.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect())
        .collect();
    let origin = |e: &std::collections::BTreeMap<String, String>| {
        let v: Vec<f32> = e
            .get("origin")?
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        (v.len() == 3).then(|| Vec3::new(v[0], v[2], -v[1]) * UNIT)
    };
    let is_rope = |e: &std::collections::BTreeMap<String, String>| {
        e.get("classname")
            .is_some_and(|c| c.eq_ignore_ascii_case("move_rope") || c.eq_ignore_ascii_case("keyframe_rope"))
    };
    let mut named: HashMap<String, Vec3> = HashMap::new();
    for e in entities.iter().filter(|e| is_rope(e)) {
        if let (Some(name), Some(p)) = (e.get("targetname"), origin(e)) {
            named.entry(name.to_ascii_lowercase()).or_insert(p);
        }
    }
    let number = |e: &std::collections::BTreeMap<String, String>, key: &str, default: f32| {
        e.get(key)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    };
    let mut ropes = vec![];
    for e in entities.iter().filter(|e| is_rope(e)) {
        let Some(start) = origin(e) else { continue };
        let next = e
            .get("nextkey")
            .filter(|n| !n.is_empty())
            .and_then(|n| named.get(&n.to_ascii_lowercase()))
            .copied();
        let dangling = number(e, "dangling", 0.) == 1.;
        // Without a next keyframe and with the end locked the rope does not
        // simulate or draw anything.
        let Some(target) = next else { continue };
        let count = match number(e, "type", 0.) as i32 {
            0 => ROPE_MAX_SEGMENTS,
            1 => ROPE_TYPE1_NUMSEGMENTS,
            _ => ROPE_TYPE2_NUMSEGMENTS,
        };
        let span = start.distance(target) / UNIT;
        let length = (span.trunc() + number(e, "slack", 0.).trunc() + ROPESLACK_FUDGEFACTOR).max(0.);
        let subdiv = match number(e, "subdiv", 2.) as i32 {
            255 => DEFAULT_SUBDIV,
            s => s.clamp(0, 7) as usize,
        };
        let mut material = e.get("ropematerial").cloned().unwrap_or_else(|| "cable/cable.vmt".into());
        if !material.to_ascii_lowercase().contains(".vmt") {
            material.push_str(".vmt");
        }
        let nodes: Vec<Vec3> = (0..count)
            .map(|i| start.lerp(target, i as f32 / (count - 1) as f32))
            .collect();
        ropes.push(Rope {
            previous: nodes.clone(),
            nodes,
            start,
            end: (!dangling).then_some(target),
            spring: length / (count - 1) as f32 * UNIT,
            width: number(e, "width", 2.).max(0.) * UNIT,
            subdiv,
            texture_scale: number(e, "texturescale", 4.).max(0.01),
            material,
            light: vec![Vec3::ONE; count],
        });
    }
    ropes
}

impl Rope {
    /// One CSimplePhysics step plus CBaseRopePhysics::ApplyConstraints.
    pub(crate) fn step(&mut self) {
        let acceleration = Vec3::Y * ROPE_GRAVITY * UNIT;
        let multiplier = TIME_STEP * TIME_STEP * 0.5;
        for (p, previous) in self.nodes.iter_mut().zip(self.previous.iter_mut()) {
            let current = *p;
            *p = *p + (*p - *previous) * ENERGY + acceleration * multiplier;
            *previous = current;
        }
        let spring_squared = self.spring * self.spring;
        for _ in 0..ITERATIONS {
            for i in 0..self.nodes.len() - 1 {
                let to = self.nodes[i] - self.nodes[i + 1];
                let distance_squared = to.length_squared();
                if distance_squared > spring_squared {
                    let correction = to * (1. - self.spring / distance_squared.sqrt());
                    self.nodes[i] -= correction * 0.5;
                    self.nodes[i + 1] += correction * 0.5;
                }
            }
            // Delegate: endpoints stay locked to their keyframes.
            self.nodes[0] = self.start;
            self.previous[0] = self.start;
            if let Some(end) = self.end {
                let last = self.nodes.len() - 1;
                self.nodes[last] = end;
                self.previous[last] = end;
            }
        }
    }
    /// Rendered centre line (Catmull-Rom through the nodes) and its lighting.
    fn line(&self) -> (Vec<Vec3>, Vec<Vec3>) {
        let points = catmull_rom(&self.nodes, self.subdiv);
        let light = catmull_rom(&self.light, self.subdiv);
        (points, light)
    }
}

/// C_RopeKeyframe's spline: each span between nodes `i` and `i + 1` gains
/// `subdiv` Catmull-Rom points at t = (k + 1) / (subdiv + 1); the outer
/// control points repeat the ends.
pub(crate) fn catmull_rom(points: &[Vec3], subdiv: usize) -> Vec<Vec3> {
    if points.len() < 2 || subdiv == 0 {
        return points.to_vec();
    }
    let n = points.len();
    let mut out = Vec::with_capacity(n + (n - 1) * subdiv);
    for i in 0..n - 1 {
        let p1 = points[i.saturating_sub(1)];
        let p2 = points[i];
        let p3 = points[i + 1];
        let p4 = points[(i + 2).min(n - 1)];
        out.push(p2);
        for k in 0..subdiv {
            let t = (k + 1) as f32 / (subdiv + 1) as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * ((2. * p2)
                    + (-p1 + p3) * t
                    + (2. * p1 - 5. * p2 + 4. * p3 - p4) * t2
                    + (-p1 + 3. * p2 - 3. * p3 + p4) * t3),
            );
        }
    }
    out.push(points[n - 1]);
    out
}

/// Camera-facing strip for one rope, appended to shared buffers.
#[allow(clippy::too_many_arguments)]
fn strip(
    points: &[Vec3],
    light: &[Vec3],
    width: f32,
    texels_per_metre: f32,
    camera: Vec3,
    pixel: f32,
    out: &mut Buffers,
) {
    let mut v = 0.;
    let first = out.positions.len() as u32;
    for (k, &p) in points.iter().enumerate() {
        let tangent = if k + 1 < points.len() {
            points[k + 1] - p
        } else {
            p - points[k - 1]
        }
        .normalize_or_zero();
        if k > 0 {
            v += p.distance(points[k - 1]) * texels_per_metre;
        }
        let view = camera - p;
        let mut side = tangent.cross(view.normalize_or_zero()).normalize_or_zero();
        if side == Vec3::ZERO {
            side = tangent.any_orthonormal_vector();
        }
        // Sub-pixel ropes: one pixel wide, faded by the coverage they lost.
        let minimum = view.length() * pixel;
        let (drawn, alpha) = if width < minimum {
            (minimum, (width / minimum).max(0.3))
        } else {
            (width, 1.)
        };
        let half = side * drawn * 0.5;
        let c = light.get(k).copied().unwrap_or(Vec3::ONE);
        for (offset, u) in [(-half, 0.), (half, 1.)] {
            out.positions.push((p + offset).to_array());
            out.normals.push(view.normalize_or_zero().to_array());
            out.uvs.push([u, v]);
            out.colors.push([c.x, c.y, c.z, alpha]);
        }
        if k > 0 {
            let b = first + (k as u32 - 1) * 2;
            out.indices.extend([b, b + 1, b + 2, b + 1, b + 3, b + 2]);
        }
    }
}
#[derive(Default)]
struct Buffers {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}
impl Buffers {
    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Node lighting: ComputeLighting without a normal, approximated by the
/// brightest face of the ambient cube (local lights included) plus the
/// visible sun, clamped to 1.
fn node_light(world: &crate::bsp::World, shadows: &crate::lighting::WorldShadow, p: Vec3) -> Vec3 {
    let cube = crate::viewer::model_ambient(world, shadows, p);
    let mut light = cube
        .iter()
        .map(|c| Vec3::from_array(*c))
        .max_by(|a, b| a.max_element().total_cmp(&b.max_element()))
        .unwrap_or(Vec3::splat(0.5));
    if let Some(sun) = world.suns.first() {
        let ray = Vec3::from_array(sun.direction).normalize_or_zero();
        if !shadows.blocked(p.to_array(), ray.to_array()) {
            light += Vec3::from_array(sun.intensity);
        }
    }
    let peak = light.max_element();
    if peak > 1. {
        light /= peak;
    }
    light
}

#[derive(Component)]
struct RopeDraw(String);

#[derive(Default)]
struct State {
    map: usize,
    ropes: Vec<Rope>,
    clock: f32,
    /// Material -> (StandardMaterial, texels per unit for TextureScale 1).
    materials: HashMap<String, (Handle<StandardMaterial>, f32)>,
}

pub fn install(app: &mut App) {
    app.add_systems(Update, update);
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    map: Option<Res<crate::viewer::Map>>,
    shadows: Option<Res<crate::lighting::WorldShadow>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    camera: Query<(&GlobalTransform, &Projection), With<crate::viewer::GameCamera>>,
    windows: Query<&Window>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    draws: Query<(Entity, &RopeDraw, &Mesh3d)>,
    mut state: Local<State>,
) {
    let _timing = crate::performance::scope("ropes::update");
    let (Some(map), Some(shadows)) = (map, shadows) else {
        return;
    };
    let world = &map.0.world;
    let id = world.entities.as_ptr() as usize ^ world.entities.len();
    if state.map != id {
        state.map = id;
        state.clock = 0.;
        state.ropes = from_entities(&world.entities);
        for rope in &mut state.ropes {
            for _ in 0..(PREWARM / TIME_STEP) as usize {
                rope.step();
            }
            rope.light = rope.nodes.iter().map(|p| node_light(world, &shadows, *p)).collect();
        }
        for (entity, _, _) in &draws {
            commands.entity(entity).despawn();
        }
        if !state.ropes.is_empty() {
            println!("Map ropes: {}", state.ropes.len());
        }
        return;
    }
    if state.ropes.is_empty() {
        return;
    }
    // The fixed 50 Hz rope step (endpoints are static, so this only matters
    // while a rope is still settling).
    state.clock += time.delta_secs().min(0.25);
    while state.clock >= TIME_STEP {
        state.clock -= TIME_STEP;
        for rope in &mut state.ropes {
            rope.step();
        }
    }
    let Some((camera, projection)) = camera.iter().next() else {
        return;
    };
    let height = windows.iter().next().map_or(1080., |w| w.resolution.height().max(1.));
    let fov = match projection {
        Projection::Perspective(p) => p.fov,
        _ => 1.2,
    };
    let pixel = 2. * (fov * 0.5).tan() / height;
    let eye = camera.translation();
    let mut buffers: HashMap<String, Buffers> = HashMap::new();
    let State { ropes, materials: cache, .. } = &mut *state;
    for rope in ropes.iter() {
        let key = rope.material.to_ascii_lowercase();
        if !cache.contains_key(&key) {
            let entry = rope_material(source.as_deref(), &key, &mut images, &mut materials);
            cache.insert(key.clone(), entry);
        }
        let per_unit = cache[&key].1;
        let (points, light) = rope.line();
        strip(
            &points,
            &light,
            rope.width,
            per_unit / rope.texture_scale / UNIT,
            eye,
            pixel,
            buffers.entry(key).or_default(),
        );
    }
    for (key, data) in buffers {
        let mesh = data.mesh();
        if let Some((_, _, handle)) = draws.iter().find(|(_, d, _)| d.0 == key) {
            if let Some(mut existing) = meshes.get_mut(&handle.0) {
                *existing = mesh;
            }
            continue;
        }
        commands.spawn((
            RopeDraw(key.clone()),
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(cache[&key].0.clone()),
            Transform::default(),
            NoFrustumCulling,
            RenderLayers::layer(1),
        ));
    }
}

/// The rope material's base texture, unlit and tinted by the per-node
/// lighting carried in the vertex colours. Returns the texels per unit at
/// TextureScale 1 (C_RopeKeyframe: 4 / TextureScale texels per inch over the
/// texture height).
fn rope_material(
    source: Option<&crate::game_ui::HudSource>,
    name: &str,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> (Handle<StandardMaterial>, f32) {
    let texture = source.and_then(|source| {
        let mut assets = crate::source_assets::SourceAssets::new(&source.0, vec![]).ok()?;
        let material = crate::material::load(&mut assets, name).ok()?;
        let base = material.base?;
        let path = format!(
            "materials/{}.vtf",
            base.replace('\\', "/").trim_start_matches("materials/").trim_end_matches(".vtf")
        );
        crate::vtf::decode(&assets.read(&path).ok()?, 512).ok()
    });
    let per_unit = texture.as_ref().map_or(1. / 64., |t| 4. / t.height.max(1) as f32);
    let image = texture.map(|t| {
        let mut image = Image::new(
            Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            t.rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::Repeat,
            ..ImageSamplerDescriptor::linear()
        });
        images.add(image)
    });
    if image.is_none() {
        eprintln!("Map ropes: no base texture for {name}; drawing untextured");
    }
    let handle = materials.add(StandardMaterial {
        base_color: if image.is_some() { Color::WHITE } else { Color::srgb(0.08, 0.08, 0.08) },
        base_color_texture: image,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    (handle, per_unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    #[test]
    fn keyframes_link_by_next_key_with_sdk_segments_and_slack() {
        let ropes = from_entities(&[
            entity(&[("classname", "move_rope"), ("targetname", "a"), ("origin", "0 0 100"), ("NextKey", "b"), ("Slack", "140"), ("Width", "1"), ("Subdiv", "2"), ("Type", "0")]),
            entity(&[("classname", "keyframe_rope"), ("targetname", "b"), ("origin", "300 0 100"), ("NextKey", "c"), ("Type", "1"), ("Dangling", "1")]),
            entity(&[("classname", "keyframe_rope"), ("targetname", "c"), ("origin", "300 0 0")]),
            entity(&[("classname", "keyframe_rope"), ("targetname", "lonely"), ("origin", "0 0 0")]),
        ]);
        assert_eq!(ropes.len(), 2);
        assert_eq!(ropes[0].nodes.len(), 10);
        // (300 + 140 - 100) / 9 units per spring.
        assert!((ropes[0].spring / UNIT - 340. / 9.).abs() < 1e-3);
        assert_eq!(ropes[1].nodes.len(), 4);
        assert!(ropes[1].end.is_none());
        assert_eq!(ropes[1].material, "cable/cable.vmt");
    }
    #[test]
    fn a_slack_rope_sags_under_gravity_and_never_stretches_past_its_length() {
        let mut rope = from_entities(&[
            entity(&[("classname", "move_rope"), ("targetname", "a"), ("origin", "0 0 0"), ("NextKey", "b"), ("Slack", "200")]),
            entity(&[("classname", "keyframe_rope"), ("targetname", "b"), ("origin", "400 0 0")]),
        ])
        .remove(0);
        for _ in 0..150 {
            rope.step();
        }
        let middle = rope.nodes[5];
        assert!(middle.y < -0.5, "sags: {middle:?}");
        assert_eq!(rope.nodes[0], rope.start);
        assert_eq!(rope.nodes[9], rope.end.unwrap());
        // Settled: almost still after the 3 s prewarm.
        let before = rope.nodes.clone();
        rope.step();
        let moved = rope.nodes.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0., f32::max);
        assert!(moved < 0.01, "moved {moved}");
        // Total length within the constraint (small residual stretch only).
        let length: f32 = rope.nodes.windows(2).map(|w| w[0].distance(w[1])).sum();
        assert!(length <= rope.spring * 9. * 1.05, "{length} vs {}", rope.spring * 9.);
    }
    #[test]
    fn a_taut_rope_stays_nearly_straight() {
        let mut rope = from_entities(&[
            entity(&[("classname", "move_rope"), ("targetname", "a"), ("origin", "0 0 0"), ("NextKey", "b"), ("Slack", "0")]),
            entity(&[("classname", "keyframe_rope"), ("targetname", "b"), ("origin", "400 0 0")]),
        ])
        .remove(0);
        for _ in 0..150 {
            rope.step();
        }
        // Slack 0 + fudge -100 is shorter than the span: springs only pull.
        assert!(rope.nodes[5].y > -0.4, "{:?}", rope.nodes[5]);
    }
    #[test]
    fn catmull_rom_passes_through_the_nodes_and_subdivides_each_span() {
        let nodes = [Vec3::ZERO, Vec3::X, Vec3::new(2., 1., 0.)];
        let out = catmull_rom(&nodes, 2);
        assert_eq!(out.len(), 3 + 2 * 2);
        assert_eq!(out[0], nodes[0]);
        assert_eq!(out[3], nodes[1]);
        assert_eq!(out[6], nodes[2]);
    }
    #[test]
    fn c1m2_power_lines_load_as_ropes() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        if !path.is_file() {
            return;
        }
        let world = crate::bsp::load_file(&path, root).unwrap();
        let ropes = from_entities(&world.entities);
        assert!(ropes.len() >= 50, "{}", ropes.len());
        assert!(ropes.iter().all(|r| r.material.eq_ignore_ascii_case("cable/cable.vmt")));
    }
}
