//! Native VHV static-prop lighting and Source ambient-cube evaluation.
use crate::binary::*;
use std::io;
/// World triangle hierarchy for lighting rays. Sky faces are already excluded
/// from this mesh, so they admit sunlight instead of blocking it like clip hulls.
#[derive(bevy::prelude::Resource)]
pub struct WorldShadow {
    nodes: Vec<ShadowNode>,
}
struct ShadowNode {
    lo: [f32; 3],
    hi: [f32; 3],
    children: Option<(usize, usize)>,
    triangles: Vec<[f32; 3]>,
}
impl WorldShadow {
    pub fn new(vertices: &[[f32; 3]]) -> Self {
        let mut tree = Self { nodes: vec![] };
        if !vertices.is_empty() {
            tree.build(
                vertices
                    .chunks_exact(3)
                    .map(|t| [t[0], t[1], t[2]])
                    .collect(),
            );
        }
        tree
    }
    fn build(&mut self, mut triangles: Vec<[[f32; 3]; 3]>) -> usize {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for t in &triangles {
            for p in t {
                for axis in 0..3 {
                    lo[axis] = lo[axis].min(p[axis]);
                    hi[axis] = hi[axis].max(p[axis]);
                }
            }
        }
        let id = self.nodes.len();
        self.nodes.push(ShadowNode {
            lo,
            hi,
            children: None,
            triangles: vec![],
        });
        if triangles.len() <= 8 {
            self.nodes[id].triangles = triangles.into_iter().flatten().collect();
        } else {
            let axis = (0..3)
                .max_by(|a, b| (hi[*a] - lo[*a]).total_cmp(&(hi[*b] - lo[*b])))
                .unwrap();
            let middle = triangles.len() / 2;
            let center = |t: &[[f32; 3]; 3]| t.iter().map(|p| p[axis]).sum::<f32>();
            triangles.select_nth_unstable_by(middle, |a, b| center(a).total_cmp(&center(b)));
            let right = triangles.split_off(middle);
            let left = self.build(triangles);
            let right = self.build(right);
            self.nodes[id].children = Some((left, right));
        }
        id
    }
    pub fn blocked(&self, origin: [f32; 3], direction: [f32; 3]) -> bool {
        !self.nodes.is_empty() && self.visit(0, origin, direction, f32::INFINITY)
    }
    pub fn blocked_to(&self, origin: [f32; 3], target: [f32; 3]) -> bool {
        let d = bevy::prelude::Vec3::from_array(target) - bevy::prelude::Vec3::from_array(origin);
        !self.nodes.is_empty()
            && self.visit(
                0,
                origin,
                d.normalize_or_zero().to_array(),
                (d.length() - 0.03).max(0.),
            )
    }
    fn visit(&self, id: usize, origin: [f32; 3], direction: [f32; 3], limit: f32) -> bool {
        let node = &self.nodes[id];
        let mut near: f32 = 0.03;
        let mut far = limit;
        for axis in 0..3 {
            if direction[axis].abs() < 1e-8 {
                if origin[axis] < node.lo[axis] || origin[axis] > node.hi[axis] {
                    return false;
                }
            } else {
                let a = (node.lo[axis] - origin[axis]) / direction[axis];
                let b = (node.hi[axis] - origin[axis]) / direction[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
                if near > far {
                    return false;
                }
            }
        }
        if let Some((left, right)) = node.children {
            self.visit(left, origin, direction, limit)
                || self.visit(right, origin, direction, limit)
        } else {
            blocked_before(origin, direction, &node.triangles, limit)
        }
    }
}
#[cfg(test)]
pub fn blocked(origin: [f32; 3], direction: [f32; 3], triangles: &[[f32; 3]]) -> bool {
    blocked_before(origin, direction, triangles, f32::INFINITY)
}
fn blocked_before(
    origin: [f32; 3],
    direction: [f32; 3],
    triangles: &[[f32; 3]],
    limit: f32,
) -> bool {
    let sub = |a: [f32; 3], b: [f32; 3]| std::array::from_fn::<_, 3, _>(|i| a[i] - b[i]);
    let dot = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f32>();
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    for t in triangles.chunks_exact(3) {
        let e1 = sub(t[1], t[0]);
        let e2 = sub(t[2], t[0]);
        let h = cross(direction, e2);
        let det = dot(e1, h);
        if det.abs() < 1e-7 {
            continue;
        }
        let s = sub(origin, t[0]);
        let u = dot(s, h) / det;
        if !(0. ..=1.).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(direction, q) / det;
        if v < 0. || u + v > 1. {
            continue;
        }
        let distance = dot(e2, q) / det;
        if distance > 0.03 && distance < limit {
            return true;
        }
    }
    false
}
pub fn vhv_lod0(bytes: &[u8], checksum: u32) -> io::Result<Vec<Vec<[f32; 3]>>> {
    vhv_lod(bytes, checksum, 0)
}
pub fn vhv_lod(bytes: &[u8], checksum: u32, lod: usize) -> io::Result<Vec<Vec<[f32; 3]>>> {
    range(bytes, 0, 40)?;
    if u32_at(bytes, 0)? != 2 || u32_at(bytes, 4)? != checksum || u32_at(bytes, 12)? != 4 {
        return Err(invalid("Unsupported or mismatched VHV lighting header"));
    }
    let meshes = count(bytes, 20, 65536)?;
    let mut result = vec![];
    for i in 0..meshes {
        let mesh = range(bytes, 40 + i * 28, 28)?;
        if u32_at(mesh, 0)? as usize != lod {
            continue;
        }
        let n = count(mesh, 4, 2_000_000)?;
        let offset = u32_at(mesh, 8)? as usize;
        let data = range(bytes, offset, n * 4)?;
        // Source static vertex lighting is BGRA, gamma encoded and overbright-scaled.
        // The shader decodes GammaToLinear(staticColour * cOverbright).
        result.push(
            data.chunks_exact(4)
                .map(|p| [p[2], p[1], p[0]].map(|c| (c as f32 / 255. * 2.).powf(2.2)))
                .collect(),
        );
    }
    Ok(result)
}
pub fn ambient(cube: &[[f32; 3]; 6], normal: [f32; 3]) -> [f32; 4] {
    let length = normal.iter().map(|v| v * v).sum::<f32>().max(1e-12);
    let mut rgb = [0.; 3];
    for axis in 0..3 {
        let color = cube[axis * 2 + usize::from(normal[axis] < 0.)];
        let weight = normal[axis] * normal[axis] / length;
        for i in 0..3 {
            rgb[i] += color[i] * weight;
        }
    }
    [rgb[0], rgb[1], rgb[2], 1.]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_lighting_rays_stop_at_the_light_not_a_wall_behind_it() {
        let wall = [[-2., -2., 2.], [2., -2., 2.], [0., 2., 2.]];
        let shadows = WorldShadow::new(&wall);
        assert!(!shadows.blocked_to([0.; 3], [0., 0., 1.]));
        assert!(shadows.blocked_to([0.; 3], [0., 0., 3.]));
    }
    #[test]
    fn sun_trace_stops_at_world_geometry_but_not_behind_the_origin() {
        let triangles = [[0., 0., 1.], [2., 0., 1.], [0., 2., 1.]];
        assert!(blocked([0.5, 0.5, 0.], [0., 0., 1.], &triangles));
        assert!(!blocked([0.5, 0.5, 2.], [0., 0., 1.], &triangles));
    }
    #[test]
    fn shadow_hierarchy_matches_triangle_scan_for_parallel_and_diagonal_rays() {
        let mut triangles = vec![];
        for x in -6..6 {
            for y in -6..6 {
                let (x, y) = (x as f32, y as f32);
                triangles.extend([[x, y, 1.], [x + 0.8, y, 1.], [x, y + 0.8, 1.]]);
            }
        }
        let tree = WorldShadow::new(&triangles);
        for x in -20..20 {
            for direction in [[0., 0., 1.], [0.4, -0.2, 1.], [1., 0., 0.], [0., 0., -1.]] {
                let origin = [x as f32 * 0.4, 0.25, 0.];
                assert_eq!(
                    tree.blocked(origin, direction),
                    blocked(origin, direction, &triangles)
                );
            }
        }
        assert!(!WorldShadow::new(&[]).blocked([0.; 3], [0., 0., 1.]));
    }
    #[test]
    fn vhv_header_checksum_bgra_and_lod_selection() {
        let mut b = vec![0; 72];
        for (offset, value) in [
            (0, 2u32),
            (4, 123),
            (12, 4),
            (16, 1),
            (20, 1),
            (44, 1),
            (48, 68),
        ] {
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        b[68..72].copy_from_slice(&[0, 64, 128, 255]);
        let colors = vhv_lod0(&b, 123).unwrap();
        assert!((colors[0][0][0] - (256f32 / 255.).powf(2.2)).abs() < 1e-6);
        assert_eq!(colors[0][0][2], 0.);
        assert!(vhv_lod0(&b, 124).is_err());
        assert!(vhv_lod0(&b[..70], 123).is_err());
    }
    #[test]
    fn ambient_cube_uses_axis_signs_and_squared_normal_weights() {
        let cube = [[1.; 3], [2.; 3], [3.; 3], [4.; 3], [5.; 3], [6.; 3]];
        assert_eq!(ambient(&cube, [0., 0., -1.]), [6., 6., 6., 1.]);
        assert_eq!(ambient(&cube, [1., 1., 0.]), [2., 2., 2., 1.]);
    }
}
