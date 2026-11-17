//! Mass/inertia contact impulses. Collision geometry remains a separate concern.
use bevy::prelude::*;
/// Cached convex geometry in renderer metres. Bevel planes are sweep helpers,
/// not faces; SAT uses actual face normals and edge cross products.
#[derive(Clone, Default)]
pub struct Hull {
    pub vertices: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub edges: Vec<Vec3>,
}
impl Hull {
    pub fn native(h: &l4d2_formats::phy::Convex) -> Self {
        let point =
            |v: l4d2r_math::Vec3| Vec3::from_array(crate::binary::source_position(v.to_array()));
        let mut vertices = Vec::new();
        for triangle in &h.triangles {
            for source in triangle {
                let v = point(*source);
                if !vertices
                    .iter()
                    .any(|p: &Vec3| p.distance_squared(v) < 1e-10)
                {
                    vertices.push(v);
                }
            }
        }
        for &(a, b) in &h.edges {
            for v in [point(a), point(b)] {
                if !vertices
                    .iter()
                    .any(|p: &Vec3| p.distance_squared(v) < 1e-10)
                {
                    vertices.push(v);
                }
            }
        }
        let unique_axes = |directions: Vec<Vec3>| {
            let mut unique = Vec::new();
            for axis in directions {
                if axis.length_squared() > 1e-8
                    && !unique
                        .iter()
                        .any(|other: &Vec3| other.dot(axis).abs() > 0.999999)
                {
                    unique.push(axis);
                }
            }
            unique
        };
        Self {
            vertices,
            normals: unique_axes(
                h.planes
                    .iter()
                    .map(|(n, _)| Vec3::from_array(crate::binary::source_normal(n.to_array())))
                    .collect(),
            ),
            edges: unique_axes(
                h.edges
                    .iter()
                    .map(|(a, b)| (point(*b) - point(*a)).normalize_or_zero())
                    .collect(),
            ),
        }
    }
    pub fn transformed(&self, t: &Transform) -> Self {
        Self {
            vertices: self
                .vertices
                .iter()
                .map(|p| t.transform_point(*p))
                .collect(),
            normals: self.normals.iter().map(|n| t.rotation * *n).collect(),
            edges: self.edges.iter().map(|e| t.rotation * *e).collect(),
        }
    }
}
fn feature_hull(mut points: Vec<Vec2>) -> Vec<Vec2> {
    points.sort_by(|a,b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup_by(|a,b| a.distance_squared(*b)<1e-12);
    if points.len()<3 { return points; }
    let mut hull=Vec::with_capacity(points.len()*2);
    for &p in &points {
        while hull.len()>=2 {
            let a: Vec2=hull[hull.len()-2];let b: Vec2=hull[hull.len()-1];
            if (b-a).perp_dot(p-b)>0. { break; } hull.pop();
        }
        hull.push(p);
    }
    let lower=hull.len();
    for &p in points.iter().rev().skip(1) {
        while hull.len()>lower {
            let a=hull[hull.len()-2];let b=hull[hull.len()-1];
            if (b-a).perp_dot(p-b)>0. { break; } hull.pop();
        }
        hull.push(p);
    }
    hull.pop();hull
}
fn closest_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let edge=b-a;
    a+edge*((p-a).dot(edge)/edge.length_squared().max(1e-20)).clamp(0.,1.)
}
fn feature_contains(feature: &[Vec2], p: Vec2) -> bool {
    match feature.len() {
        0=>false,
        1=>p.distance_squared(feature[0])<=1e-10,
        2=>p.distance_squared(closest_segment(p,feature[0],feature[1]))<=1e-10,
        _=>(0..feature.len()).all(|i| {
            let a=feature[i];let edge=feature[(i+1)%feature.len()]-a;
            edge.perp_dot(p-a)>=-1e-5*edge.length()
        }),
    }
}
/// The force must act in the overlapping support features. Averaging entire
/// support-face centres puts it outside the overlap for crossing long limbs,
/// introducing a false lever arm that makes joint/contact projection fight.
fn support_contact(a: &Hull,b: &Hull,n: Vec3) -> Vec3 {
    let u=n.any_orthonormal_vector();let v=n.cross(u);
    let high=a.vertices.iter().map(|p|p.dot(n)).fold(f32::NEG_INFINITY,f32::max);
    let low=b.vertices.iter().map(|p|p.dot(n)).fold(f32::INFINITY,f32::min);
    let project=|p: &Vec3|Vec2::new(p.dot(u),p.dot(v));
    let fa=feature_hull(a.vertices.iter().filter(|p| high-p.dot(n)<0.003).map(project).collect());
    let fb=feature_hull(b.vertices.iter().filter(|p| p.dot(n)-low<0.003).map(project).collect());
    let mut intersection=Vec::new();
    let mut add=|p: Vec2| {
        if !intersection.iter().any(|q: &Vec2|q.distance_squared(p)<1e-10) {intersection.push(p);}
    };
    for &p in &fa { if feature_contains(&fb,p) {add(p);} }
    for &p in &fb { if feature_contains(&fa,p) {add(p);} }
    let edges=|len:usize|if len>2 {len} else {len.saturating_sub(1)};
    for i in 0..edges(fa.len()) { for j in 0..edges(fb.len()) {
        let p=fa[i];let r=fa[(i+1)%fa.len()]-p;
        let q=fb[j];let s=fb[(j+1)%fb.len()]-q;
        let denominator=r.perp_dot(s);
        if denominator.abs()<1e-12 {continue;}
        let t=(q-p).perp_dot(s)/denominator;
        let along=(q-p).perp_dot(r)/denominator;
        if (-1e-5..=1.00001).contains(&t) && (-1e-5..=1.00001).contains(&along) {add(p+r*t.clamp(0.,1.));}
    }}
    let point=if !intersection.is_empty() {
        intersection.iter().copied().sum::<Vec2>()/intersection.len() as f32
    } else {
        // Edge/vertex features need not overlap in projection. Use their
        // closest pair rather than unrelated face centres.
        let mut best=(f32::INFINITY,Vec2::ZERO);
        for (left,right) in [(&fa,&fb),(&fb,&fa)] {
            for &p in left {
                for &q in right {let distance=p.distance_squared(q);if distance<best.0 {best=(distance,(p+q)*0.5);}}
                for i in 0..edges(right.len()) {
                    let q=closest_segment(p,right[i],right[(i+1)%right.len()]);let distance=p.distance_squared(q);
                    if distance<best.0 {best=(distance,(p+q)*0.5);}
                }
            }
        }
        best.1
    };
    u*point.x+v*point.y+n*((high+low)*0.5)
}
/// Normal points from A to B. Returns no contact for separated convexes.
#[cfg(test)]
fn convex_contact_scalar(a: &Hull, b: &Hull) -> Option<(Vec3, f32, Vec3)> {
    if a.vertices.is_empty() || b.vertices.is_empty() {
        return None;
    }
    let mut best = (Vec3::ZERO, f32::INFINITY);
    let axes = a
        .normals
        .iter()
        .copied()
        .chain(b.normals.iter().copied())
        .chain(
            a.edges
                .iter()
                .flat_map(|ea| b.edges.iter().map(move |eb| ea.cross(*eb))),
        );
    for axis in axes {
        if axis.length_squared() < 1e-8 {
            continue;
        }
        let n = axis.normalize();
        let interval = |h: &Hull| {
            h.vertices
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p.dot(n)), hi.max(p.dot(n)))
                })
        };
        let (al, ah) = interval(a);
        let (bl, bh) = interval(b);
        if ah < bl - 0.003 || bh < al - 0.003 {
            return None;
        }
        let positive = ah - bl;
        let negative = bh - al;
        let (normal, depth) = if positive <= negative {
            (n, positive)
        } else {
            (-n, negative)
        };
        if depth < best.1 {
            best = (normal, depth);
        }
    }
    if !best.1.is_finite() {
        return None;
    }
    Some((
        best.0,
        best.1,
        support_contact(a,b,best.0),
    ))
}
/// Same SAT axes, ordering, tolerance and support point as the scalar
/// reference. Project four axes together so dense special-infected hulls
/// do not scan every vertex separately for each edge cross product.
pub fn convex_contact(a: &Hull, b: &Hull) -> Option<(Vec3, f32, Vec3)> {
    if a.vertices.is_empty() || b.vertices.is_empty() { return None; }
    let mut axes = a.normals.iter().copied().chain(b.normals.iter().copied())
        .chain(a.edges.iter().flat_map(|ea| b.edges.iter().map(move |eb| ea.cross(*eb))));
    let mut best = (Vec3::ZERO, f32::INFINITY);
    loop {
        let mut batch = [Vec3::ZERO; 4];
        let mut count = 0;
        for axis in &mut batch {
            let Some(next) = axes.next() else { break };
            *axis = next; count += 1;
        }
        if count == 0 { break; }
        let x = Vec4::new(batch[0].x, batch[1].x, batch[2].x, batch[3].x);
        let y = Vec4::new(batch[0].y, batch[1].y, batch[2].y, batch[3].y);
        let z = Vec4::new(batch[0].z, batch[1].z, batch[2].z, batch[3].z);
        let length = x*x + y*y + z*z;
        let inverse = length.max(Vec4::splat(1e-8)).sqrt().recip();
        let (x, y, z) = (x * inverse, y * inverse, z * inverse);
        let interval = |h: &Hull| {
            h.vertices.iter().fold((Vec4::splat(f32::INFINITY), Vec4::splat(f32::NEG_INFINITY)), |(lo, hi), p| {
                let d = x * p.x + y * p.y + z * p.z;
                (lo.min(d), hi.max(d))
            })
        };
        let (al, ah) = interval(a);
        let (bl, bh) = interval(b);
        for lane in 0..count {
            if length[lane] < 1e-8 { continue; }
            if ah[lane] < bl[lane] - 0.003 || bh[lane] < al[lane] - 0.003 { return None; }
            let positive = ah[lane] - bl[lane];
            let negative = bh[lane] - al[lane];
            let n = Vec3::new(x[lane], y[lane], z[lane]);
            let (normal, depth) = if positive <= negative { (n, positive) } else { (-n, negative) };
            if depth < best.1 { best = (normal, depth); }
        }
    }
    if !best.1.is_finite() { return None; }
    Some((best.0, best.1, support_contact(a,b,best.0)))
}
pub fn inverse_box_inertia(mass: f32, half: Vec3, scale: f32) -> Vec3 {
    let squared = half * half;
    let diagonal = Vec3::new(
        squared.y + squared.z,
        squared.x + squared.z,
        squared.x + squared.y,
    ) * mass
        * scale
        / 3.;
    diagonal.max(Vec3::splat(0.000001)).recip()
}
pub fn world_inverse(rotation: Quat, inverse: Vec3) -> Mat3 {
    let r = Mat3::from_quat(rotation);
    r * Mat3::from_diagonal(inverse) * r.transpose()
}
pub fn effective_mass(inverse_mass: f32, inertia: Mat3, arm: Vec3, direction: Vec3) -> f32 {
    inverse_mass + direction.dot((inertia * arm.cross(direction)).cross(arm))
}
pub fn apply(
    linear: &mut Vec3,
    angular: &mut Vec3,
    inverse_mass: f32,
    inertia: Mat3,
    arm: Vec3,
    impulse: Vec3,
) {
    *linear += impulse * inverse_mass;
    *angular += inertia * arm.cross(impulse);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batched_sat_matches_scalar_on_owned_special_infected_hulls() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for name in ["boomer", "smoker", "hunter", "spitter", "jockey", "charger", "hulk"] {
            let profile = crate::physics_data::Profile::load(&mut assets, &format!("models/infected/{name}.mdl")).unwrap();
            let hulls = profile.geometry.iter().flat_map(|s| &s.convexes).map(Hull::native).collect::<Vec<_>>();
            assert!(!hulls.is_empty());
            let centered = |h: &Hull, rotation: Quat, offset: Vec3| {
                let center = h.vertices.iter().copied().sum::<Vec3>() / h.vertices.len() as f32;
                h.transformed(&Transform::from_rotation(rotation).with_translation(offset - rotation * center))
            };
            let pairs = (0..192).map(|i| {
                let a = &hulls[i % hulls.len()];
                let b = &hulls[(i * 7 + 3) % hulls.len()];
                let f = i as f32;
                (centered(a, Quat::from_euler(EulerRot::XYZ, f*0.37, f*0.11, f*0.23), Vec3::ZERO),
                 centered(b, Quat::from_euler(EulerRot::XYZ, f*0.13, f*0.41, f*0.17), Vec3::new((f*1.7).sin(), (f*2.3).cos(), (f*0.7).sin()) * (i % 5) as f32 * 0.07))
            }).collect::<Vec<_>>();
            let start = std::time::Instant::now();
            let reference = pairs.iter().map(|(a,b)| convex_contact_scalar(a,b)).collect::<Vec<_>>();
            let scalar_us = start.elapsed().as_secs_f64() * 1e6;
            let start = std::time::Instant::now();
            let batched = pairs.iter().map(|(a,b)| convex_contact(a,b)).collect::<Vec<_>>();
            let batched_us = start.elapsed().as_secs_f64() * 1e6;
            for (i, (a,b)) in reference.iter().zip(&batched).enumerate() {
                assert_eq!(a.is_some(), b.is_some(), "{name} pair {i}");
                if let (Some((an,ad,ap)), Some((bn,bd,bp))) = (a,b) {
                    assert!((ad-bd).abs() < 0.00001, "{name} pair {i} depth {ad} {bd}");
                    assert!(an.dot(*bn) > 0.9999, "{name} pair {i} normal {an:?} {bn:?}");
                    assert!(ap.distance(*bp) < 0.0001, "{name} pair {i} point {ap:?} {bp:?}");
                }
            }
            println!("SATBENCH {name} hulls={} pairs={} scalar_us={scalar_us:.1} batched_us={batched_us:.1}", hulls.len(), pairs.len());
        }
    }
    fn box_hull(half: Vec3, rotation: Quat, position: Vec3) -> Hull {
        let vertices = [-1., 1.]
            .into_iter()
            .flat_map(|x| {
                [-1., 1.].into_iter().flat_map(move |y| {
                    [-1., 1.]
                        .into_iter()
                        .map(move |z| rotation * (Vec3::new(x, y, z) * half) + position)
                })
            })
            .collect();
        let axes = [Vec3::X, Vec3::Y, Vec3::Z].map(|n| rotation * n).to_vec();
        Hull {
            vertices,
            normals: axes.clone(),
            edges: axes,
        }
    }
    #[test]
    fn crossing_long_faces_place_force_inside_the_actual_overlap() {
        // Owned IVP crossing-box fixture: the static bar meets the far end
        // of the falling bar (Source X in[10,14]), not its centre. Native
        // first-impact impulse gives an effective X lever about13 units.
        let unit=crate::collision::UNIT;
        let a=box_hull(Vec3::new(16.,2.,2.)*unit,Quat::IDENTITY,Vec3::ZERO);
        let b=box_hull(Vec3::new(2.,2.,16.)*unit,Quat::IDENTITY,Vec3::new(12.,-3.9,0.)*unit);
        let (normal,depth,point)=convex_contact(&a,&b).unwrap();
        assert!(normal.dot(Vec3::NEG_Y)>0.9999);
        assert!((depth-0.1*unit).abs()<1e-6);
        assert!((point.x/unit-12.).abs()<1e-4,"{point:?}");
        assert!(point.z.abs()<1e-6);
        let (_,reverse_depth,reverse_point)=convex_contact(&b,&a).unwrap();
        assert!((depth-reverse_depth).abs()<1e-6);
        assert!(point.distance(reverse_point)<1e-6);
    }
    #[test]
    fn overlap_feature_contact_is_invariant_under_world_rotation_and_translation() {
        let a=box_hull(Vec3::new(0.4,0.05,0.05),Quat::IDENTITY,Vec3::ZERO);
        let b=box_hull(Vec3::new(0.05,0.05,0.4),Quat::IDENTITY,Vec3::new(0.3,-0.09,0.));
        let (n,d,p)=convex_contact(&a,&b).unwrap();
        let t=Transform::from_rotation(Quat::from_euler(EulerRot::XYZ,0.37,0.62,-0.29)).with_translation(Vec3::new(3.,2.,-5.));
        let (rotated_n,rotated_d,rotated_p)=convex_contact(&a.transformed(&t),&b.transformed(&t)).unwrap();
        assert!(rotated_n.dot(t.rotation*n)>0.9999);
        assert!((d-rotated_d).abs()<1e-6);
        assert!(rotated_p.distance(t.transform_point(p))<1e-5);
    }
    #[test]
    fn degenerate_support_features_remain_finite_and_contain_their_endpoints() {
        for points in [vec![Vec2::ONE],vec![Vec2::ZERO,Vec2::X,Vec2::X*2.],vec![Vec2::ZERO,Vec2::X,Vec2::Y,Vec2::ONE]] {
            let hull=feature_hull(points.clone());
            assert!(hull.iter().all(|v|v.is_finite()));
            assert!(points.iter().all(|&p|feature_contains(&hull,p)));
        }
    }
    #[test]
    fn rotated_thin_props_do_not_collide_in_empty_aabb_space() {
        let rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let a = box_hull(Vec3::new(2., 0.1, 0.1), rotation, Vec3::ZERO);
        let b = box_hull(Vec3::new(2., 0.1, 0.1), rotation, rotation * Vec3::Y * 0.5);
        assert!(convex_contact(&a, &b).is_none());
        let b = box_hull(Vec3::new(2., 0.1, 0.1), rotation, rotation * Vec3::Y * 0.15);
        let (normal, depth, point) = convex_contact(&a, &b).unwrap();
        assert!((depth - 0.05).abs() < 1e-5);
        assert!(normal.dot(rotation * Vec3::Y) > 0.99);
        assert!(point.is_finite());
    }
    #[test]
    fn contained_shapes_use_exit_distance_and_contact_is_symmetric() {
        let a = box_hull(Vec3::ONE, Quat::IDENTITY, Vec3::ZERO);
        let b = box_hull(Vec3::splat(0.1), Quat::IDENTITY, Vec3::X * 0.5);
        let (n, d, _) = convex_contact(&a, &b).unwrap();
        let (other, reverse, _) = convex_contact(&b, &a).unwrap();
        assert!((d - 0.6).abs() < 1e-5);
        assert!((d - reverse).abs() < 1e-5);
        assert!(n.dot(other) < -0.99);
    }
    #[test]
    fn centred_and_offset_impulses_have_correct_momentum_and_rotation() {
        let inertia = Mat3::from_diagonal(inverse_box_inertia(12., Vec3::ONE, 1.));
        let (mut v, mut w) = (Vec3::ZERO, Vec3::ZERO);
        apply(&mut v, &mut w, 1. / 12., inertia, Vec3::ZERO, Vec3::X * 12.);
        assert!((v.x - 1.).abs() < 0.00001);
        assert_eq!(w, Vec3::ZERO);
        apply(&mut v, &mut w, 1. / 12., inertia, Vec3::Y, Vec3::X * 12.);
        assert!(w.z < -1.);
        assert!(effective_mass(1. / 12., inertia, Vec3::Y, Vec3::X) > 1. / 12.);
    }
}
