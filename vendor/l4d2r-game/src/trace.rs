//! Swept-box tracing against the BSP brush tree: a port of the engine's `CM_BoxTrace` (RecurseTree / TraceToLeaf /
//! ClipBoxToBrush / TestBoxInBrush), written from the public Source engine algorithm.
//! UNVERIFIED against engine.dll: the epsilon handling in `recurse` follows the Source SDK. Displacement collision is
//! approximated: each displacement triangle becomes a convex prism (front plane, 32-unit back plane, edge planes and
//! axis bevels) clipped like a brush; the engine's own `CDispCollTree` is not ported (tracked in docs/STATUS.md).

use glam::{Mat3, Mat4, Vec3};
use l4d2_formats::bsp::Bsp;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub const DIST_EPSILON: f32 = 0.03125;

// Contents flags (public/bspflags.h; same in L4D2).
pub const CONTENTS_SOLID: u32 = 0x1;
pub const CONTENTS_WINDOW: u32 = 0x2;
pub const CONTENTS_GRATE: u32 = 0x8;
pub const CONTENTS_WATER: u32 = 0x20;
pub const CONTENTS_BLOCKLOS: u32 = 0x40;
pub const CONTENTS_OPAQUE: u32 = 0x80;
pub const CONTENTS_MOVEABLE: u32 = 0x4000;
pub const CONTENTS_PLAYERCLIP: u32 = 0x10000;
pub const CONTENTS_MONSTERCLIP: u32 = 0x20000;
pub const CONTENTS_MONSTER: u32 = 0x2000000;
pub const CONTENTS_DEBRIS: u32 = 0x4000000;
pub const CONTENTS_HITBOX: u32 = 0x40000000;
pub const MASK_SOLID: u32 =
    CONTENTS_SOLID | CONTENTS_MOVEABLE | CONTENTS_WINDOW | CONTENTS_MONSTER | CONTENTS_GRATE;
pub const MASK_PLAYERSOLID: u32 = CONTENTS_SOLID
    | CONTENTS_MOVEABLE
    | CONTENTS_PLAYERCLIP
    | CONTENTS_WINDOW
    | CONTENTS_MONSTER
    | CONTENTS_GRATE;
pub const MASK_NPCSOLID: u32 = CONTENTS_SOLID
    | CONTENTS_MOVEABLE
    | CONTENTS_MONSTERCLIP
    | CONTENTS_WINDOW
    | CONTENTS_MONSTER
    | CONTENTS_GRATE;
pub const MASK_SHOT: u32 = CONTENTS_SOLID
    | CONTENTS_MOVEABLE
    | CONTENTS_MONSTER
    | CONTENTS_WINDOW
    | CONTENTS_DEBRIS
    | CONTENTS_HITBOX;

#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub start: Vec3,
    pub end_pos: Vec3,
    pub fraction: f32,
    pub normal: Vec3,
    pub plane_dist: f32,
    pub contents: u32,
    pub texinfo: Option<usize>,
    pub start_solid: bool,
    pub all_solid: bool,
    pub hit_brush: Option<usize>,
    pub hit_model: usize,
    pub hit_origin: Vec3,
}

impl Trace {
    fn new(start: Vec3, end: Vec3) -> Trace {
        Trace {
            start,
            end_pos: end,
            fraction: 1.0,
            normal: Vec3::ZERO,
            plane_dist: 0.0,
            contents: 0,
            texinfo: None,
            start_solid: false,
            all_solid: false,
            hit_brush: None,
            hit_model: 0,
            hit_origin: Vec3::ZERO,
        }
    }
    pub fn hit(&self) -> bool {
        self.fraction < 1.0
    }
}
/// An axial BSP contact face plus a scene-clear swept neighbourhood. Only valid
/// against the immutable collision snapshot used to certify it.
#[derive(Clone, Copy)]
pub struct ContactPatch {
    start: Vec3,
    end: Vec3,
    radius: Vec3,
    grown_dist: f32,
    template: Trace,
}
impl ContactPatch {
    pub fn trace(&self, end: Vec3) -> Option<Trace> {
        self.trace_from(self.start, end)
    }
    pub fn trace_from(&self, start: Vec3, end: Vec3) -> Option<Trace> {
        if !start.is_finite()
            || !end.is_finite()
            || !(start - self.start).abs().cmple(self.radius).all()
            || !(end - self.end).abs().cmple(self.radius).all()
        {
            return None;
        }
        let d1 = (start - self.template.hit_origin).dot(self.template.normal) - self.grown_dist;
        let d2 = (end - self.template.hit_origin).dot(self.template.normal) - self.grown_dist;
        if d1 <= 0. {
            return None;
        }
        // A BSP node can cull a near-miss before brush-plane epsilon clipping.
        // Reuse confirmed crossings only. Clear results must retain the full
        // traversal: a neighbouring boundary face may win near its epsilon.
        if d2 >= 0. {
            return None;
        }
        let mut hit = Trace::new(start, end);
        if d2 < d1 {
            let entry = (d1 - DIST_EPSILON).max(0.) / (d1 - d2);
            if entry > -1. && entry < 1. {
                let fraction = entry.max(0.);
                hit = self.template;
                hit.start = start;
                hit.fraction = fraction;
                hit.end_pos = start + (end - start) * fraction;
            }
        }
        Some(hit)
    }
}

struct Work<'a> {
    brush_planes: &'a [Vec<(Vec3, f32)>],
    brush_bounds: &'a [Option<(Vec3, Vec3)>],
    bsp: &'a Bsp,
    start: Vec3,
    end: Vec3,
    mins: Vec3,
    maxs: Vec3,
    extents: Vec3,
    is_point: bool,
    mask: u32,
    force_volume: bool,
    ignored_brush: Option<usize>,
    ignored_plane: Option<(Vec3, f32)>,
    trace: Trace,
    /// Brush visit stamps (a brush straddling several leaves is clipped once).
    visited: &'a mut Vec<u32>,
    stamp: u32,
}

/// Back-plane depth of a displacement triangle prism.
const DISP_DEPTH: f32 = 32.0;
const DISP_CELL: f32 = 128.0;

#[derive(Clone)]
struct DispTri {
    planes: Vec<(Vec3, f32)>,
    mins: Vec3,
    maxs: Vec3,
    contents: u32,
    /// Hammer "no hull collision": box traces (player/NPC movement) pass.
    no_hull: bool,
    /// Hammer "no ray collision": point traces (bullets, sight) pass.
    no_ray: bool,
}

#[derive(Clone)]
struct DispSet {
    tris: Vec<DispTri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl DispSet {
    fn build(bsp: &Bsp) -> DispSet {
        let mut tris = Vec::new();
        for (contents, flags, [a, b, c]) in bsp.displacement_triangles_with_flags() {
            let n = (b - a).cross(c - a);
            if n.length_squared() < 1e-6 {
                continue;
            }
            let n = n.normalize();
            let centroid = (a + b + c) / 3.0;
            let mut planes = vec![(n, n.dot(a)), (-n, -n.dot(a) + DISP_DEPTH)];
            let verts = [a, b, c];
            for i in 0..3 {
                let (p, q) = (verts[i], verts[(i + 1) % 3]);
                let e = q - p;
                let en = e.cross(n);
                if en.length_squared() > 1e-8 {
                    let en = en.normalize();
                    planes.push((en, en.dot(p)));
                }
                // Bevels against each world axis so axis-aligned boxes slide over edges correctly.
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    let bn = e.cross(axis);
                    if bn.length_squared() < 1e-8 {
                        continue;
                    }
                    let mut bn = bn.normalize();
                    // Orient away from the prism interior (centroid must be behind the plane).
                    if bn.dot(centroid) - bn.dot(p) > 0.0 {
                        bn = -bn;
                    }
                    planes.push((bn, bn.dot(p)));
                }
            }
            let mins = a.min(b).min(c) - n.abs() * DISP_DEPTH;
            let maxs = a.max(b).max(c) + n.abs() * 0.0;
            tris.push(DispTri {
                planes,
                mins: mins.min(a.min(b).min(c)),
                maxs,
                contents: contents as u32,
                no_hull: flags & 4 != 0,
                no_ray: flags & 8 != 0,
            });
        }
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let (x0, x1) = (
                (t.mins.x / DISP_CELL).floor() as i32,
                (t.maxs.x / DISP_CELL).floor() as i32,
            );
            let (y0, y1) = (
                (t.mins.y / DISP_CELL).floor() as i32,
                (t.maxs.y / DISP_CELL).floor() as i32,
            );
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        DispSet { tris, grid }
    }
}

/// What an extra collider is made of.
#[derive(Clone, Debug)]
pub enum ColliderShape {
    Convexes {
        shape: std::sync::Arc<Vec<l4d2_formats::phy::Convex>>,
        matrix: Mat4,
        contents: u32,
    },
    OrientedBox {
        mins: Vec3,
        maxs: Vec3,
        matrix: Mat4,
        contents: u32,
    },
    /// A brush-entity model at `origin` (no rotation).
    Model { model: usize, origin: Vec3 },
    /// A rotated brush-entity model (model space -> world). Traced in model
    /// space against the real brushes, with the hull grown to the model-space
    /// bounds of the rotated box, as the engine's transformed box trace does.
    RotatedModel { model: usize, matrix: Mat4 },
    /// Axis-aligned box with the given contents (stand-in for props whose `.phy` collision isn't parsed).
    Box {
        mins: Vec3,
        maxs: Vec3,
        contents: u32,
    },
}

/// Entity-driven collision (doors, breakables, prop boxes); toggled/moved by the entity layer.
#[derive(Clone, Debug)]
pub struct Collider {
    pub shape: ColliderShape,
    pub active: bool,
}

fn collider_unchanged(a: &Collider, b: &Collider) -> bool {
    if a.active != b.active {
        return false;
    }
    match (&a.shape, &b.shape) {
        (
            ColliderShape::Box {
                mins: a,
                maxs: b,
                contents: c,
            },
            ColliderShape::Box {
                mins: x,
                maxs: y,
                contents: z,
            },
        ) => a == x && b == y && c == z,
        (
            ColliderShape::Model {
                model: a,
                origin: b,
            },
            ColliderShape::Model {
                model: x,
                origin: y,
            },
        ) => a == x && b == y,
        (
            ColliderShape::RotatedModel { model: a, matrix: b },
            ColliderShape::RotatedModel { model: x, matrix: y },
        ) => a == x && b == y,
        (
            ColliderShape::OrientedBox {
                mins: a,
                maxs: b,
                matrix: c,
                contents: d,
            },
            ColliderShape::OrientedBox {
                mins: w,
                maxs: x,
                matrix: y,
                contents: z,
            },
        ) => a == w && b == x && c == y && d == z,
        (
            ColliderShape::Convexes {
                shape: a,
                matrix: b,
                contents: c,
            },
            ColliderShape::Convexes {
                shape: x,
                matrix: y,
                contents: z,
            },
        ) => std::sync::Arc::ptr_eq(a, x) && b == y && c == z,
        _ => false,
    }
}
/// Oriented box standing in for a prop's `.phy` collision model (UNVERIFIED approximation from the MDL hull).
#[derive(Clone, Debug)]
pub struct Obb {
    /// Convex pieces (model space) from the prop's `.phy`; when set the box is only the broad phase.
    pub shape: Option<std::sync::Arc<Vec<l4d2_formats::phy::Convex>>>,
    pub center: Vec3,
    pub half: Vec3,
    /// Local -> world rotation.
    pub rot: Mat3,
    pub contents: u32,
    /// Index of the owning map entity (`usize::MAX` for static props); lets a broken prop switch its collision off.
    pub owner: usize,
    pub enabled: Cell<bool>,
    mins: Vec3,
    maxs: Vec3,
    /// World-space convex pieces (face planes + edge bevels against the world axes), built once at placement.
    wconvex: std::sync::Arc<Vec<WConvex>>,
}

#[derive(Clone, Debug)]
struct WConvex {
    planes: std::sync::Arc<Vec<(Vec3, f32)>>,
    mins: Vec3,
    maxs: Vec3,
}

#[derive(Clone)]
struct PlacedConvexes {
    shape: std::sync::Arc<Vec<l4d2_formats::phy::Convex>>,
    matrix: Mat4,
    pieces: std::sync::Arc<Vec<WConvex>>,
}
fn prepare_convexes(collider: &Collider, cached: &mut Option<PlacedConvexes>) {
    let ColliderShape::Convexes { shape, matrix, .. } = &collider.shape else {
        *cached = None;
        return;
    };
    if cached
        .as_ref()
        .is_some_and(|c| std::sync::Arc::ptr_eq(&c.shape, shape) && c.matrix == *matrix)
    {
        return;
    }
    let rotation = Mat3::from_mat4(*matrix);
    let pieces = shape
        .iter()
        .map(|cv| {
            let mut placed = world_convex(cv, matrix.w_axis.truncate(), rotation);
            // Retain the same conservative broad-phase bounds used by live traces.
            let center = matrix.transform_point3((cv.mins + cv.maxs) * 0.5);
            let half = (cv.maxs - cv.mins) * 0.5;
            let extent = rotation.x_axis.abs() * half.x
                + rotation.y_axis.abs() * half.y
                + rotation.z_axis.abs() * half.z;
            placed.mins = center - extent;
            placed.maxs = center + extent;
            placed
        })
        .collect();
    *cached = Some(PlacedConvexes {
        shape: shape.clone(),
        matrix: *matrix,
        pieces: std::sync::Arc::new(pieces),
    });
}

/// Place a model-space convex in the world: rotated face planes plus bevel planes cross(edge, world axis) that are
/// supporting (every vertex behind them), which makes `plane.d + |n|.ext` the exact Minkowski growth for an axis-aligned hull.
fn world_convex(cv: &l4d2_formats::phy::Convex, origin: Vec3, rot: Mat3) -> WConvex {
    let mut planes: Vec<(Vec3, f32)> = cv
        .planes
        .iter()
        .map(|&(n, d)| {
            let nw = rot * n;
            (nw, d + nw.dot(origin))
        })
        .collect();
    let edges: Vec<(Vec3, Vec3)> = cv
        .edges
        .iter()
        .map(|&(a, b)| (origin + rot * a, origin + rot * b))
        .collect();
    let pts: Vec<Vec3> = edges.iter().flat_map(|&(a, b)| [a, b]).collect();
    let centroid = if pts.is_empty() {
        origin
    } else {
        pts.iter().fold(Vec3::ZERO, |a, p| a + *p) / pts.len() as f32
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let base = planes.len();
    for &(a, b) in &edges {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let bn = (b - a).cross(axis);
            if bn.length_squared() < 1e-6 {
                continue;
            }
            let mut bn = bn.normalize();
            if bn.dot(a - centroid) < 0.0 {
                bn = -bn;
            }
            let d = bn.dot(a);
            if pts.iter().any(|p| bn.dot(*p) > d + 0.02) {
                continue; // not a supporting plane
            }
            if planes
                .iter()
                .any(|(n2, d2)| n2.dot(bn) > 0.9999 && (d2 - d).abs() < 0.05)
            {
                continue;
            }
            planes.push((bn, d));
        }
    }
    let _ = base;
    WConvex {
        planes: std::sync::Arc::new(planes),
        mins: lo,
        maxs: hi,
    }
}

impl Obb {
    pub fn new(center: Vec3, half: Vec3, rot: Mat3, contents: u32) -> Obb {
        let ext = Vec3::new(
            rot.x_axis.x.abs() * half.x + rot.y_axis.x.abs() * half.y + rot.z_axis.x.abs() * half.z,
            rot.x_axis.y.abs() * half.x + rot.y_axis.y.abs() * half.y + rot.z_axis.y.abs() * half.z,
            rot.x_axis.z.abs() * half.x + rot.y_axis.z.abs() * half.y + rot.z_axis.z.abs() * half.z,
        );
        Obb {
            shape: None,
            owner: usize::MAX,
            enabled: Cell::new(true),
            center,
            half,
            rot,
            contents,
            mins: center - ext,
            maxs: center + ext,
            wconvex: std::sync::Arc::new(Vec::new()),
        }
    }

    /// A prop with a real collision model: `origin`/`rot` place model space, `lmins/lmaxs` bound the convexes.
    pub fn with_shape(
        origin: Vec3,
        rot: Mat3,
        lmins: Vec3,
        lmaxs: Vec3,
        shape: std::sync::Arc<Vec<l4d2_formats::phy::Convex>>,
        contents: u32,
    ) -> Obb {
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in 0..8 {
            let c = Vec3::new(
                if i & 1 == 0 { lmins.x } else { lmaxs.x },
                if i & 2 == 0 { lmins.y } else { lmaxs.y },
                if i & 4 == 0 { lmins.z } else { lmaxs.z },
            );
            let w = origin + rot * c;
            mn = mn.min(w);
            mx = mx.max(w);
        }
        let wconvex = shape.iter().map(|c| world_convex(c, origin, rot)).collect();
        Obb {
            shape: Some(shape),
            owner: usize::MAX,
            enabled: Cell::new(true),
            center: origin,
            half: (lmaxs - lmins) * 0.5,
            rot,
            contents,
            mins: mn,
            maxs: mx,
            wconvex: std::sync::Arc::new(wconvex),
        }
    }
}

/// Collision world: the BSP brush tree plus the displacement prisms and entity colliders; build once, trace many times.
#[derive(Clone)]
pub struct Tracer {
    ignored_brush: Cell<Option<usize>>,
    ignored_plane: Cell<Option<(Vec3, f32)>>,
    epoch: Cell<u64>,
    environment_epoch: Cell<u64>,
    brush_planes: std::sync::Arc<Vec<Vec<(Vec3, f32)>>>,
    brush_bounds: std::sync::Arc<Vec<Option<(Vec3, Vec3)>>>,
    bsp: std::sync::Arc<Bsp>,
    visited: RefCell<Vec<u32>>,
    stamp: Cell<u32>,
    disp: std::sync::Arc<DispSet>,
    disp_visited: RefCell<Vec<u32>>,
    colliders: RefCell<Vec<Collider>>,
    collider_owners: RefCell<Vec<usize>>,
    live_owners: RefCell<std::collections::HashSet<usize>>,
    collider_convexes: RefCell<Vec<Option<PlacedConvexes>>>,
    collider_grid: RefCell<HashMap<(i32, i32, i32), Vec<usize>>>,
    collider_global: RefCell<Vec<usize>>,
    collider_candidates_cache: RefCell<Vec<([i32; 6], std::sync::Arc<[usize]>)>>,
    obbs: Vec<Obb>,
    obb_grid: std::sync::Arc<HashMap<(i32, i32, i32), Vec<u32>>>,
    obb_candidates: RefCell<Vec<([i32; 6], std::sync::Arc<[u32]>)>>,
}

impl Tracer {
    pub fn certify_contact_patch(
        &self,
        start: Vec3,
        end: Vec3,
        half: Vec3,
        radius: f32,
        mask: u32,
    ) -> Option<ContactPatch> {
        let hit = self.box_trace(start, end, -half, half, mask, 0, Vec3::ZERO);
        let brush = hit.hit_brush?;
        if hit.hit_model != 0 && self.colliders.borrow().iter().filter(|c| c.active && matches!(c.shape, ColliderShape::Model { model, .. } if model == hit.hit_model)).count() != 1 { return None; }
        if hit.start_solid
            || hit.fraction >= 1.
            || ![Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z].contains(&hit.normal)
            || radius <= 0.
        {
            return None;
        }
        let planes = &self.brush_planes[brush];
        let axis = if hit.normal.x != 0. {
            0
        } else if hit.normal.y != 0. {
            1
        } else {
            2
        };
        let extent = half[axis];
        let radius = Vec3::splat(radius);

        let floor = planes
            .iter()
            .find(|(n, d)| *n == hit.normal && *d + extent == hit.plane_dist)?;
        let dist = floor.1;
        if (start - hit.hit_origin).dot(hit.normal) <= dist + extent {
            return None;
        }
        let lo = start.min(end) - hit.hit_origin - half - radius;
        let hi = start.max(end) - hit.hit_origin + half + radius;
        for &(n, d) in planes.iter() {
            if n == hit.normal && d == dist {
                continue;
            }
            let mut corner = Vec3::new(
                if n.x >= 0. { hi.x } else { lo.x },
                if n.y >= 0. { hi.y } else { lo.y },
                if n.z >= 0. { hi.z } else { lo.z },
            );
            let a = (dist - extent - DIST_EPSILON * 2.) * hit.normal[axis];
            let b = (dist + extent + DIST_EPSILON * 2.) * hit.normal[axis];
            corner[axis] = if n[axis] >= 0. { a.max(b) } else { a.min(b) };
            if corner.dot(n) > d {
                return None;
            }
        }
        // Reject the whole widened swept region if any OTHER world brush,
        // displacement, prop or active entity can intersect it.
        let previous = self.ignored_brush.replace(Some(brush));
        let previous_plane = self
            .ignored_plane
            .replace((hit.hit_model == 0).then_some(*floor));
        let clear = self.box_trace(
            start,
            end,
            -(half + radius),
            half + radius,
            mask,
            0,
            Vec3::ZERO,
        );
        self.ignored_brush.set(previous);
        self.ignored_plane.set(previous_plane);
        if clear.start_solid || clear.fraction < 1. {
            return None;
        }
        Some(ContactPatch {
            start,
            end,
            radius,
            grown_dist: dist + extent,
            template: hit,
        })
    }
    pub fn snapshot_epoch(&self) -> u64 {
        self.epoch.get()
    }
    /// BSP/static-OBB traces do not depend on movable entity colliders.
    pub fn environment_epoch(&self) -> u64 {
        self.environment_epoch.get()
    }
    pub fn new(bsp: std::sync::Arc<Bsp>) -> Tracer {
        let disp = DispSet::build(&bsp);
        let n = disp.tris.len();
        let brush_planes = bsp
            .brushes
            .iter()
            .map(|b| {
                (0..b.num_sides as usize)
                    .map(|i| {
                        let p =
                            bsp.planes[bsp.brush_sides[b.first_side as usize + i].plane as usize];
                        (p.normal, p.dist)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let brush_bounds = brush_planes
            .iter()
            .map(|planes| {
                let mut lo = Vec3::splat(f32::NEG_INFINITY);
                let mut hi = Vec3::splat(f32::INFINITY);
                for &(n, d) in planes {
                    for a in 0..3 {
                        if n[a].abs() == 1. && n[(a + 1) % 3] == 0. && n[(a + 2) % 3] == 0. {
                            if n[a] > 0. {
                                hi[a] = hi[a].min(d);
                            } else {
                                lo[a] = lo[a].max(-d);
                            }
                        }
                    }
                }
                (lo.is_finite() && hi.is_finite() && lo.cmple(hi).all()).then_some((lo, hi))
            })
            .collect();
        Tracer {
            ignored_brush: Cell::new(None),
            ignored_plane: Cell::new(None),
            epoch: Cell::new(0),
            environment_epoch: Cell::new(0),
            brush_planes: std::sync::Arc::new(brush_planes),
            brush_bounds: std::sync::Arc::new(brush_bounds),
            bsp: bsp.clone(),
            visited: RefCell::new(vec![0; bsp.brushes.len()]),
            stamp: Cell::new(0),
            disp: std::sync::Arc::new(disp),
            disp_visited: RefCell::new(vec![0; n]),
            colliders: RefCell::new(Vec::new()),
            collider_owners: RefCell::new(Vec::new()),
            live_owners: RefCell::new(std::collections::HashSet::new()),
            collider_convexes: RefCell::new(Vec::new()),
            collider_grid: RefCell::new(HashMap::new()),
            collider_global: RefCell::new(Vec::new()),
            collider_candidates_cache: RefCell::new(Vec::new()),
            obbs: Vec::new(),
            obb_grid: std::sync::Arc::new(HashMap::new()),
            obb_candidates: RefCell::new(Vec::new()),
        }
    }

    /// Trace a box (`mins`/`maxs` relative to the origin) from `start` to `end` through brush model `model`
    /// (0 = world; others are brush entities positioned by `model_origin`).
    /// Debug helper: (center, world mins, world maxs, convex count) of every prop collider whose bounds contain `p` expanded by `r`.
    pub fn debug_obbs_near(&self, p: Vec3, r: f32) -> Vec<(Vec3, Vec3, Vec3, usize)> {
        self.obbs
            .iter()
            .filter(|o| {
                !(o.maxs.cmplt(p - Vec3::splat(r)).any() || o.mins.cmpgt(p + Vec3::splat(r)).any())
            })
            .map(|o| {
                (
                    o.center,
                    o.mins,
                    o.maxs,
                    o.shape.as_ref().map(|s| s.len()).unwrap_or(0),
                )
            })
            .collect()
    }

    /// Debug helper: planes (face or bevel) of nearby convex props within `eps` of the hull-centre point (grown space).
    pub fn debug_touching(&self, start: Vec3, mins: Vec3, maxs: Vec3, eps: f32) -> Vec<String> {
        let mut out = Vec::new();
        for (oi, o) in self.obbs.iter().enumerate() {
            if o.maxs.cmplt(start + mins - Vec3::splat(8.0)).any()
                || o.mins.cmpgt(start + maxs + Vec3::splat(8.0)).any()
            {
                continue;
            }
            let Some(shape) = &o.shape else { continue };
            let inv = o.rot.transpose();
            let (hc, ext) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5);
            let ls = inv * (start + hc - o.center);
            let a = Mat3::from_cols(inv.x_axis.abs(), inv.y_axis.abs(), inv.z_axis.abs());
            let lext = a * ext;
            for (ci, cv) in shape.iter().enumerate() {
                for (k, (n, d)) in cv.planes.iter().chain(cv.bevels.iter()).enumerate() {
                    let val = n.dot(ls) - (d + n.abs().dot(lext));
                    if val.abs() < eps {
                        out.push(format!("obb {oi} convex {ci} plane {k} bevel={} n_local {} n_world {} val {val:.4}", k >= cv.planes.len(), n, o.rot * *n));
                    }
                }
            }
        }
        out
    }

    /// Debug helper: which world convexes contain the hull-centre point in grown space, with the largest plane value.
    pub fn debug_inside(&self, start: Vec3, mins: Vec3, maxs: Vec3) -> Vec<String> {
        let mut out = Vec::new();
        let (hc, ext) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5);
        for (oi, o) in self.obbs.iter().enumerate() {
            for (ci, cv) in o.wconvex.iter().enumerate() {
                let mut worst = f32::MIN;
                for &(n, d) in cv.planes.iter() {
                    worst = worst.max(n.dot(start + hc) - (d + n.abs().dot(ext)));
                }
                if worst < 0.0 {
                    out.push(format!("obb {oi} convex {ci}: inside, worst plane value {worst:.3}, planes {}, bounds {}..{}", cv.planes.len(), cv.mins, cv.maxs));
                }
            }
        }
        out
    }

    /// Switch the collision of every prop box owned by map entity `owner` on/off (broken glass, killed props).
    pub fn set_obb_owner_active(&self, owner: usize, active: bool) {
        for o in self.obbs.iter().filter(|o| o.owner == owner) {
            if o.enabled.replace(active) != active {
                self.epoch.set(self.epoch.get().wrapping_add(1));
                self.environment_epoch
                    .set(self.environment_epoch.get().wrapping_add(1));
            }
        }
    }
    /// Synchronize map-owned boxes in one pass, leaving compiled static props
    /// alone. This avoids an entities × boxes scan on each map update.
    pub fn sync_obb_owner_activity(&self, active: impl Fn(usize) -> bool) {
        for o in &self.obbs {
            if o.owner != usize::MAX {
                let value = active(o.owner);
                if o.enabled.replace(value) != value {
                    self.epoch.set(self.epoch.get().wrapping_add(1));
                    self.environment_epoch
                        .set(self.environment_epoch.get().wrapping_add(1));
                }
            }
        }
    }

    /// World bounds of the prop boxes owned by map entity `owner`.
    pub fn obb_bounds_of(&self, owner: usize) -> Option<(Vec3, Vec3)> {
        let mut it = self.obbs.iter().filter(|o| o.owner == owner);
        let f = it.next()?;
        Some(it.fold((f.mins, f.maxs), |(a, b), o| (a.min(o.mins), b.max(o.maxs))))
    }

    /// Install prop OBBs with a 256-unit XYZ grid. Height matters in stacked
    /// interiors: tiny corpse traces must not scan every floor in a column.
    pub fn set_obbs(&mut self, obbs: Vec<Obb>) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.environment_epoch
            .set(self.environment_epoch.get().wrapping_add(1));
        let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, o) in obbs.iter().enumerate() {
            for x in (o.mins.x / 256.0).floor() as i32..=(o.maxs.x / 256.0).floor() as i32 {
                for y in (o.mins.y / 256.0).floor() as i32..=(o.maxs.y / 256.0).floor() as i32 {
                    for z in (o.mins.z / 256.0).floor() as i32..=(o.maxs.z / 256.0).floor() as i32 {
                        grid.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        self.obbs = obbs;
        self.obb_grid = std::sync::Arc::new(grid);
        self.obb_candidates.borrow_mut().clear();
    }

    fn trace_obbs(
        &self,
        t: Trace,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Trace {
        self.trace_obbs_filtered(t, start, end, mins, maxs, mask, None)
    }
    fn trace_obbs_filtered(
        &self,
        t: Trace,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        ignored: Option<usize>,
    ) -> Trace {
        self.trace_obbs_filtered_set(t, start, end, mins, maxs, mask, ignored, None)
    }
    fn trace_obbs_filtered_set(
        &self,
        mut t: Trace,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        ignored: Option<usize>,
        ignored_set: Option<&std::collections::HashSet<usize>>,
    ) -> Trace {
        let (lo, hi) = (
            start.min(end) + mins - Vec3::splat(2.0),
            start.max(end) + maxs + Vec3::splat(2.0),
        );
        let a = (lo / 256.).floor().as_ivec3();
        let b = (hi / 256.).floor().as_ivec3();
        let key = [a.x, a.y, a.z, b.x, b.y, b.z];
        let cached = self
            .obb_candidates
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone());
        let list = cached.unwrap_or_else(|| {
            let mut seen: Vec<u32> = Vec::new();
            let mut candidates: Vec<u32> = Vec::new();
            for x in (lo.x / 256.0).floor() as i32..=(hi.x / 256.0).floor() as i32 {
                for y in (lo.y / 256.0).floor() as i32..=(hi.y / 256.0).floor() as i32 {
                    candidates.clear();
                    for z in (lo.z / 256.0).floor() as i32..=(hi.z / 256.0).floor() as i32 {
                        if let Some(list) = self.obb_grid.get(&(x, y, z)) {
                            candidates.extend_from_slice(list);
                        }
                    }
                    // Keep the previous XY-column's insertion order for ties.
                    candidates.sort_unstable();
                    candidates.dedup();
                    for &i in &candidates {
                        if !seen.contains(&i) {
                            seen.push(i);
                        }
                    }
                }
            }
            let list: std::sync::Arc<[u32]> = seen.into();
            let mut cache = self.obb_candidates.borrow_mut();
            if cache.len() >= 64 {
                cache.remove(0);
            }
            cache.push((key, list.clone()));
            list
        });
        for &i in list.iter() {
            let o = &self.obbs[i as usize];
            if !o.enabled.get()
                || o.contents & mask == 0
                || ignored == Some(o.owner)
                || ignored_set.is_some_and(|set| set.contains(&o.owner))
                || self.live_owners.borrow().contains(&o.owner)
                || o.maxs.cmplt(lo).any()
                || o.mins.cmpgt(hi).any()
            {
                continue;
            }
            // Into the box's local frame; the swept hull's local AABB is |R^T| * extents.
            let inv = o.rot.transpose();
            // The hull is axis-aligned and not centred on the origin (feet): trace its centre point instead.
            let (hc, ext) = ((mins + maxs) * 0.5, (maxs - mins) * 0.5);
            let (ls, le) = (inv * (start + hc - o.center), inv * (end + hc - o.center));
            let a = Mat3::from_cols(inv.x_axis.abs(), inv.y_axis.abs(), inv.z_axis.abs());
            let lext = a * ext;
            if o.shape.is_some() {
                // Convex pieces in world space: point trace of the hull centre against planes grown by the hull extents.
                let (wlo, whi) = (start.min(end) + mins, start.max(end) + maxs);
                let (wc_s, wc_e) = (start + hc, end + hc);
                for cv in o.wconvex.iter() {
                    if cv.mins.cmpgt(whi).any() || cv.maxs.cmplt(wlo).any() {
                        continue;
                    }
                    let grown: Vec<(Vec3, f32)> = cv
                        .planes
                        .iter()
                        .map(|&(n, d)| (n, d + n.abs().dot(ext)))
                        .collect();
                    let mut visited = vec![0u32; 0];
                    let mut w = Work {
                        brush_planes: &self.brush_planes,
                        brush_bounds: &self.brush_bounds,
                        bsp: &self.bsp,
                        start: wc_s,
                        end: wc_e,
                        mins: Vec3::ZERO,
                        maxs: Vec3::ZERO,
                        extents: Vec3::ZERO,
                        is_point: true,
                        mask: u32::MAX,
                        force_volume: false,
                        ignored_brush: self.ignored_brush.get(),
                        ignored_plane: self.ignored_plane.get(),
                        trace: Trace::new(wc_s, wc_e),
                        visited: &mut visited,
                        stamp: 0,
                    };
                    if wc_s == wc_e {
                        if test_planes(&w, &grown) {
                            w.trace.fraction = 0.0;
                            w.trace.start_solid = true;
                            w.trace.all_solid = true;
                            w.trace.contents = o.contents;
                        }
                    } else {
                        clip_planes(&mut w, &grown, o.contents);
                    }
                    let mut other = w.trace;
                    other.end_pos = if other.fraction >= 1.0 {
                        end
                    } else {
                        start + (end - start) * other.fraction
                    };
                    t = merge_traces(t, other, start, end);
                    if t.all_solid {
                        return t;
                    }
                }
                continue;
            }
            let b = o.half + lext;
            let planes = [
                (Vec3::X, b.x),
                (-Vec3::X, b.x),
                (Vec3::Y, b.y),
                (-Vec3::Y, b.y),
                (Vec3::Z, b.z),
                (-Vec3::Z, b.z),
            ];
            let mut visited = vec![0u32; 0];
            let mut w = Work {
                brush_planes: &self.brush_planes,
                brush_bounds: &self.brush_bounds,
                bsp: &self.bsp,
                start: ls,
                end: le,
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                extents: Vec3::ZERO,
                is_point: true,
                mask: u32::MAX,
                force_volume: false,
                ignored_brush: self.ignored_brush.get(),
                ignored_plane: self.ignored_plane.get(),
                trace: Trace::new(ls, le),
                visited: &mut visited,
                stamp: 0,
            };
            if ls == le {
                if test_planes(&w, &planes) {
                    w.trace.fraction = 0.0;
                    w.trace.start_solid = true;
                    w.trace.all_solid = true;
                    w.trace.contents = o.contents;
                }
            } else {
                clip_planes(&mut w, &planes, o.contents);
            }
            let mut other = w.trace;
            other.normal = o.rot * other.normal;
            other.plane_dist += other.normal.dot(o.center - hc);
            other.end_pos = if other.fraction >= 1.0 {
                end
            } else {
                start + (end - start) * other.fraction
            };
            t = merge_traces(t, other, start, end);
            if t.all_solid {
                return t;
            }
        }
        t
    }

    pub fn set_colliders(&self, c: Vec<Collider>) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        *self.collider_owners.borrow_mut() = (0..c.len()).collect();
        *self.live_owners.borrow_mut() = (0..c.len()).collect();
        let mut cached = self.collider_convexes.borrow_mut();
        cached.resize_with(c.len(), || None);
        for (collider, prepared) in c.iter().zip(cached.iter_mut()) {
            prepare_convexes(collider, prepared);
        }
        *self.colliders.borrow_mut() = c;
        drop(cached);
        self.rebuild_collider_grid();
    }
    pub fn set_owned_colliders(&self, colliders: Vec<(usize, Collider)>) {
        static REUSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let reuse = *REUSE.get_or_init(|| !std::env::args().any(|a| a == "--no-collider-reuse"));
        let owners = self.collider_owners.borrow();
        let previous = self.colliders.borrow();
        if reuse
            && colliders.len() == previous.len()
            && owners.len() == previous.len()
            && colliders
                .iter()
                .zip(previous.iter())
                .zip(owners.iter())
                .all(|(((owner, new), old), old_owner)| {
                    owner == old_owner && collider_unchanged(new, old)
                })
        {
            return;
        }
        drop(owners);
        drop(previous);
        let (owners, shapes): (Vec<_>, Vec<_>) = colliders.into_iter().unzip();
        self.set_colliders(shapes);
        *self.live_owners.borrow_mut() = owners.iter().copied().collect();
        *self.collider_owners.borrow_mut() = owners;
    }
    pub fn collider(&self, i: usize) -> Option<Collider> {
        self.colliders.borrow().get(i).cloned()
    }
    pub fn set_collider(&self, i: usize, c: Collider) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        if let Some(slot) = self.colliders.borrow_mut().get_mut(i) {
            prepare_convexes(&c, &mut self.collider_convexes.borrow_mut()[i]);
            *slot = c;
        }
        self.rebuild_collider_grid();
    }

    fn rebuild_collider_grid(&self) {
        self.collider_candidates_cache.borrow_mut().clear();
        let colliders = self.colliders.borrow();
        let prepared = self.collider_convexes.borrow();
        let mut grid = self.collider_grid.borrow_mut();
        let mut global = self.collider_global.borrow_mut();
        grid.clear();
        global.clear();
        for (index, c) in colliders.iter().enumerate() {
            let bounds = match &c.shape {
                ColliderShape::Box { mins, maxs, .. } => Some((*mins, *maxs)),
                ColliderShape::Model { model, origin } => self
                    .bsp
                    .models
                    .get(*model)
                    .map(|m| (m.mins + *origin, m.maxs + *origin)),
                ColliderShape::RotatedModel { model, matrix } => {
                    self.bsp.models.get(*model).map(|m| rotated_bounds(m.mins, m.maxs, *matrix))
                }
                ColliderShape::OrientedBox {
                    mins, maxs, matrix, ..
                } => {
                    let center = matrix.transform_point3((*mins + *maxs) * 0.5);
                    let half = (*maxs - *mins) * 0.5;
                    let r = Mat3::from_mat4(*matrix);
                    let extent =
                        r.x_axis.abs() * half.x + r.y_axis.abs() * half.y + r.z_axis.abs() * half.z;
                    Some((center - extent, center + extent))
                }
                ColliderShape::Convexes { .. } => prepared[index].as_ref().map(|p| {
                    p.pieces.iter().fold(
                        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                        |(lo, hi), piece| (lo.min(piece.mins), hi.max(piece.maxs)),
                    )
                }),
            };
            let Some((lo, hi)) = bounds else {
                global.push(index);
                continue;
            };
            if !lo.is_finite() || !hi.is_finite() {
                global.push(index);
                continue;
            }
            let a = (lo / 256.).floor().as_ivec3();
            let b = (hi / 256.).floor().as_ivec3();
            let count = (b.x as i64 - a.x as i64 + 1)
                .saturating_mul(b.y as i64 - a.y as i64 + 1)
                .saturating_mul(b.z as i64 - a.z as i64 + 1);
            if count <= 0 || count > 4096 {
                global.push(index);
                continue;
            }
            for x in a.x..=b.x {
                for y in a.y..=b.y {
                    for z in a.z..=b.z {
                        grid.entry((x, y, z)).or_default().push(index);
                    }
                }
            }
        }
    }
    fn collider_candidates(&self, lo: Vec3, hi: Vec3, count: usize) -> std::sync::Arc<[usize]> {
        let a = (lo / 256.).floor().as_ivec3();
        let b = (hi / 256.).floor().as_ivec3();
        let cells = (b.x as i64 - a.x as i64 + 1)
            .saturating_mul(b.y as i64 - a.y as i64 + 1)
            .saturating_mul(b.z as i64 - a.z as i64 + 1);
        if !lo.is_finite() || !hi.is_finite() || cells <= 0 || cells > 4096 {
            return (0..count).collect::<Vec<_>>().into();
        }
        let key = [a.x, a.y, a.z, b.x, b.y, b.z];
        if let Some((_, list)) = self
            .collider_candidates_cache
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
        {
            return list.clone();
        }
        let grid = self.collider_grid.borrow();
        let mut candidates = self.collider_global.borrow().clone();
        for x in a.x..=b.x {
            for y in a.y..=b.y {
                for z in a.z..=b.z {
                    if let Some(indices) = grid.get(&(x, y, z)) {
                        candidates.extend_from_slice(indices);
                    }
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        let list: std::sync::Arc<[usize]> = candidates.into();
        let mut cache = self.collider_candidates_cache.borrow_mut();
        if cache.len() >= 64 {
            cache.remove(0);
        }
        cache.push((key, list.clone()));
        list
    }

    /// Trace against the world (model 0 + displacements) and every active collider; the nearest hit wins.
    pub fn box_trace(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        model: usize,
        model_origin: Vec3,
    ) -> Trace {
        self.box_trace_filtered(
            start,
            end,
            mins,
            maxs,
            mask,
            model,
            model_origin,
            None,
            None,
        )
    }
    pub fn box_trace_without_owner(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        owner: usize,
    ) -> Trace {
        self.box_trace_filtered(
            start,
            end,
            mins,
            maxs,
            mask,
            0,
            Vec3::ZERO,
            Some(owner),
            None,
        )
    }
    pub fn box_trace_without_owners(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        ignored: &std::collections::HashSet<usize>,
    ) -> Trace {
        self.box_trace_filtered(
            start,
            end,
            mins,
            maxs,
            mask,
            0,
            Vec3::ZERO,
            None,
            Some(ignored),
        )
    }
    fn box_trace_filtered(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        model: usize,
        model_origin: Vec3,
        ignored: Option<usize>,
        ignored_set: Option<&std::collections::HashSet<usize>>,
    ) -> Trace {
        let mut t = self.box_trace_model(start, end, mins, maxs, mask, model, model_origin);
        if model != 0 || t.all_solid {
            return t;
        }
        if !self.obbs.is_empty() {
            t = self.trace_obbs_filtered_set(t, start, end, mins, maxs, mask, ignored, ignored_set);
            if t.all_solid {
                return t;
            }
        }
        let colliders = self.colliders.borrow();
        let owners = self.collider_owners.borrow();
        let prepared = self.collider_convexes.borrow();
        if colliders.is_empty() {
            return t;
        }
        let (lo, hi) = (
            start.min(end) + mins - Vec3::splat(2.0),
            start.max(end) + maxs + Vec3::splat(2.0),
        );
        for &index in self.collider_candidates(lo, hi, colliders.len()).iter() {
            let c = &colliders[index];
            if !(c.active
                && ignored != Some(owners[index])
                && !ignored_set.is_some_and(|s| s.contains(&owners[index])))
            {
                continue;
            }
            let other = match &c.shape {
                ColliderShape::Convexes { contents, .. } => {
                    if contents & mask == 0 {
                        continue;
                    }
                    let mut result = Trace::new(start, end);
                    for placed in prepared[index]
                        .as_ref()
                        .expect("Prepared convex collider")
                        .pieces
                        .iter()
                    {
                        if self.ignored_plane.get().is_some_and(|face| {
                            placed
                                .planes
                                .iter()
                                .any(|&(n, d)| n == face.0 && d <= face.1)
                        }) {
                            continue;
                        }
                        if placed.maxs.cmplt(lo).any() || placed.mins.cmpgt(hi).any() {
                            continue;
                        }
                        let other =
                            self.trace_planes(start, end, mins, maxs, &placed.planes, *contents);
                        result = merge_traces(result, other, start, end);
                        if result.all_solid {
                            break;
                        }
                    }
                    result
                }
                ColliderShape::OrientedBox {
                    mins: bmin,
                    maxs: bmax,
                    matrix,
                    contents,
                } => {
                    if contents & mask == 0 {
                        continue;
                    }
                    let center = matrix.transform_point3((*bmin + *bmax) * 0.5);
                    let half = (*bmax - *bmin) * 0.5;
                    let rotation = Mat3::from_mat4(*matrix);
                    let extent = rotation.x_axis.abs() * half.x
                        + rotation.y_axis.abs() * half.y
                        + rotation.z_axis.abs() * half.z;
                    if (center + extent).cmplt(lo).any() || (center - extent).cmpgt(hi).any() {
                        continue;
                    }
                    self.trace_oriented_box(
                        start, end, mins, maxs, *bmin, *bmax, *matrix, *contents,
                    )
                }
                ColliderShape::Model { model, origin } => {
                    let Some(m) = self.bsp.models.get(*model) else {
                        continue;
                    };
                    if (m.maxs + *origin).cmplt(lo).any() || (m.mins + *origin).cmpgt(hi).any() {
                        continue;
                    }
                    self.box_trace_model(start, end, mins, maxs, mask, *model, *origin)
                }
                ColliderShape::RotatedModel { model, matrix } => {
                    let Some(m) = self.bsp.models.get(*model) else {
                        continue;
                    };
                    let (wlo, whi) = rotated_bounds(m.mins, m.maxs, *matrix);
                    if whi.cmplt(lo).any() || wlo.cmpgt(hi).any() {
                        continue;
                    }
                    self.trace_rotated_model(start, end, mins, maxs, mask, *model, *matrix)
                }
                ColliderShape::Box {
                    mins: bmin,
                    maxs: bmax,
                    contents,
                } => {
                    if bmax.cmplt(lo).any() || bmin.cmpgt(hi).any() || contents & mask == 0 {
                        continue;
                    }
                    self.trace_box_collider(start, end, mins, maxs, *bmin, *bmax, *contents)
                }
            };
            t = merge_traces(t, other, start, end);
            if t.all_solid {
                break;
            }
        }
        t
    }

    /// Trace a hull against a rotated brush model: start/end into model
    /// space, the hull as the model-space AABB of the rotated box, then the
    /// hit plane back to world space.
    #[allow(clippy::too_many_arguments)]
    fn trace_rotated_model(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        model: usize,
        matrix: Mat4,
    ) -> Trace {
        let inverse = matrix.inverse();
        let rotation = Mat3::from_mat4(matrix);
        let back = rotation.transpose();
        let centre = (mins + maxs) * 0.5;
        let half = (maxs - mins) * 0.5;
        let local_half = back.x_axis.abs() * half.x + back.y_axis.abs() * half.y + back.z_axis.abs() * half.z;
        let local_centre = back * centre;
        let a = inverse.transform_point3(start);
        let b = inverse.transform_point3(end);
        let mut t = self.box_trace_model(
            a,
            b,
            local_centre - local_half,
            local_centre + local_half,
            mask,
            model,
            Vec3::ZERO,
        );
        let normal = rotation * t.normal;
        let on_plane = matrix.transform_point3(t.normal * t.plane_dist);
        t.start = start;
        t.end_pos = start + (end - start) * t.fraction;
        if t.normal.length_squared() > 0. {
            t.normal = normal.normalize();
            t.plane_dist = t.normal.dot(on_plane);
        }
        t.hit_origin = Vec3::ZERO;
        // Contact patches assume axial world brushes.
        t.hit_brush = None;
        t
    }

    fn trace_oriented_box(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        bmin: Vec3,
        bmax: Vec3,
        matrix: Mat4,
        contents: u32,
    ) -> Trace {
        let center = matrix.transform_point3((bmin + bmax) * 0.5);
        let half = (bmax - bmin) * 0.5;
        let rotation = Mat3::from_mat4(matrix);
        let axes = [rotation.x_axis, rotation.y_axis, rotation.z_axis];
        let mut normals = vec![Vec3::X, Vec3::Y, Vec3::Z];
        normals.extend(axes);
        for a in axes {
            for b in [Vec3::X, Vec3::Y, Vec3::Z] {
                let n = a.cross(b);
                if n.length_squared() > 1e-8 {
                    normals.push(n.normalize());
                }
            }
        }
        let mut planes = vec![];
        for n in normals {
            let radius = (0..3).map(|i| n.dot(axes[i]).abs() * half[i]).sum::<f32>();
            planes.push((n, n.dot(center) + radius));
            planes.push((-n, -n.dot(center) + radius));
        }
        self.trace_planes(start, end, mins, maxs, &planes, contents)
    }

    fn trace_planes(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        planes: &[(Vec3, f32)],
        contents: u32,
    ) -> Trace {
        let mut visited = vec![];
        let mut w = Work {
            brush_planes: &self.brush_planes,
            brush_bounds: &self.brush_bounds,
            bsp: &self.bsp,
            start,
            end,
            mins,
            maxs,
            extents: mins.abs().max(maxs.abs()),
            is_point: mins == Vec3::ZERO && maxs == Vec3::ZERO,
            mask: u32::MAX,
            force_volume: false,
            ignored_brush: self.ignored_brush.get(),
            ignored_plane: self.ignored_plane.get(),
            trace: Trace::new(start, end),
            visited: &mut visited,
            stamp: 0,
        };
        if start == end {
            if test_planes(&w, planes) {
                w.trace.fraction = 0.;
                w.trace.start_solid = true;
                w.trace.all_solid = true;
                w.trace.contents = contents;
            }
        } else {
            clip_planes(&mut w, planes, contents);
        }
        w.trace.end_pos = start + (end - start) * w.trace.fraction;
        w.trace
    }

    fn trace_box_collider(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        bmin: Vec3,
        bmax: Vec3,
        contents: u32,
    ) -> Trace {
        let planes = [
            (Vec3::X, bmax.x),
            (-Vec3::X, -bmin.x),
            (Vec3::Y, bmax.y),
            (-Vec3::Y, -bmin.y),
            (Vec3::Z, bmax.z),
            (-Vec3::Z, -bmin.z),
        ];
        let extents = Vec3::new(
            (-mins.x).max(maxs.x),
            (-mins.y).max(maxs.y),
            (-mins.z).max(maxs.z),
        );
        let mut visited = vec![0u32; 0];
        let mut w = Work {
            brush_planes: &self.brush_planes,
            brush_bounds: &self.brush_bounds,
            bsp: &self.bsp,
            start,
            end,
            mins,
            maxs,
            extents,
            is_point: mins == Vec3::ZERO && maxs == Vec3::ZERO,
            mask: u32::MAX,
            force_volume: false,
            ignored_brush: self.ignored_brush.get(),
            ignored_plane: self.ignored_plane.get(),
            trace: Trace::new(start, end),
            visited: &mut visited,
            stamp: 0,
        };
        if start == end {
            if test_planes(&w, &planes) {
                w.trace.fraction = 0.0;
                w.trace.start_solid = true;
                w.trace.all_solid = true;
                w.trace.contents = contents;
            }
        } else {
            clip_planes(&mut w, &planes, contents);
        }
        let mut t = w.trace;
        t.end_pos = if t.fraction >= 1.0 {
            end
        } else {
            start + (end - start) * t.fraction
        };
        t
    }

    /// Trigger hulls retain their convex planes even when the compiler marks
    /// their leaves/brushes CONTENTS_EMPTY. They must not become movement solids.
    pub fn trigger_overlap(&self, point: Vec3, mins: Vec3, maxs: Vec3, model: usize) -> bool {
        self.box_trace_model_filter(point, point, mins, maxs, u32::MAX, model, Vec3::ZERO, true)
            .start_solid
    }
    /// Ladder/trigger geometry may have CONTENTS_EMPTY. Keep its authored
    /// convex planes while returning the actual contacted face and fraction.
    pub fn trigger_sweep(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, model: usize) -> Trace {
        self.box_trace_model_filter(start, end, mins, maxs, u32::MAX, model, Vec3::ZERO, true)
    }
    pub fn box_trace_environment(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Trace {
        let t = self.box_trace_model(start, end, mins, maxs, mask, 0, Vec3::ZERO);
        self.trace_obbs(t, start, end, mins, maxs, mask)
    }
    /// World brushes only (no props, boxes or OBBs): Source's world-only
    /// trace filter.
    pub fn world_box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        self.box_trace_model(start, end, mins, maxs, mask, 0, Vec3::ZERO)
    }
    fn box_trace_model(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        model: usize,
        model_origin: Vec3,
    ) -> Trace {
        self.box_trace_model_filter(start, end, mins, maxs, mask, model, model_origin, false)
    }
    fn box_trace_model_filter(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        model: usize,
        model_origin: Vec3,
        force_volume: bool,
    ) -> Trace {
        let Some(m) = self.bsp.models.get(model) else {
            return Trace::new(start, end);
        };
        let head = m.head_node;
        let stamp = self.stamp.get().wrapping_add(1);
        self.stamp.set(stamp);
        let mut visited = self.visited.borrow_mut();
        let (s, e) = (start - model_origin, end - model_origin);
        let extents = Vec3::new(
            (-mins.x).max(maxs.x),
            (-mins.y).max(maxs.y),
            (-mins.z).max(maxs.z),
        );
        let mut w = Work {
            brush_planes: &self.brush_planes,
            brush_bounds: &self.brush_bounds,
            bsp: &self.bsp,
            start: s,
            end: e,
            mins,
            maxs,
            extents,
            is_point: mins == Vec3::ZERO && maxs == Vec3::ZERO,
            mask,
            force_volume,
            ignored_brush: self.ignored_brush.get(),
            ignored_plane: self.ignored_plane.get().map(|(n, d)| {
                let margin = if model_origin == Vec3::ZERO {
                    0.
                } else {
                    DIST_EPSILON * 2.
                };
                (n, d - n.dot(model_origin) - margin)
            }),
            trace: Trace::new(s, e),
            visited: &mut visited,
            stamp,
        };
        if s == e {
            // Position test (CM_TestInLeaf over every leaf the box touches).
            let (lo, hi) = (s + mins - Vec3::ONE, s + maxs + Vec3::ONE);
            test_node(&mut w, head, lo, hi);
        } else {
            recurse(&mut w, head, 0.0, 1.0, s, e);
        }
        if model == 0 {
            self.trace_disp(&mut w, stamp);
        }
        let mut t = w.trace;
        t.start = start;
        if t.hit_brush.is_some() {
            t.hit_model = model;
            t.hit_origin = model_origin;
        }
        t.end_pos = if t.fraction >= 1.0 {
            end
        } else {
            start + (end - start) * t.fraction
        };
        t
    }

    fn trace_disp(&self, w: &mut Work, stamp: u32) {
        if self.disp.tris.is_empty() || w.trace.all_solid {
            return;
        }
        let lo = w.start.min(w.end) + w.mins - Vec3::splat(1.0);
        let hi = w.start.max(w.end) + w.maxs + Vec3::splat(1.0);
        let mut dv = self.disp_visited.borrow_mut();
        let (x0, x1) = (
            (lo.x / DISP_CELL).floor() as i32,
            (hi.x / DISP_CELL).floor() as i32,
        );
        let (y0, y1) = (
            (lo.y / DISP_CELL).floor() as i32,
            (hi.y / DISP_CELL).floor() as i32,
        );
        for x in x0..=x1 {
            for y in y0..=y1 {
                let Some(list) = self.disp.grid.get(&(x, y)) else {
                    continue;
                };
                for &ti in list {
                    if dv[ti as usize] == stamp {
                        continue;
                    }
                    dv[ti as usize] = stamp;
                    let t = &self.disp.tris[ti as usize];
                    if t.contents & w.mask == 0 || t.mins.cmpgt(hi).any() || t.maxs.cmplt(lo).any()
                    {
                        continue;
                    }
                    // c13m1's creek water surface is a displacement flagged
                    // no hull / no ray collision: survivors wade through it.
                    let point = w.mins == Vec3::ZERO && w.maxs == Vec3::ZERO;
                    if (point && t.no_ray) || (!point && t.no_hull) {
                        continue;
                    }
                    if w.start == w.end {
                        if test_planes(w, &t.planes) {
                            w.trace.fraction = 0.0;
                            w.trace.start_solid = true;
                            w.trace.all_solid = true;
                            w.trace.contents = t.contents;
                            return;
                        }
                    } else {
                        clip_planes(w, &t.planes, t.contents);
                    }
                }
            }
        }
    }

    /// Contents at a point (OR of all brushes containing it), world model only.
    pub fn point_contents(&self, p: Vec3, mask: u32) -> u32 {
        let t = self.box_trace(p, p, Vec3::ZERO, Vec3::ZERO, mask, 0, Vec3::ZERO);
        if t.start_solid {
            t.contents
        } else {
            0
        }
    }
}

/// Keep the closer of two traces over the same path; start-solid is sticky.
fn merge_traces(a: Trace, b: Trace, start: Vec3, end: Vec3) -> Trace {
    if b.all_solid {
        return Trace {
            start,
            end_pos: start,
            ..b
        };
    }
    let mut out = if b.fraction < a.fraction {
        Trace { start, ..b }
    } else {
        a
    };
    out.start_solid |= a.start_solid | b.start_solid;
    out.end_pos = if out.fraction >= 1.0 {
        end
    } else {
        start + (end - start) * out.fraction
    };
    out
}

fn leaf_index(num: i32) -> usize {
    (-1 - num) as usize
}

fn test_node(w: &mut Work, num: i32, lo: Vec3, hi: Vec3) {
    if w.trace.all_solid {
        return;
    }
    if num < 0 {
        test_leaf(w, leaf_index(num));
        return;
    }
    let node = w.bsp.nodes[num as usize];
    let pl = w.bsp.planes[node.plane as usize];
    // Which side(s) of the plane does the (expanded) test box occupy?
    let (near, far) = (
        pl.normal.x * if pl.normal.x >= 0.0 { lo.x } else { hi.x }
            + pl.normal.y * if pl.normal.y >= 0.0 { lo.y } else { hi.y }
            + pl.normal.z * if pl.normal.z >= 0.0 { lo.z } else { hi.z },
        pl.normal.x * if pl.normal.x >= 0.0 { hi.x } else { lo.x }
            + pl.normal.y * if pl.normal.y >= 0.0 { hi.y } else { lo.y }
            + pl.normal.z * if pl.normal.z >= 0.0 { hi.z } else { lo.z },
    );
    if near - pl.dist >= 0.0 {
        test_node(w, node.children[0], lo, hi);
    } else if far - pl.dist < 0.0 {
        test_node(w, node.children[1], lo, hi);
    } else {
        test_node(w, node.children[0], lo, hi);
        test_node(w, node.children[1], lo, hi);
    }
}

fn test_leaf(w: &mut Work, leaf: usize) {
    let l = w.bsp.leafs[leaf];
    // Referenced brushes are authoritative, as in the sweep path. In
    // single-leaf/custom BSPs an empty leaf can still reference solid
    // boundary brushes. Skipping those produced false clear-volume
    // certificates and let articulated corpses pass through the floor.
    for k in 0..l.num_leaf_brushes as usize {
        let bi = w.bsp.leaf_brushes[l.first_leaf_brush as usize + k] as usize;
        if w.visited[bi] == w.stamp {
            continue;
        }
        w.visited[bi] = w.stamp;
        if w.ignored_brush == Some(bi)
            || w.ignored_plane.is_some_and(|face| {
                w.brush_planes[bi]
                    .iter()
                    .any(|&(n, d)| n == face.0 && d <= face.1)
            })
        {
            continue;
        }
        let b = w.bsp.brushes[bi];
        if (!w.force_volume && (b.contents as u32 & w.mask) == 0) || b.num_sides == 0 {
            continue;
        }
        if test_box_in_brush(w, bi) {
            w.trace.fraction = 0.0;
            w.trace.start_solid = true;
            w.trace.all_solid = true;
            w.trace.contents = b.contents as u32;
            return;
        }
    }
}

fn side_offset(w: &Work, n: Vec3) -> Vec3 {
    Vec3::new(
        if n.x < 0.0 { w.maxs.x } else { w.mins.x },
        if n.y < 0.0 { w.maxs.y } else { w.mins.y },
        if n.z < 0.0 { w.maxs.z } else { w.mins.z },
    )
}

fn brush_outside(w: &Work, bi: usize) -> bool {
    w.brush_bounds[bi].is_some_and(|(lo, hi)| {
        hi.cmplt(w.start.min(w.end) + w.mins - Vec3::ONE).any()
            || lo.cmpgt(w.start.max(w.end) + w.maxs + Vec3::ONE).any()
    })
}
fn test_box_in_brush(w: &Work, bi: usize) -> bool {
    if brush_outside(w, bi) {
        return false;
    }
    let b = w.bsp.brushes[bi];
    for s in 0..b.num_sides as usize {
        let side = w.bsp.brush_sides[b.first_side as usize + s];
        let pl = w.bsp.planes[side.plane as usize];
        let dist = if w.is_point {
            pl.dist
        } else {
            pl.dist - side_offset(w, pl.normal).dot(pl.normal)
        };
        if w.start.dot(pl.normal) - dist > 0.0 {
            return false;
        }
    }
    true
}

fn test_planes(w: &Work, planes: &[(Vec3, f32)]) -> bool {
    planes.iter().all(|&(n, d)| {
        let dist = if w.is_point {
            d
        } else {
            d - side_offset(w, n).dot(n)
        };
        w.start.dot(n) - dist <= 0.0
    })
}

fn recurse(w: &mut Work, num: i32, p1f: f32, p2f: f32, p1: Vec3, p2: Vec3) {
    if w.trace.fraction <= p1f {
        return;
    }
    if num < 0 {
        trace_leaf(w, leaf_index(num));
        return;
    }
    let node = w.bsp.nodes[num as usize];
    let pl = w.bsp.planes[node.plane as usize];
    let (t1, t2, offset);
    if pl.ty < 3 {
        let a = pl.ty as usize;
        t1 = p1[a] - pl.dist;
        t2 = p2[a] - pl.dist;
        offset = w.extents[a];
    } else {
        t1 = pl.normal.dot(p1) - pl.dist;
        t2 = pl.normal.dot(p2) - pl.dist;
        offset = if w.is_point {
            0.0
        } else {
            (w.extents.x * pl.normal.x).abs()
                + (w.extents.y * pl.normal.y).abs()
                + (w.extents.z * pl.normal.z).abs()
        };
    }
    if t1 >= offset && t2 >= offset {
        recurse(w, node.children[0], p1f, p2f, p1, p2);
        return;
    }
    if t1 <= -offset && t2 <= -offset {
        recurse(w, node.children[1], p1f, p2f, p1, p2);
        return;
    }
    let (side, mut frac, mut frac2);
    if t1 < t2 {
        let idist = 1.0 / (t1 - t2);
        side = 1;
        frac2 = (t1 + offset + DIST_EPSILON) * idist;
        frac = (t1 - offset + DIST_EPSILON) * idist;
    } else if t1 > t2 {
        let idist = 1.0 / (t1 - t2);
        side = 0;
        frac2 = (t1 - offset - DIST_EPSILON) * idist;
        frac = (t1 + offset + DIST_EPSILON) * idist;
    } else {
        side = 0;
        frac = 1.0;
        frac2 = 0.0;
    }
    frac = frac.clamp(0.0, 1.0);
    frac2 = frac2.clamp(0.0, 1.0);
    let midf = p1f + (p2f - p1f) * frac;
    let mid = p1 + (p2 - p1) * frac;
    recurse(w, node.children[side], p1f, midf, p1, mid);
    let midf = p1f + (p2f - p1f) * frac2;
    let mid = p1 + (p2 - p1) * frac2;
    recurse(w, node.children[side ^ 1], midf, p2f, mid, p2);
}

fn trace_leaf(w: &mut Work, leaf: usize) {
    let l = w.bsp.leafs[leaf];
    for k in 0..l.num_leaf_brushes as usize {
        let bi = w.bsp.leaf_brushes[l.first_leaf_brush as usize + k] as usize;
        if w.visited[bi] == w.stamp {
            continue;
        }
        w.visited[bi] = w.stamp;
        if w.ignored_brush == Some(bi)
            || w.ignored_plane.is_some_and(|face| {
                w.brush_planes[bi]
                    .iter()
                    .any(|&(n, d)| n == face.0 && d <= face.1)
            })
        {
            continue;
        }
        let b = w.bsp.brushes[bi];
        if !w.force_volume && (b.contents as u32 & w.mask) == 0 {
            continue;
        }
        clip_box_to_brush(w, bi);
        if w.trace.fraction == 0.0 {
            return;
        }
    }
}

fn clip_box_to_brush(w: &mut Work, bi: usize) {
    if brush_outside(w, bi) {
        return;
    }
    let b = w.bsp.brushes[bi];
    if b.num_sides == 0 {
        return;
    }
    let planes = &w.brush_planes[bi];
    let previous = w.trace.fraction;
    clip_planes(w, planes, b.contents as u32);
    if w.trace.fraction < previous {
        w.trace.hit_brush = Some(bi);
        w.trace.texinfo = (0..b.num_sides as usize).find_map(|i| {
            let side = w.bsp.brush_sides[b.first_side as usize + i];
            let plane = w.bsp.planes[side.plane as usize];
            (side.texinfo >= 0 && plane.normal.dot(w.trace.normal) > 0.9999)
                .then_some(side.texinfo as usize)
        });
    }
}

/// CM_ClipBoxToBrush over an explicit convex plane list.
fn clip_planes(w: &mut Work, planes: &[(Vec3, f32)], contents: u32) {
    let (mut enter, mut leave) = (-1.0f32, 1.0f32);
    let mut clip: Option<(Vec3, f32)> = None;
    let (mut getout, mut startout) = (false, false);
    for &(normal, pdist) in planes {
        let dist = if w.is_point {
            pdist
        } else {
            pdist - side_offset(w, normal).dot(normal)
        };
        let d1 = w.start.dot(normal) - dist;
        let d2 = w.end.dot(normal) - dist;
        if d2 > 0.0 {
            getout = true;
        }
        if d1 > 0.0 {
            startout = true;
        }
        if d1 > 0.0 && d2 >= d1 {
            return; // completely in front of the face
        }
        if d1 <= 0.0 && d2 <= 0.0 {
            continue;
        }
        if d1 > d2 {
            let f = ((d1 - DIST_EPSILON).max(0.0)) / (d1 - d2);
            if f > enter {
                enter = f;
                clip = Some((normal, dist));
            }
        } else {
            let f = (d1 + DIST_EPSILON) / (d1 - d2);
            if f < leave {
                leave = f;
            }
        }
    }
    if !startout {
        w.trace.start_solid = true;
        w.trace.contents = contents;
        if !getout {
            w.trace.all_solid = true;
            w.trace.fraction = 0.0;
        }
        return;
    }
    if enter < leave && enter > -1.0 && enter < w.trace.fraction {
        let f = enter.max(0.0);
        w.trace.fraction = f;
        if let Some((n, d)) = clip {
            w.trace.normal = n;
            w.trace.plane_dist = d;
        }
        w.trace.contents = contents;
    }
}

#[cfg(test)]
mod brush_cache_tests {
    use super::*;
    #[test]
    fn certified_floor_patch_matches_full_scene_with_plane_epsilon() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        if !path.exists() {
            return;
        }
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let tracer = Tracer::new(bsp);
        let half = Vec3::splat(0.02);
        for height in [8., 0.021] {
            let start = Vec3::new(1200., 800., height);
            let end = Vec3::new(1200., 800., if height > 1. { -1. } else { -0.2 });
            let patch = tracer
                .certify_contact_patch(start, end, half, 0.5, CONTENTS_SOLID)
                .expect("open lab floor must certify");
            for i in 0..2000 {
                let jitter = Vec3::new(
                    (i % 17) as f32 / 17. - 0.5,
                    ((i / 17) % 19) as f32 / 19. - 0.5,
                    (i % 31) as f32 / 31. - 0.5,
                );
                let point = end + jitter;
                let b = tracer.box_trace(start, point, -half, half, CONTENTS_SOLID, 0, Vec3::ZERO);
                let Some(a) = patch.trace(point) else {
                    continue;
                };
                assert_eq!(
                    (a.fraction, a.normal, a.start_solid, a.end_pos),
                    (b.fraction, b.normal, b.start_solid, b.end_pos),
                    "height {height} jitter {jitter:?}"
                );
                let shifted_start = start + jitter * 0.1;
                if let Some(shared) = patch.trace_from(shifted_start, point) {
                    let full = tracer.box_trace(
                        shifted_start,
                        point,
                        -half,
                        half,
                        CONTENTS_SOLID,
                        0,
                        Vec3::ZERO,
                    );
                    assert_eq!(
                        (
                            shared.fraction,
                            shared.normal,
                            shared.start_solid,
                            shared.end_pos
                        ),
                        (full.fraction, full.normal, full.start_solid, full.end_pos)
                    );
                }
            }
        }
    }
    #[test]
    fn stacked_prop_grid_matches_full_height_column_traces() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        if !path.exists() {
            return;
        }
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let mut fast = Tracer::new(bsp);
        let obbs = (0..320)
            .map(|i| {
                Obb::new(
                    Vec3::new(
                        (i % 5) as f32 * 150. - 300.,
                        ((i / 5) % 4) as f32 * 180. - 250.,
                        (i / 20) as f32 * 256. - 1920.,
                    ),
                    Vec3::new(30., 40., 12.),
                    Mat3::from_rotation_z(i as f32 * 0.09),
                    CONTENTS_SOLID,
                )
            })
            .collect();
        fast.set_obbs(obbs);
        let mut reference = fast.clone();
        let mut full_column = HashMap::new();
        for (i, o) in reference.obbs.iter().enumerate() {
            for x in (o.mins.x / 256.).floor() as i32..=(o.maxs.x / 256.).floor() as i32 {
                for y in (o.mins.y / 256.).floor() as i32..=(o.maxs.y / 256.).floor() as i32 {
                    for z in -16..=16 {
                        full_column
                            .entry((x, y, z))
                            .or_insert_with(Vec::new)
                            .push(i as u32);
                    }
                }
            }
        }
        reference.obb_grid = std::sync::Arc::new(full_column);
        let mut seed = 42u32;
        for i in 0..1500 {
            let mut value = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / 16777216. - 0.5
            };
            let center = fast.obbs[i % fast.obbs.len()].center;
            let start = center + Vec3::new(value(), value(), value()) * 140.;
            let end = if i % 3 == 0 {
                start
            } else {
                start + Vec3::new(value(), value(), value()) * 600.
            };
            let half = Vec3::splat(if i % 2 == 0 { 0.02 } else { 16. });
            let test = |tracer: &Tracer| {
                tracer.trace_obbs_filtered(
                    Trace::new(start, end),
                    start,
                    end,
                    -half,
                    half,
                    CONTENTS_SOLID,
                    None,
                )
            };
            let (a, b) = (test(&fast), test(&reference));
            assert_eq!(
                (a.fraction, a.normal, a.start_solid, a.all_solid, a.end_pos),
                (b.fraction, b.normal, b.start_solid, b.all_solid, b.end_pos)
            );
        }
    }
    #[test]
    fn owner_sets_exclude_parented_prop_obbs_without_hiding_other_obstacles() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let mut tracer = Tracer::new(bsp);
        let mut glass = Obb::new(Vec3::ZERO, Vec3::splat(5.), Mat3::IDENTITY, CONTENTS_SOLID);
        glass.owner = 17;
        let mut wall = Obb::new(Vec3::X * 25., Vec3::splat(5.), Mat3::IDENTITY, CONTENTS_SOLID);
        wall.owner = 18;
        tracer.set_obbs(vec![glass, wall]);
        let start = Vec3::X * -20.;
        let end = Vec3::X * 40.;
        let trace = |owners: &std::collections::HashSet<usize>| tracer.trace_obbs_filtered_set(
            Trace::new(start, end), start, end, Vec3::ZERO, Vec3::ZERO, CONTENTS_SOLID, None, Some(owners));
        assert!(trace(&Default::default()).fraction < 0.5);
        let skipped = trace(&std::collections::HashSet::from([17]));
        assert!(skipped.fraction > 0.5 && skipped.fraction < 1.);
        assert_eq!(trace(&std::collections::HashSet::from([17,18])).fraction, 1.);
    }
    #[test]
    fn cached_prop_candidates_follow_activity_and_geometry_changes() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let mut tracer = Tracer::new(bsp);
        let mut prop = Obb::new(Vec3::ZERO, Vec3::splat(10.), Mat3::IDENTITY, CONTENTS_SOLID);
        prop.owner = 17;
        tracer.set_obbs(vec![prop]);
        let start = Vec3::X * -20.;
        let end = Vec3::X * 20.;
        let trace = |t: &Tracer| {
            t.trace_obbs_filtered(
                Trace::new(start, end),
                start,
                end,
                Vec3::ZERO,
                Vec3::ZERO,
                CONTENTS_SOLID,
                None,
            )
        };
        assert!(trace(&tracer).fraction < 1.);
        assert!(trace(&tracer).fraction < 1.);
        let snapshot = tracer.clone();
        tracer.set_obb_owner_active(17, false);
        assert_eq!(trace(&tracer).fraction, 1.);
        assert!(trace(&snapshot).fraction < 1.);
        tracer.set_obbs(Vec::new());
        assert_eq!(trace(&tracer).fraction, 1.);
        tracer.set_obbs(vec![Obb::new(
            Vec3::ZERO,
            Vec3::splat(5.),
            Mat3::IDENTITY,
            CONTENTS_SOLID,
        )]);
        assert!(trace(&tracer).fraction < 1.);
    }
    #[test]
    fn lab_floor_position_tests_agree_with_sweeps_inside_empty_leaf() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        if !path.exists() { return; }
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let tracer = Tracer::new(bsp);
        let half = Vec3::splat(0.02);
        let inside = Vec3::new(128., 285., -0.1);
        assert!(tracer.box_trace(inside, inside, -half, half, MASK_SOLID, 0, Vec3::ZERO).start_solid);
        let above = Vec3::new(128., 285., 10.);
        assert!(!tracer.box_trace(above, above, -half, half, MASK_SOLID, 0, Vec3::ZERO).start_solid);
        assert!(tracer.box_trace(above, inside, -half, half, MASK_SOLID, 0, Vec3::ZERO).fraction < 1.);
    }
    #[test]
    fn brush_bounds_match_unfiltered_clipping_for_lab_sweeps_and_position_tests() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maps/rewrite_lab.bsp");
        if !path.exists() {
            return;
        }
        let bsp = std::sync::Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let fast = Tracer::new(bsp.clone());
        let mut control = Tracer::new(bsp);
        std::sync::Arc::make_mut(&mut control.brush_bounds).fill(None);
        let mut seed = 7u32;
        for i in 0..1000 {
            let mut random = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                seed as f32 / u32::MAX as f32
            };
            let start = Vec3::new(
                random() * 4200. - 50.,
                random() * 3200. - 50.,
                random() * 260. - 20.,
            );
            let end = if i % 3 == 0 {
                start
            } else {
                start
                    + Vec3::new(
                        random() * 320. - 160.,
                        random() * 320. - 160.,
                        random() * 160. - 80.,
                    )
            };
            for half in [Vec3::ZERO, Vec3::splat(0.02), Vec3::new(16., 16., 36.)] {
                let a = fast.box_trace(start, end, -half, half, MASK_SOLID, 0, Vec3::ZERO);
                let b = control.box_trace(start, end, -half, half, MASK_SOLID, 0, Vec3::ZERO);
                assert_eq!(a.fraction, b.fraction);
                assert_eq!(a.normal, b.normal);
                assert_eq!(a.start_solid, b.start_solid);
                assert_eq!(a.all_solid, b.all_solid);
                assert_eq!(a.contents, b.contents);
            }
        }
    }
}

/// World-space AABB of a model-space box under `matrix`.
fn rotated_bounds(mins: Vec3, maxs: Vec3, matrix: Mat4) -> (Vec3, Vec3) {
    let center = matrix.transform_point3((mins + maxs) * 0.5);
    let half = (maxs - mins) * 0.5;
    let r = Mat3::from_mat4(matrix);
    let extent = r.x_axis.abs() * half.x + r.y_axis.abs() * half.y + r.z_axis.abs() * half.z;
    (center - extent, center + extent)
}
