//! Owned CBreakableSurface pane grid, hit selection, support and collision.
//! Pane collision and owned crack-edge masks; flying shards remain separate.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};
use l4d2r_game_core::{
    entities::{DamageKind, EntEvent, EntityWorld},
    random::UniformRandomStream,
};
use l4d2r_math::Vec3 as SourceVec;
use std::{collections::HashMap, sync::Arc};
#[derive(Clone)]
pub struct Surface {
    pub crack_material: Option<String>,
    pub initialized: bool,
    pub revision: u64,
    pub wide: usize,
    pub high: usize,
    corner: SourceVec,
    u: SourceVec,
    v: SourceVec,
    normal: SourceVec,
    width: f32,
    height: f32,
    depths: (f32, f32),
    support: [f32; 256],
    fragility: f32,
    tile: bool,
    think: bool,
    edge_styles: [u8; 1024],
    edge_rng: UniformRandomStream,
    rng: UniformRandomStream,
    pub shapes: Arc<Vec<l4d2_formats::phy::Convex>>,
    pub(crate) released: Vec<SourceVec>,
}
impl Surface {
    fn new(world: &EntityWorld, id: usize) -> Option<Self> {
        let e = &world.ents[id];
        if e.me.classname != "func_breakable_surf" || e.me.kv_i32("error").unwrap_or(0) != 0 {
            return None;
        }
        let point = |key| {
            e.me.kv(key)
                .map(|s| SourceVec::from_array(crate::kv::vector(Some(&s.to_string()))))
        };
        let (ll, ul, lr, ur) = (
            point("lowerleft")?,
            point("upperleft")?,
            point("lowerright")?,
            point("upperright")?,
        );
        let normal = (ur - ul).cross(ll - ul).normalize_or_zero();
        if normal.length_squared() < 0.9 {
            return None;
        }
        let forward = -normal;
        let u = if forward.truncate().length_squared() > 1e-8 {
            SourceVec::new(forward.y, -forward.x, 0.).normalize()
        } else {
            SourceVec::NEG_Y
        };
        let v = u.cross(forward).normalize();
        let corners = [ll, ul, lr, ur];
        let min_u = corners
            .iter()
            .map(|p| p.dot(u))
            .fold(f32::INFINITY, f32::min);
        let min_v = corners
            .iter()
            .map(|p| p.dot(v))
            .fold(f32::INFINITY, f32::min);
        let width = corners
            .iter()
            .map(|p| p.dot(u))
            .fold(f32::NEG_INFINITY, f32::max)
            - min_u;
        let height = corners
            .iter()
            .map(|p| p.dot(v))
            .fold(f32::NEG_INFINITY, f32::max)
            - min_v;
        let corner = u * min_u + v * min_v + normal * ll.dot(normal);
        // 100e0ed0: truncate span/12, then cap each dimension to 16.
        if width < 12.
            || height < 12.
            || corners.iter().any(|p| ((*p - ll).dot(normal)).abs() > 0.1)
        {
            return None;
        }
        let wide = ((width / 12.) as usize).min(16);
        let high = ((height / 12.) as usize).min(16);
        let mut depths = (0f32, 0f32);
        // Retain the authored brush's physical thickness, including sloped skylights.
        if let Some(model) = e.me.brush.and_then(|i| world.bsp.models.get(i)) {
            for face in world
                .bsp
                .faces
                .get(model.first_face as usize..(model.first_face + model.num_faces) as usize)?
            {
                for edge in world.bsp.surfedges.get(
                    face.first_edge as usize..face.first_edge as usize + face.num_edges as usize,
                )? {
                    let endpoints = world.bsp.edges.get(edge.unsigned_abs() as usize)?;
                    let p = world
                        .bsp
                        .vertexes
                        .get(endpoints[usize::from(*edge < 0)] as usize)?;
                    let d = (*p - corner).dot(normal);
                    depths.0 = depths.0.min(d);
                    depths.1 = depths.1.max(d);
                }
            }
        }
        if depths.1 - depths.0 < 0.01 {
            depths = (-0.01, 0.01);
        }
        let mut s = Self {
            crack_material: None,
            initialized: false,
            revision: 0,
            wide,
            high,
            corner,
            u,
            v,
            normal,
            width: width / wide as f32,
            height: height / high as f32,
            depths,
            support: [1.; 256],
            fragility: e
                .me
                .kv_f32("fragility")
                .filter(|v| v.is_finite())
                .unwrap_or(100.)
                .clamp(0., 100.),
            tile: e.me.kv_i32("surfacetype") == Some(1),
            think: false,
            edge_styles: [255; 1024],
            edge_rng: UniformRandomStream::new(1),
            rng: UniformRandomStream::new(1),
            shapes: Arc::new(vec![]),
            released: vec![],
        };
        s.rebuild();
        Some(s)
    }
    pub fn coordinates(&self, p: SourceVec) -> [f32; 2] {
        let d = p - self.corner;
        [d.dot(self.u) / self.width, d.dot(self.v) / self.height]
    }
    fn intact(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && x < self.wide as i32
            && y < self.high as i32
            && self.support[x as usize * 16 + y as usize] >= 0.
    }

    fn edge_families(&self, x: i32, y: i32) -> [Option<u8>; 4] {
        if self.intact(x, y) {
            return [None; 4];
        }
        let solid = |dx, dy| {
            let (a, b) = (x + dx, y + dy);
            a < 0 || b < 0 || a >= self.wide as i32 || b >= self.high as i32 || self.intact(a, b)
        };
        let (up, down, left, right) = (solid(0, 1), solid(0, -1), solid(-1, 0), solid(1, 0));
        let family = |edge: bool, a: bool, b: bool, first: u8, second: u8| {
            edge.then_some(match (a, b) {
                (true, true) => 1,
                (true, false) => first,
                (false, true) => second,
                _ => 0,
            })
        };
        [
            family(down, !left && solid(-1, -1), !right && solid(1, -1), 3, 2),
            family(right, !up && solid(1, 1), !down && solid(1, -1), 2, 3),
            family(up, !left && solid(-1, 1), !right && solid(1, 1), 2, 3),
            family(left, !up && solid(-1, 1), !down && solid(-1, -1), 3, 2),
        ]
    }
    fn remove(&mut self, x: i32, y: i32) -> bool {
        if !self.intact(x, y) {
            return false;
        }
        self.support[x as usize * 16 + y as usize] = -1.;
        if !self.tile {
            self.released.push(self.corner + self.u * ((x as f32 + 0.5) * self.width)
                + self.v * ((y as f32 + 0.5) * self.height));
        }
        self.think = true;
        self.revision += 1;
        true
    }
    fn clear(&mut self) {
        for x in 0..self.wide {
            for y in 0..self.high {
                self.remove(x as i32, y as i32);
            }
        }
    }
    fn hit(&mut self, p: SourceVec, kind: DamageKind) {
        self.initialized = true;
        let [x, y] = self.coordinates(p);
        let (ix, iy) = (x as i32, y as i32);
        match kind {
            DamageKind::Bullet | DamageKind::Club => {
                self.remove(ix, iy);
                if !self.tile {
                    let fx = x - ix as f32;
                    let fy = y - iy as f32;
                    if fx > 0.8 && ix != self.wide as i32 - 1 {
                        self.remove(ix + 1, iy);
                    } else if fx < 0.2 && ix != 0 {
                        self.remove(ix - 1, iy);
                    }
                    if fy > 0.8 && iy != self.high as i32 - 1 {
                        self.remove(ix, iy + 1);
                    } else if fy < 0.2 && iy != 0 {
                        self.remove(ix, iy - 1);
                    }
                    if self.rng.random_int(0, 1) == 0 {
                        self.remove(ix, iy + 1);
                        if self.rng.random_int(0, 1) == 0 {
                            self.remove(ix, iy + 2);
                        }
                    }
                }
            }
            DamageKind::Blast => {
                if self.tile {
                    for dx in -4i32..4 {
                        for dy in -4i32..4 {
                            if dx.abs() + dy.abs() < self.rng.random_int(2, 5) {
                                self.remove(ix + dx, iy + dy);
                            }
                        }
                    }
                } else {
                    self.clear();
                }
            }
            DamageKind::Burn => {}
        }
        self.revision += 1;
        self.rebuild();
    }
    fn shatter(&mut self, p: SourceVec) {
        self.initialized = true;
        let (x, y) = (p.x * self.wide as f32, p.y * self.high as f32);
        let radius = p.z.max(0.);
        let x0 = ((x - radius / self.width) as i32).max(0);
        let x1 = ((x + radius / self.width) as i32 + 1).min(self.wide as i32);
        let y0 = ((y - radius / self.height) as i32).max(0);
        let y1 = ((y + radius / self.height) as i32 + 1).min(self.high as i32);
        for i in x0..x1 {
            for j in y0..y1 {
                // Native InputShatter tests pane centers against the requested Source-unit radius.
                let dx = (i as f32 + 0.5) * self.width - x * self.width;
                let dy = (j as f32 + 0.5) * self.width - y * self.height;
                if dx * dx + dy * dy <= radius * radius {
                    self.remove(i, j);
                }
            }
        }
        self.revision += 1;
        self.rebuild();
    }
    fn support_step(&mut self) -> bool {
        if !self.think || self.tile {
            self.think = false;
            return false;
        }
        self.think = false;
        let previous = self.support;
        let sample = |x: i32, y: i32| previous[x as usize * 16 + y as usize].max(0.);
        let mut broken = vec![];
        for x in 0..self.wide as i32 {
            for y in 0..self.high as i32 {
                if !self.intact(x, y) {
                    continue;
                }
                // Owned100df0c0: upper/lower edge asymmetry and diagonal weights.
                let up = if y == self.high as i32 - 1 {
                    1.01
                } else {
                    sample(x, y + 1) + 0.01
                };
                let down = if y == 0 {
                    1.25
                } else {
                    sample(x, y - 1) * 1.25
                };
                let left = if x == 0 { 1. } else { sample(x - 1, y) };
                let right = if x == self.wide as i32 - 1 {
                    1.
                } else {
                    sample(x + 1, y)
                };
                let dl = if y == 0 || x == 0 {
                    1.
                } else {
                    sample(x - 1, y - 1)
                };
                let dr = if y == 0 || x == self.wide as i32 - 1 {
                    1.
                } else {
                    sample(x + 1, y - 1)
                };
                let ul = if y == self.high as i32 - 1 || x == 0 {
                    0.25
                } else {
                    sample(x - 1, y + 1) * 0.25
                };
                let ur = if y == self.high as i32 - 1 || x == self.wide as i32 - 1 {
                    0.25
                } else {
                    sample(x + 1, y + 1) * 0.25
                };
                let s = ((up + down + left + right + dl + dr + ul + ur) as f64 * (4. / 27.)) as f32;
                self.support[x as usize * 16 + y as usize] = s;
                if s < (self.fragility as f64 * 0.01 * 0.2) as f32 {
                    broken.push((x, y));
                }
            }
        }
        for &(x, y) in &broken {
            self.remove(x, y);
        }
        if !broken.is_empty() {
            self.rebuild();
        }
        !broken.is_empty()
    }
    fn rebuild(&mut self) {
        for x in 0..self.wide {
            for y in 0..self.high {
                let families = self.edge_families(x as i32, y as i32);
                for rotation in 0..4 {
                    let slot = (x * 16 + y) * 4 + rotation;
                    if families[rotation].is_none() {
                        self.edge_styles[slot] = 255;
                    } else if self.edge_styles[slot] == 255 {
                        self.edge_styles[slot] = self.edge_rng.random_int(0, 2) as u8;
                    }
                }
            }
        }

        let mut shapes = vec![];
        for x in 0..self.wide {
            for y in 0..self.high {
                if !self.intact(x as i32, y as i32) {
                    continue;
                }
                let base = self.corner
                    + self.u * (x as f32 * self.width)
                    + self.v * (y as f32 * self.height);
                let axes = [self.u, self.v, self.normal];
                let spans = [(0., self.width), (0., self.height), self.depths];
                let points = (0..8)
                    .map(|i| {
                        base + axes[0] * if i & 1 == 0 { spans[0].0 } else { spans[0].1 }
                            + axes[1] * if i & 2 == 0 { spans[1].0 } else { spans[1].1 }
                            + axes[2] * if i & 4 == 0 { spans[2].0 } else { spans[2].1 }
                    })
                    .collect::<Vec<_>>();
                let mut planes = vec![];
                for axis in axes {
                    for n in [axis, -axis] {
                        planes.push((
                            n,
                            points
                                .iter()
                                .map(|p| p.dot(n))
                                .fold(f32::NEG_INFINITY, f32::max),
                        ));
                    }
                }
                let mut edges = vec![];
                for i in 0..8 {
                    for bit in [1, 2, 4] {
                        if i & bit == 0 {
                            edges.push((points[i], points[i | bit]));
                        }
                    }
                }
                let mut bevels = vec![];
                for &(a, b) in &edges {
                    for axis in [SourceVec::X, SourceVec::Y, SourceVec::Z] {
                        let n = (b - a).cross(axis).normalize_or_zero();
                        if n.length_squared() < 0.9 {
                            continue;
                        }
                        for n in [n, -n] {
                            if planes
                                .iter()
                                .chain(bevels.iter())
                                .any(|(old, _)| n.dot(*old) > 0.99999)
                            {
                                continue;
                            }
                            bevels.push((
                                n,
                                points
                                    .iter()
                                    .map(|p| p.dot(n))
                                    .fold(f32::NEG_INFINITY, f32::max),
                            ));
                        }
                    }
                }
                let mins = points
                    .iter()
                    .copied()
                    .fold(SourceVec::splat(f32::INFINITY), SourceVec::min);
                let maxs = points
                    .iter()
                    .copied()
                    .fold(SourceVec::splat(f32::NEG_INFINITY), SourceVec::max);
                shapes.push(l4d2_formats::phy::Convex {
                    topology: None,
                    triangle_headers: vec![l4d2_formats::phy::TriangleHeader::default();12],
                    triangles: [
                        [0, 4, 6],
                        [0, 6, 2],
                        [1, 3, 7],
                        [1, 7, 5],
                        [0, 1, 5],
                        [0, 5, 4],
                        [2, 6, 7],
                        [2, 7, 3],
                        [0, 2, 3],
                        [0, 3, 1],
                        [4, 5, 7],
                        [4, 7, 6],
                    ]
                    .map(|i| [points[i[0]], points[i[1]], points[i[2]]])
                    .to_vec(),
                    planes,
                    edges,
                    bevels,
                    mins,
                    maxs,
                });
            }
        }
        self.shapes = Arc::new(shapes);
    }
    fn reset(&mut self) {
        self.edge_styles = [255; 1024];
        self.edge_rng = UniformRandomStream::new(1);
        self.initialized = false;
        self.revision += 1;
        self.support = [1.; 256];
        self.think = false;
        self.rng = UniformRandomStream::new(1);
        self.rebuild();
    }
}
pub fn load(world: &EntityWorld) -> HashMap<usize, Surface> {
    (0..world.ents.len())
        .filter_map(|id| Surface::new(world, id).map(|s| (id, s)))
        .collect()
}
pub fn prepare_materials(
    root: &std::path::Path,
    scene: &mut crate::scene::Scene,
    logic: &mut crate::l4d2r_cinematic::MapLogic,
) -> std::io::Result<()> {
    let mut assets =
        crate::source_assets::SourceAssets::new(root, logic.world.bsp.pakfile().to_vec())?;
    if !logic.break_surfaces.is_empty() {
        for prefix in ["glass", "tile"] {
            if !logic
                .break_surfaces
                .values()
                .any(|state| state.tile == (prefix == "tile"))
            {
                continue;
            }
            for family in 1..=3 {
                for style in ['a', 'b', 'c', 'd'] {
                    let path = format!("models/broken{prefix}/{prefix}broken_{family:02}{style}");
                    if let Err(error) = scene.add_material(&mut assets, &path) {
                        scene
                            .report
                            .push(format!("Breakable edge mask {path}: {error}"));
                    }
                }
            }
        }
    }
    for (id, state) in &mut logic.break_surfaces {
        let Some(model) = logic.world.ents[*id].me.brush else {
            continue;
        };
        let names = scene
            .world
            .surfaces
            .iter()
            .filter(|s| s.model == model)
            .map(|s| s.material.clone())
            .collect::<Vec<_>>();
        for name in names {
            if let Ok(Some(crack)) = crate::material::crack_material(&mut assets, &name) {
                match scene.add_material(&mut assets, &crack) {
                    Ok(()) => {
                        state.crack_material = Some(crack);
                        break;
                    }
                    Err(error) => scene
                        .report
                        .push(format!("Breakable crack material {crack}: {error}")),
                }
            }
        }
    }
    Ok(())
}
pub fn reset(logic: &mut crate::l4d2r_cinematic::MapLogic) {
    for state in logic.break_surfaces.values_mut() {
        state.reset();
    }
}
fn started(logic: &mut crate::l4d2r_cinematic::MapLogic, id: usize, first: bool) {
    if first {
        //100e0ed0: initial fracture zeroes parent health; pane entities remain.
        if let Some(d) = logic.world.durability.get_mut(&id) {
            d.health = 0.;
        }
        if let l4d2r_game_core::entities::Kind::Breakable { health } =
            &mut logic.world.ents[id].kind
        {
            *health = 0.;
        }
        logic.world.fire_output_pub(id, "OnBreak");
        logic.world.events.push(EntEvent::Broken { ent: id });
    }
}
pub fn damage_at(
    logic: &mut crate::l4d2r_cinematic::MapLogic,
    id: usize,
    damage: f32,
    kind: DamageKind,
    point: Vec3,
) -> bool {
    if !damage.is_finite()
        || damage <= 0.
        || !logic.world.ents[id].alive
        || logic.world.ents[id].me.kv_i32("material") == Some(7)
    {
        return false;
    }
    let Some(state) = logic.break_surfaces.get_mut(&id) else {
        return false;
    };
    let local = logic
        .world
        .world_matrix(id)
        .inverse()
        .transform_point3(SourceVec::new(point.x, -point.z, point.y) / crate::collision::UNIT);
    let first = !state.initialized;
    state.hit(local, kind);
    // TraceAttack subtracts integer raw damage; pane state survives zero health.
    let mut changed_health = None;
    if let Some(d) = logic.world.durability.get_mut(&id) {
        d.health = (d.health as i32 - damage as i32) as f32;
        changed_health = Some(d.health);
        if let l4d2r_game_core::entities::Kind::Breakable { health } =
            &mut logic.world.ents[id].kind
        {
            *health = d.health;
        }
    }
    if let Some(health) = changed_health {
        logic
            .world
            .fire_output_value(id, "OnHealthChanged", Some(&health.to_string()), None);
    }
    started(logic, id, first);
    true
}
pub fn update(logic: &mut crate::l4d2r_cinematic::MapLogic) -> bool {
    let events = logic
        .world
        .events
        .iter()
        .filter_map(|e| match e {
            EntEvent::SurfaceDamage { ent, damage, kind } => Some((*ent, None, *damage, *kind)),
            EntEvent::SurfaceShatter { ent, point_radius } => {
                Some((*ent, Some(*point_radius), 0., DamageKind::Bullet))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut changed = false;
    for (id, shatter, damage, kind) in events {
        if let Some(p) = shatter {
            if let Some(s) = logic.break_surfaces.get_mut(&id) {
                let first = !s.initialized;
                s.shatter(p);
                started(logic, id, first);
                changed = true;
            }
        } else if let Some(s) = logic.break_surfaces.get(&id) {
            let center = s.corner
                + s.u * (s.width * s.wide as f32 * 0.5)
                + s.v * (s.height * s.high as f32 * 0.5);
            let point = logic.world.world_matrix(id).transform_point3(center);
            changed |= damage_at(
                logic,
                id,
                damage,
                kind,
                Vec3::from_array(crate::binary::source_position(point.to_array())),
            );
        }
    }
    for s in logic.break_surfaces.values_mut() {
        changed |= s.support_step();
    }
    changed
}

// Independent pane-mask UV: material UV0 and the lightmap UV1 stay unchanged.
pub const GLASS_MASK_UV: bevy::mesh::MeshVertexAttribute = bevy::mesh::MeshVertexAttribute::new(
    "SourceGlassMaskUv",
    91342,
    bevy::render::render_resource::VertexFormat::Float32x2,
);
const MASK_SIZE: usize = 128;
const MASK_PAD: usize = 2;
const MASK_STRIDE: usize = MASK_SIZE + MASK_PAD * 2;
const MASK_ATLAS: usize = MASK_STRIDE * 4;
fn mask_uv(p: Vec2, rotation: usize, slot: usize) -> [f32; 2] {
    let p = match rotation {
        0 => Vec2::ONE - p,
        1 => Vec2::new(1. - p.y, p.x),
        2 => p,
        _ => Vec2::new(p.y, 1. - p.x),
    };
    [
        (slot % 4 * MASK_STRIDE + MASK_PAD) as f32 / MASK_ATLAS as f32
            + p.x * MASK_SIZE as f32 / MASK_ATLAS as f32,
        (slot / 4 * MASK_STRIDE + MASK_PAD) as f32 / MASK_ATLAS as f32
            + p.y * MASK_SIZE as f32 / MASK_ATLAS as f32,
    ]
}
/// Native family 0..3 selects artwork a..d; style 0..2 selects 03,02,01.
/// Padding prevents another family's alpha bleeding across a pane edge.
pub fn mask_atlas(
    textures: &HashMap<String, crate::vtf::Texture>,
    prefix: &str,
) -> Option<crate::vtf::Texture> {
    let mut rgba = vec![0; MASK_ATLAS * MASK_ATLAS * 4];
    for slot in 0..13 {
        let texture = if slot == 12 {
            None
        } else {
            let path = format!(
                "materials/models/broken{prefix}/{prefix}broken_{:02}{}.vtf",
                3 - slot % 3,
                (b'a' + (slot / 3) as u8) as char
            );
            Some(textures.get(&path)?)
        };
        for y in 0..MASK_STRIDE {
            for x in 0..MASK_STRIDE {
                let alpha = texture.map_or(255, |t| {
                    let sx = (x as isize - MASK_PAD as isize).clamp(0, (MASK_SIZE - 1) as isize)
                        as usize
                        * t.width as usize
                        / MASK_SIZE;
                    let sy = (y as isize - MASK_PAD as isize).clamp(0, (MASK_SIZE - 1) as isize)
                        as usize
                        * t.height as usize
                        / MASK_SIZE;
                    t.rgba[(sy * t.width as usize + sx) * 4 + 3]
                });
                let at =
                    ((slot / 4 * MASK_STRIDE + y) * MASK_ATLAS + slot % 4 * MASK_STRIDE + x) * 4;
                rgba[at..at + 4].copy_from_slice(&[255, 255, 255, alpha]);
            }
        }
    }
    Some(crate::vtf::Texture {
        width: MASK_ATLAS as u32,
        height: MASK_ATLAS as u32,
        mips: vec![rgba.clone()],
        rgba,
        clamp: [true; 2],
        no_mip: true,
        full: [MASK_ATLAS as u32; 2],
        blocks: None,
    })
}
#[derive(Resource, Default)]
pub struct GlassMasks {
    pub glass: Option<Handle<Image>>,
    pub tile: Option<Handle<Image>>,
    variants: HashMap<
        (AssetId<crate::viewer::SourceMaterial>, bool),
        Handle<crate::viewer::SourceMaterial>,
    >,
}
impl GlassMasks {
    fn available(&self, tile: bool) -> bool {
        if tile {
            self.tile.is_some()
        } else {
            self.glass.is_some()
        }
    }
    fn material(
        &mut self,
        base: &Handle<crate::viewer::SourceMaterial>,
        tile: bool,
        materials: &mut Assets<crate::viewer::SourceMaterial>,
    ) -> Handle<crate::viewer::SourceMaterial> {
        let mask = if tile {
            self.tile.clone()
        } else {
            self.glass.clone()
        };
        if mask.is_none() {
            return base.clone();
        }
        self.variants
            .entry((base.id(), tile))
            .or_insert_with(|| {
                let mut variant = materials.get(base).expect("loaded crack material").clone();
                variant.extension.glass_mask = mask;
                materials.add(variant)
            })
            .clone()
    }
}
#[derive(Component)]
pub struct OriginalMesh {
    mesh: Mesh,
    revision: u64,
    material: Handle<crate::viewer::SourceMaterial>,
}
// Clip triangles in pane coordinates, carrying barycentric weights for every attribute.
#[cfg(test)]
fn clipped(original: &Mesh, surface: &Surface) -> Option<Mesh> {
    clipped_mode(original, surface, false)
}
fn clipped_mode(original: &Mesh, surface: &Surface, masked: bool) -> Option<Mesh> {
    let VertexAttributeValues::Float32x3(positions) =
        original.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let indices = original
        .indices()
        .map(|i| i.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| (0..positions.len()).collect());
    let mut samples = Vec::<([usize; 3], Vec3, [f32; 2])>::new();
    for tri in indices.chunks_exact(3) {
        let ids = [tri[0], tri[1], tri[2]];
        let uv = ids.map(|i| {
            let p = Vec3::from_array(positions[i]);
            Vec2::from_array(
                surface.coordinates(SourceVec::new(p.x, -p.z, p.y) / crate::collision::UNIT),
            )
        });
        for x in 0..surface.wide {
            for y in 0..surface.high {
                let solid = surface.intact(x as i32, y as i32);
                let mut layers = vec![];
                if solid {
                    layers.push((12, 0));
                } else if masked {
                    for (rotation, family) in surface
                        .edge_families(x as i32, y as i32)
                        .into_iter()
                        .enumerate()
                    {
                        if let Some(family) = family {
                            let style = surface.edge_styles[(x * 16 + y) * 4 + rotation].min(2);
                            layers.push((family as usize * 3 + style as usize, rotation));
                        }
                    }
                }
                if layers.is_empty() {
                    continue;
                }
                let mut polygon = vec![(uv[0], Vec3::X), (uv[1], Vec3::Y), (uv[2], Vec3::Z)];
                for (axis, bound, sign) in [
                    (0, x as f32, 1.),
                    (0, x as f32 + 1., -1.),
                    (1, y as f32, 1.),
                    (1, y as f32 + 1., -1.),
                ] {
                    let mut next = vec![];
                    for i in 0..polygon.len() {
                        let (a, wa) = polygon[i];
                        let (b, wb) = polygon[(i + 1) % polygon.len()];
                        let da = (a[axis] - bound) * sign;
                        let db = (b[axis] - bound) * sign;
                        if da >= 0. {
                            next.push((a, wa));
                        }
                        if (da >= 0.) != (db >= 0.) {
                            let t = da / (da - db);
                            next.push((a.lerp(b, t), wa.lerp(wb, t)));
                        }
                    }
                    polygon = next;
                    if polygon.len() < 3 {
                        break;
                    }
                }
                for i in 1..polygon.len().saturating_sub(1) {
                    let a = polygon[i].0 - polygon[0].0;
                    let b = polygon[i + 1].0 - polygon[0].0;
                    if (a.x * b.y - a.y * b.x).abs() < 1e-8 {
                        continue;
                    }
                    for &(slot, rotation) in &layers {
                        for (point, weight) in [polygon[0], polygon[i], polygon[i + 1]] {
                            let local = point - Vec2::new(x as f32, y as f32);
                            samples.push((ids, weight, mask_uv(local, rotation, slot)));
                        }
                    }
                }
            }
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    for (attribute, values) in original.attributes() {
        macro_rules! interpolate {
            ($data:expr,$count:expr,$variant:ident) => {{
                let data = $data;
                VertexAttributeValues::$variant(
                    samples
                        .iter()
                        .map(|(ids, w, _)| {
                            let mut value = [0.; $count];
                            for k in 0..$count {
                                value[k] = data[ids[0]][k] * w.x
                                    + data[ids[1]][k] * w.y
                                    + data[ids[2]][k] * w.z;
                            }
                            value
                        })
                        .collect(),
                )
            }};
        }
        let result = match values {
            VertexAttributeValues::Float32x2(v) => interpolate!(v, 2, Float32x2),
            VertexAttributeValues::Float32x3(v) => interpolate!(v, 3, Float32x3),
            VertexAttributeValues::Float32x4(v) => interpolate!(v, 4, Float32x4),
            _ => return None,
        };
        mesh.insert_attribute(attribute.clone(), result);
    }
    if masked {
        mesh.insert_attribute(
            GLASS_MASK_UV,
            samples.iter().map(|(_, _, uv)| *uv).collect::<Vec<_>>(),
        );
    }
    Some(mesh)
}
pub fn presentation(
    mut commands: Commands,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    mut meshes: ResMut<Assets<Mesh>>,
    materials: Res<crate::viewer::SourceMaterials>,
    mut masks: ResMut<GlassMasks>,
    mut source_materials: ResMut<Assets<crate::viewer::SourceMaterial>>,
    mut draws: Query<(
        Entity,
        &crate::l4d2r_cinematic::MapEntity,
        &mut Mesh3d,
        &mut MeshMaterial3d<crate::viewer::SourceMaterial>,
        Option<&mut OriginalMesh>,
    )>,
) {
    for (entity, owner, mut draw, mut material, cache) in &mut draws {
        let Some(state) = logic.break_surfaces.get(&owner.0) else {
            continue;
        };
        if let Some(mut cache) = cache {
            if !state.initialized {
                material.0 = cache.material.clone();
                if let Some(mut mesh) = meshes.get_mut(&draw.0) {
                    *mesh = cache.mesh.clone();
                }
                commands.entity(entity).remove::<OriginalMesh>();
            } else if cache.revision != state.revision {
                if let Some(mesh) = clipped_mode(
                    &cache.mesh,
                    state,
                    state
                        .crack_material
                        .as_ref()
                        .is_some_and(|p| materials.0.contains_key(p))
                        && masks.available(state.tile),
                ) {
                    if let Some(mut target) = meshes.get_mut(&draw.0) {
                        *target = mesh;
                    }
                    cache.revision = state.revision;
                }
            }
        } else if state.initialized {
            let Some(original) = meshes.get(&draw.0).cloned() else {
                continue;
            };
            if let Some(mesh) = clipped_mode(
                &original,
                state,
                state
                    .crack_material
                    .as_ref()
                    .is_some_and(|p| materials.0.contains_key(p))
                    && masks.available(state.tile),
            ) {
                let original_material = material.0.clone();
                if let Some(crack) = state
                    .crack_material
                    .as_ref()
                    .and_then(|p| materials.0.get(p))
                {
                    material.0 = masks.material(crack, state.tile, &mut source_materials);
                }
                if std::env::args().any(|a| a == "--capture-game") {
                    println!("Fractured surface draw {}: mask={} vertices={} crack={:?} loaded={}",owner.0,mesh.attribute(GLASS_MASK_UV).is_some(),mesh.count_vertices(),state.crack_material,state.crack_material.as_ref().is_some_and(|p|materials.0.contains_key(p)));
                }
                draw.0 = meshes.add(mesh);
                commands.entity(entity).insert(OriginalMesh {
                    mesh: original,
                    revision: state.revision,
                    material: original_material,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_render_preparation_connects_the_actual_sugarmill_window_material_and_masks() {
        let Some((mut logic,id))=owned() else {return;};
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path=root.join("left4dead2/maps/c4m2_sugarmill_a.bsp");
        let world=crate::bsp::load_file(&path,root).unwrap();
        let mut scene=crate::scene::Scene::load(root,world).unwrap();
        prepare_materials(root,&mut scene,&mut logic).unwrap();
        let state=&logic.break_surfaces[&id];
        assert!(state.crack_material.is_some(),"model {:?}, materials {:?}, diagnostics {:?}",logic.world.ents[id].me.brush,scene.world.surfaces.iter().filter(|s|Some(s.model)==logic.world.ents[id].me.brush).map(|s|&s.material).collect::<Vec<_>>(),scene.report.iter().filter(|s|s.contains("crack")).collect::<Vec<_>>());
        let crack=&scene.materials[state.crack_material.as_ref().unwrap()];
        assert_eq!(crack.base.as_deref(),Some("materials/glass/offwndwb_break_hdr.vtf"));
        assert!(mask_atlas(&scene.textures,"glass").is_some());
    }
    #[test]
    fn native_edge_families_follow_holes_diagonals_and_window_frame() {
        let Some((logic, id)) = owned() else {
            return;
        };
        let mut s = logic.break_surfaces[&id].clone();
        assert_eq!(s.edge_families(3, 2), [None; 4]);
        s.remove(3, 2);
        assert_eq!(s.edge_families(3, 2), [Some(0); 4]);
        s.remove(2, 2);
        assert_eq!(s.edge_families(3, 2), [Some(3), Some(0), Some(2), None]);
        s.remove(4, 2);
        assert_eq!(s.edge_families(3, 2), [Some(1), None, Some(1), None]);
        s.rebuild();
        let styles = s.edge_styles;
        assert!(styles[(3 * 16 + 2) * 4] < 3);
        s.rebuild();
        assert_eq!(
            s.edge_styles, styles,
            "unchanged edges must not flicker between styles"
        );
        s.clear();
        assert_eq!(s.edge_families(3, 2), [None; 4]);
        assert_eq!(
            s.edge_families(0, 0),
            [Some(2), None, None, Some(3)],
            "the frame supports jagged perimeter edges"
        );
        s.reset();
        assert_eq!(s.edge_styles, [255; 1024]);
        assert_eq!(s.edge_families(3, 2), [None; 4]);
    }
    #[test]
    fn mask_atlas_uses_native_family_style_order_and_isolates_tile_borders() {
        let mut textures = HashMap::new();
        for family in 0..4 {
            for style in 0..3 {
                let alpha = (family * 3 + style + 1) as u8;
                textures.insert(
                    format!(
                        "materials/models/brokenglass/glassbroken_{:02}{}.vtf",
                        3 - style,
                        (b'a' + family as u8) as char
                    ),
                    crate::vtf::Texture {
                        width: 1,
                        height: 1,
                        rgba: vec![10, 20, 30, alpha],
                        mips: vec![],
                        clamp: [true; 2],
                        no_mip: true,
                        full: [1, 1],
                        blocks: None,
                    },
                );
            }
        }
        let atlas = mask_atlas(&textures, "glass").unwrap();
        assert!(atlas.no_mip);
        assert_eq!(
            atlas.mips.len(),
            1,
            "the GPU upload requires a populated base mip"
        );
        assert_eq!(atlas.mips[0], atlas.rgba);
        for slot in 0..13 {
            let expected = if slot == 12 { 255 } else { (slot + 1) as u8 };
            for (x, y) in [
                (0, 0),
                (MASK_PAD, MASK_PAD),
                (MASK_STRIDE - 1, MASK_STRIDE - 1),
            ] {
                let at =
                    ((slot / 4 * MASK_STRIDE + y) * MASK_ATLAS + slot % 4 * MASK_STRIDE + x) * 4;
                assert_eq!(atlas.rgba[at + 3], expected);
            }
        }
        textures.remove("materials/models/brokenglass/glassbroken_02b.vtf");
        assert!(
            mask_atlas(&textures, "glass").is_none(),
            "missing masks must retain the previous render path"
        );
        let corners = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        let expected = [
            [[1., 1.], [0., 1.], [0., 0.], [1., 0.]],
            [[1., 0.], [1., 1.], [0., 1.], [0., 0.]],
            [[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
            [[0., 1.], [0., 0.], [1., 0.], [1., 1.]],
        ];
        for rotation in 0..4 {
            for (corner, p) in corners.into_iter().enumerate() {
                let uv = mask_uv(p, rotation, 0);
                for k in 0..2 {
                    let local = (uv[k] * MASK_ATLAS as f32 - MASK_PAD as f32) / MASK_SIZE as f32;
                    assert!((local - expected[rotation][corner][k]).abs() < 1e-6);
                }
            }
        }
    }
    #[test]
    fn owned_glass_material_selects_authored_crack_replacement() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        assert_eq!(
            crate::material::crack_material(&mut assets, "glass/glasswindowbreak070a")
                .unwrap()
                .as_deref(),
            Some("glass/glasswindowbreak070b")
        );
        assert!(crate::material::load(&mut assets, "glass/glasswindowbreak070b").is_ok());
        let Some((logic, _)) = owned() else {
            return;
        };
        let mut embedded =
            crate::source_assets::SourceAssets::new(root, logic.world.bsp.pakfile().to_vec())
                .unwrap();
        let crack = crate::material::crack_material(
            &mut embedded,
            "maps/c4m2_sugarmill_a/glass/urban_glass_03_969_-5910_191",
        )
        .unwrap()
        .expect("embedded material's inherited crack replacement missing");
        assert!(crate::material::load(&mut embedded, &crack).is_ok());
    }
    #[test]
    fn infected_pounding_uses_the_contact_pane_instead_of_the_window_center() {
        let Some((mut logic, id)) = owned() else {
            return;
        };
        let s = &logic.break_surfaces[&id];
        let local = s.corner + s.u * (s.width * 0.5) + s.v * (s.height * 0.5);
        let matrix = logic.world.world_matrix(id);
        let p = Vec3::from_array(crate::binary::source_position(
            matrix.transform_point3(local).to_array(),
        ));
        let out = Vec3::from_array(crate::binary::source_normal(
            matrix.transform_vector3(s.normal).to_array(),
        ));
        assert!(logic.pound_obstacle(
            p + out * 0.5 - Vec3::Y,
            -out,
            &crate::collision::Collision::default()
        ));
        assert!(!logic.break_surfaces[&id].intact(0, 0));
        assert!(
            logic.break_surfaces[&id].intact(3, 2),
            "pound used the generic damage center instead of its actual ray contact"
        );
    }
    fn owned() -> Option<(crate::l4d2r_cinematic::MapLogic, usize)> {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return None;
        }
        let logic = crate::l4d2r_cinematic::MapLogic::load(
            root,
            &root.join("left4dead2/maps/c4m2_sugarmill_a.bsp"),
            true,
        )
        .unwrap();
        let id = logic
            .world
            .ents
            .iter()
            .position(|e| e.me.kv("hammerid") == Some("1048806"))
            .unwrap();
        Some((logic, id))
    }
    #[test]
    fn owned_sugarmill_shatter_retains_other_panes_collision_and_restarts() {
        let Some((mut logic, id)) = owned() else {
            return;
        };
        assert_eq!(
            (
                logic.break_surfaces[&id].wide,
                logic.break_surfaces[&id].high
            ),
            (8, 6)
        );
        assert_eq!(logic.break_surfaces[&id].shapes.len(), 48);
        logic.world.events.clear();
        logic.world.input(id, "Shatter", "0.5 0.5 13", None, None);
        assert!(
            logic.world.ents[id].alive,
            "Shatter incorrectly destroyed the entire brush"
        );
        assert!(update(&mut logic));
        let s = &logic.break_surfaces[&id];
        assert!(s.initialized);
        assert_eq!(
            s.shapes.len(),
            44,
            "native circle must initially remove four pane centers"
        );
        assert_eq!(logic.world.durability[&id].health, 0.);
        assert_eq!(
            logic
                .world
                .events
                .iter()
                .filter(|e| matches!(e,EntEvent::Broken{ent} if *ent==id))
                .count(),
            1
        );
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let mut collision =
            crate::collision::Collision::load(&root.join("left4dead2/maps/c4m2_sugarmill_a.bsp"))
                .unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        for (x, y, blocked) in [(3.5, 2.5, false), (0.5, 0.5, true)] {
            let p = s.corner + s.u * (s.width * x) + s.v * (s.height * y);
            let convert = |p| {
                Vec3::from_array(crate::binary::source_position(
                    logic.world.world_matrix(id).transform_point3(p).to_array(),
                ))
            };
            let a = convert(p - s.normal * 8.);
            let b = convert(p + s.normal * 8.);
            let ray = b - a;
            assert_eq!(
                logic
                    .breakable_hit_distance(id, a, ray.normalize(), ray.length())
                    .is_some(),
                blocked
            );
            assert_eq!(
                collision.trace(a, b, Vec3::ZERO).fraction < 1.,
                blocked,
                "world collision disagrees with remaining panes"
            );
        }
        logic.world.events.clear();
        logic.world.input(id, "Shatter", "0.25 0.25 7", None, None);
        update(&mut logic);
        assert!(
            !logic
                .world
                .events
                .iter()
                .any(|e| matches!(e,EntEvent::Broken{ent} if *ent==id)),
            "OnBreak repeated after initialization"
        );
        logic.restart();
        assert!(!logic.break_surfaces[&id].initialized);
        assert_eq!(logic.break_surfaces[&id].shapes.len(), 48);
        assert!(logic.world.ents[id].alive);
    }
    #[test]
    fn native_support_drops_an_isolated_interior_pane_but_preserves_frame_support() {
        let Some((logic, id)) = owned() else {
            return;
        };
        let mut s = logic.break_surfaces[&id].clone();
        s.initialized = true;
        for x in 0..3 {
            for y in 0..3 {
                if (x, y) != (1, 1) {
                    s.remove(x, y);
                }
            }
        }
        assert!(s.intact(1, 1));
        assert!(s.support_step());
        assert!(!s.intact(1, 1));
        s.reset();
        for x in 0..s.wide as i32 {
            for y in 0..s.high as i32 {
                if (x, y) != (0, 0) {
                    s.remove(x, y);
                }
            }
        }
        s.support_step();
        assert!(
            s.intact(0, 0),
            "frame-connected corner lost its native boundary support"
        );
    }
    #[test]
    fn bullet_and_club_are_localized_while_blast_destroys_the_whole_glass_surface() {
        let Some((mut logic, id)) = owned() else {
            return;
        };
        let state = &logic.break_surfaces[&id];
        let p = state.corner + state.u * (state.width * 3.5) + state.v * (state.height * 2.5);
        let world = logic.world.world_matrix(id).transform_point3(p);
        let point = Vec3::from_array(crate::binary::source_position(world.to_array()));
        logic.world.ents[id].conns.push(
            l4d2_formats::vmf::Connection::parse("OnHealthChanged", "!self,FireUser1,,0,-1")
                .unwrap(),
        );
        assert!(damage_at(&mut logic, id, 1., DamageKind::Bullet, point));
        let s = &logic.break_surfaces[&id];
        assert!(s.initialized);
        assert!(!s.intact(3, 2));
        assert!((45..48).contains(&s.shapes.len()));
        assert!(logic.world.ents[id].alive);
        assert!(logic.world.events.iter().any(
            |e| matches!(e,EntEvent::Fired{src,output} if *src==id && output=="OnHealthChanged")
        ));
        logic.restart();
        assert!(damage_at(&mut logic, id, 60., DamageKind::Club, point));
        assert!((45..48).contains(&logic.break_surfaces[&id].shapes.len()));
        assert!(damage_at(&mut logic, id, 60., DamageKind::Blast, point));
        assert_eq!(logic.break_surfaces[&id].shapes.len(), 0);
        assert!(
            logic
                .breakable_hit_distance(id, point - Vec3::Z, Vec3::Z, 2.)
                .is_none(),
            "empty surface fell back to its original solid AABB"
        );
    }
    #[test]
    fn clipped_indexed_mesh_removes_exact_pane_area_and_preserves_material_and_lightmap_uvs() {
        let Some((logic, id)) = owned() else {
            return;
        };
        let mut s = logic.break_surfaces[&id].clone();
        s.initialized = true;
        s.remove(3, 2);
        let source = [
            s.corner,
            s.corner + s.u * (s.width * s.wide as f32),
            s.corner + s.u * (s.width * s.wide as f32) + s.v * (s.height * s.high as f32),
            s.corner + s.v * (s.height * s.high as f32),
        ];
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            source
                .map(|p| crate::binary::source_position(p.to_array()))
                .to_vec(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 0., 1.]; 4]);
        let uv = vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_UV_1,
            uv.iter()
                .map(|v| [v[0] * 2. + 1., v[1] * 2. + 1.])
                .collect::<Vec<_>>(),
        );
        mesh.insert_indices(bevy::mesh::Indices::U32(vec![0, 1, 2, 0, 2, 3]));
        let result = clipped(&mesh, &s).unwrap();
        let VertexAttributeValues::Float32x3(p) =
            result.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!()
        };
        let area = p
            .chunks_exact(3)
            .map(|p| {
                let a = Vec3::from_array(p[0]);
                let b = Vec3::from_array(p[1]);
                let c = Vec3::from_array(p[2]);
                (b - a).cross(c - a).length() * 0.5
            })
            .sum::<f32>();
        let expected =
            (s.wide * s.high - 1) as f32 * s.width * s.height * crate::collision::UNIT.powi(2);
        assert!(
            (area - expected).abs() < 1e-4,
            "clipped area {area}, expected {expected}"
        );
        let VertexAttributeValues::Float32x2(uv) = result.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
        else {
            panic!()
        };
        let VertexAttributeValues::Float32x2(light) =
            result.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
        else {
            panic!()
        };
        for (uv, light) in uv.iter().zip(light) {
            for k in 0..2 {
                assert!((uv[k] * 2. + 1. - light[k]).abs() < 1e-5);
            }
        }
        s.rebuild();
        let masked = clipped_mode(&mesh, &s, true).unwrap();
        assert!(
            masked.count_vertices() > result.count_vertices(),
            "jagged edges occupy the removed cell without restoring its collider"
        );
        let VertexAttributeValues::Float32x2(mask_uvs) = masked.attribute(GLASS_MASK_UV).unwrap()
        else {
            panic!()
        };
        assert_eq!(mask_uvs.len(), masked.count_vertices());
        assert!(
            mask_uvs
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        );
        let VertexAttributeValues::Float32x2(uv) = masked.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
        else {
            panic!()
        };
        let VertexAttributeValues::Float32x2(light) =
            masked.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
        else {
            panic!()
        };
        for (uv, light) in uv.iter().zip(light) {
            for k in 0..2 {
                assert!((uv[k] * 2. + 1. - light[k]).abs() < 1e-5);
            }
        }
        s.clear();
        assert_eq!(clipped(&mesh, &s).unwrap().count_vertices(), 0);
    }
    #[test]
    fn owned_sloped_skylight_and_subway_panes_produce_finite_outward_contact_geometry() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        for (map, count) in [
            ("left4dead2/maps/c5m1_waterfront.bsp", 2),
            ("left4dead2_dlc2/maps/c8m2_subway.bsp", 14),
        ] {
            let logic =
                crate::l4d2r_cinematic::MapLogic::load(root, &root.join(map), true).unwrap();
            assert_eq!(
                logic.break_surfaces.len(),
                count,
                "missing native pane surface in {map}"
            );
            for state in logic.break_surfaces.values() {
                assert!((1..=16).contains(&state.wide) && (1..=16).contains(&state.high));
                for shape in &*state.shapes {
                    assert!(shape.mins.is_finite() && shape.maxs.is_finite());
                    assert_eq!(shape.triangles.len(), 12);
                    let center = (shape.mins + shape.maxs) * 0.5;
                    for t in &shape.triangles {
                        let normal = (t[1] - t[0]).cross(t[2] - t[0]);
                        assert!(
                            normal.dot((t[0] + t[1] + t[2]) / 3. - center) > 0.,
                            "inward/degenerate contact triangle in {map}"
                        );
                    }
                }
            }
        }
    }
}
