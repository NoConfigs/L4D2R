//! Conservative native BSP PVS. Invalid/missing rows and outside-solid cameras
//! retain all geometry; collision and entity simulation are unaffected.
use crate::{binary, bsp};
use bevy::prelude::*;
use std::io;
#[derive(Default)]
pub struct Data {
    planes: Vec<[f32; 4]>,
    nodes: Vec<(usize, [i32; 2])>,
    leaves: Vec<i16>,
    root: i32,
    rows: Vec<Option<Vec<u8>>>,
    faces: Vec<Vec<u16>>,
}
fn int(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn short(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn decode(data: &[u8], mut at: usize, count: usize) -> Option<Vec<u8>> {
    let mut row = Vec::with_capacity(count);
    while row.len() < count {
        let byte = *data.get(at)?;
        at += 1;
        if byte != 0 {
            row.push(byte);
        } else {
            let run = *data.get(at)? as usize;
            at += 1;
            if run == 0 || run > count - row.len() {
                return None;
            }
            row.resize(row.len() + run, 0);
        }
    }
    Some(row)
}
impl Data {
    pub fn parse(b: &[u8], face_count: usize) -> io::Result<Self> {
        let vis = bsp::lump(b, 4)?;
        if vis.is_empty() {
            return Ok(Self::default());
        }
        let count = binary::count(vis, 0, 32768)?;
        binary::range(vis, 4, count * 8)?;
        let rows = (0..count)
            .map(|i| {
                let at = int(vis, 4 + i * 8)?;
                if at < 0 {
                    None
                } else {
                    decode(vis, at as usize, count.div_ceil(8))
                }
            })
            .collect();
        let planes = bsp::lump(b, 1)?
            .chunks_exact(20)
            .map(|p| {
                Ok([
                    binary::f32_at(p, 0)?,
                    binary::f32_at(p, 4)?,
                    binary::f32_at(p, 8)?,
                    binary::f32_at(p, 12)?,
                ])
            })
            .collect::<io::Result<Vec<_>>>()?;
        let nodes = bsp::lump(b, 5)?
            .chunks_exact(32)
            .map(|n| {
                (
                    int(n, 0).unwrap() as usize,
                    [int(n, 4).unwrap(), int(n, 8).unwrap()],
                )
            })
            .collect();
        let leaf_data = bsp::lump(b, 10)?;
        let leaves: Vec<_> = leaf_data
            .chunks_exact(32)
            .map(|l| short(l, 4).unwrap() as i16)
            .collect();
        let leaf_faces = bsp::lump(b, 16)?;
        let mut faces = vec![vec![]; face_count];
        for (leaf, cluster) in leaf_data.chunks_exact(32).zip(&leaves) {
            if *cluster < 0 {
                continue;
            }
            let first = short(leaf, 20).unwrap() as usize;
            let n = short(leaf, 22).unwrap() as usize;
            for i in first..first + n {
                if let Some(face) = short(leaf_faces, i * 2).and_then(|f| faces.get_mut(f as usize))
                {
                    face.push(*cluster as u16);
                }
            }
        }
        for clusters in &mut faces {
            clusters.sort_unstable();
            clusters.dedup();
        }
        let root = int(bsp::lump(b, 14)?, 36).unwrap_or(0);
        Ok(Self {
            planes,
            nodes,
            leaves,
            root,
            rows,
            faces,
        })
    }
    pub fn clusters_for_face(&self, face: usize) -> Vec<u16> {
        self.faces.get(face).cloned().unwrap_or_default()
    }
    pub fn cluster_at(&self, point: Vec3) -> Option<usize> {
        let source = Vec3::new(point.x, -point.z, point.y) / 0.0254;
        let mut node = self.root;
        for _ in 0..=self.nodes.len() {
            if node < 0 {
                let cluster = *self.leaves.get((-1 - node) as usize)?;
                return (cluster >= 0 && (cluster as usize) < self.rows.len())
                    .then_some(cluster as usize);
            }
            let (plane, children) = self.nodes.get(node as usize)?;
            let p = self.planes.get(*plane)?;
            let distance = source.dot(Vec3::new(p[0], p[1], p[2])) - p[3];
            if !distance.is_finite() {
                return None;
            }
            node = children[usize::from(distance <= 0.)];
        }
        None
    }
    pub fn visible(&self, camera: Option<usize>, clusters: &[u16]) -> bool {
        let Some(camera) = camera else {
            return true;
        };
        let Some(Some(row)) = self.rows.get(camera) else {
            return true;
        };
        clusters.is_empty()
            || clusters.iter().any(|cluster| {
                let c = *cluster as usize;
                c == camera || row.get(c / 8).is_none_or(|byte| byte & (1 << (c % 8)) != 0)
            })
    }
    /// Thin solid leaves at walls/portals should not turn on the entire map.
    /// Union every adjacent reachable leaf; missing rows and distant outside
    /// cameras still retain the original fail-open behavior.
    pub fn camera_clusters(&self, point: Vec3) -> Vec<Option<usize>> {
        if let Some(cluster) = self.cluster_at(point) {
            return vec![Some(cluster)];
        }
        let adjacent = self.clusters_in_bounds(point - Vec3::splat(0.3), point + Vec3::splat(0.3));
        if adjacent.is_empty() {
            vec![None]
        } else {
            adjacent.into_iter().map(|c| Some(c as usize)).collect()
        }
    }
    /// All leaves intersecting a render-space box, including portal crossings.
    /// Empty/invalid data conservatively leaves the object visible.
    pub fn clusters_in_bounds(&self, lo: Vec3, hi: Vec3) -> Vec<u16> {
        let center = (lo + hi) * 0.5;
        let half = (hi - lo).abs() * 0.5;
        let center = Vec3::new(center.x, -center.z, center.y) / 0.0254;
        let half = Vec3::new(half.x, half.z, half.y) / 0.0254;
        let mut stack = vec![self.root];
        let mut clusters = Vec::new();
        let mut visits = 0;
        while let Some(node) = stack.pop() {
            visits += 1;
            if visits > self.nodes.len() * 2 + self.leaves.len() + 1 {
                return vec![];
            }
            if node < 0 {
                let Some(&c) = self.leaves.get((-1 - node) as usize) else {
                    return vec![];
                };
                if c >= 0 {
                    clusters.push(c as u16);
                }
            } else {
                let Some(&(plane, children)) = self.nodes.get(node as usize) else {
                    return vec![];
                };
                let Some(p) = self.planes.get(plane) else {
                    return vec![];
                };
                let normal = Vec3::new(p[0], p[1], p[2]);
                let distance = center.dot(normal) - p[3];
                let radius = half.dot(normal.abs());
                if !distance.is_finite() || !radius.is_finite() {
                    return vec![];
                }
                if distance + radius >= 0. {
                    stack.push(children[0]);
                }
                if distance - radius <= 0. {
                    stack.push(children[1]);
                }
            }
        }
        clusters.sort_unstable();
        clusters.dedup();
        clusters
    }
}
#[derive(Component)]
pub struct Clusters(pub Vec<u16>);
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn solid_boundary_camera_unions_adjacent_leaf_visibility_without_global_draws() {
        let data = Data {
            planes: vec![[1., 0., 0., 0.], [1., 0., 0., 10.]],
            nodes: vec![(0, [1, -1]), (1, [-3, -2])],
            leaves: vec![0, -1, 1],
            root: 0,
            rows: vec![Some(vec![1]), Some(vec![2])],
            faces: vec![],
        };
        let point = Vec3::new(0.12, 0., 0.);
        assert_eq!(data.cluster_at(point), None);
        let views = data.camera_clusters(point);
        assert_eq!(views, vec![Some(0), Some(1)]);
        for cluster in [0, 1] {
            assert!(views.iter().any(|c| data.visible(*c, &[cluster])));
        }
        assert!(
            !views.iter().any(|c| data.visible(*c, &[2])),
            "A boundary must not expose unrelated clusters"
        );
        assert_eq!(Data::default().camera_clusters(point), vec![None]);
        let outside = Data {
            planes: vec![[1., 0., 0., 0.]],
            nodes: vec![(0, [-1, -2])],
            leaves: vec![-1, 0],
            root: 0,
            rows: vec![Some(vec![1])],
            faces: vec![],
        };
        assert_eq!(
            outside.camera_clusters(Vec3::new(20., 0., 0.)),
            vec![None],
            "Distant outside cameras still fail open"
        );
    }
    #[test]
    fn owned_hotel_pvs_keeps_roof_visible_and_excludes_unseen_regions() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let world =
            crate::bsp::load_file(&root.join("left4dead2/maps/c1m1_hotel.bsp"), root).unwrap();
        let camera = world.visibility.cluster_at(Vec3::from_array(world.spawn));
        assert!(camera.is_some());
        let visible = world
            .surfaces
            .iter()
            .filter(|s| world.visibility.visible(camera, &s.clusters))
            .count();
        assert!(visible > 0 && visible < world.surfaces.len());
        let visible_props = world
            .props
            .iter()
            .filter(|p| p.sky || world.visibility.visible(camera, &p.clusters))
            .count();
        assert!(visible_props > 0 && visible_props < world.props.len());
        assert!(
            world
                .props
                .iter()
                .filter(|p| p.static_index.is_some())
                .any(|p| p.clusters.len() > 1)
        );
        println!(
            "Owned Hotel PVS: camera={camera:?}, surfaces={visible}/{}, props={visible_props}/{}",
            world.surfaces.len(),
            world.props.len()
        );
    }
    #[test]
    fn rle_is_bounded_and_rejects_empty_or_oversized_runs() {
        assert_eq!(decode(&[1, 0, 2, 8], 0, 4), Some(vec![1, 0, 0, 8]));
        assert_eq!(decode(&[0, 0], 0, 2), None);
        assert_eq!(decode(&[0, 3], 0, 2), None);
        assert_eq!(decode(&[0], 0, 2), None);
    }
    #[test]
    fn shared_leaf_membership_and_invalid_camera_are_conservative() {
        let data = Data {
            rows: vec![Some(vec![1]), Some(vec![2])],
            ..Default::default()
        };
        assert!(data.visible(Some(0), &[0, 1]));
        assert!(!data.visible(Some(0), &[1]));
        assert!(data.visible(Some(1), &[1]));
        assert!(data.visible(None, &[1]));
        assert!(data.visible(Some(999), &[1]));
        assert!(data.visible(Some(0), &[]));
    }
}
