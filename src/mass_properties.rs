//! Closed convex-volume integrals in renderer metres. The authored mass and
//! inertia scale remain inputs; this does not reproduce IVP's collision skin.
use bevy::{
    math::{DMat3, DVec3},
    prelude::*,
};
pub struct MassProperties {
    pub center: Vec3,
    pub inverse: Mat3,
    pub volume: f32,
}
/// Owned 100e9a60 uses the compact surface's authored diagonal, not a fresh
/// volume tensor/eigenbasis. Return renderer-local centre and inverse inertia.
pub fn phy_inertia(
    solid: &l4d2_formats::phy::PhySolid,
    mass: f32,
    scale: f32,
    rotational_limit: f32,
) -> Option<Vec3> {
    let p = solid.mass_properties?;
    if !(mass > 0. && mass.is_finite() && scale > 0. && scale.is_finite()
        && (0. ..=1.).contains(&rotational_limit))
        || !p.center_ivp.is_finite() || !p.unit_inertia_ivp.is_finite()
        || p.unit_inertia_ivp.min_element() <= 0.
    {
        return None;
    }
    let mut inertia = (Vec3::from_array(p.unit_inertia_ivp.to_array()) * scale) * mass;
    // Native 100ec3b0 retains the length in x87 precision. The minimum is
    // length(I)*limit, rather than max(I)*limit or a tensor eigenvalue ratio.
    let [x, y, z] = inertia.to_array().map(f64::from);
    let floor = (((z * z + x * x) + y * y).sqrt() * rotational_limit as f64) as f32;
    inertia = inertia.max(Vec3::splat(floor));
    if !inertia.is_finite() || inertia.min_element() <= 0. { return None; }
    Some(inertia)
}
/// Keep the authored bounded inertia available independently of its inverse.
/// Reciprocal round trips are not exact and change gyro/energy calculations.
pub fn from_phy(solid:&l4d2_formats::phy::PhySolid,mass:f32,scale:f32,rotational_limit:f32)->Option<(Vec3,Mat3)>{
    let inertia=phy_inertia(solid,mass,scale,rotational_limit)?;
    let c=solid.mass_properties?.center_ivp;
    Some((Vec3::new(c.x,-c.y,-c.z),Mat3::from_diagonal(inertia.recip())))
}
/// Bound the ratio of principal moments without discarding their orientation.
/// Source's ragdoll setup requests a 0.1 rotational-inertia limit.
pub fn limit_rotational_inertia(inverse: Mat3, ratio: f32) -> Mat3 {
    if !inverse.is_finite() || !(0. ..=1.).contains(&ratio) || ratio == 0. {
        return inverse;
    }
    let mut matrix = DMat3::from_cols(
        inverse.x_axis.as_dvec3(),
        inverse.y_axis.as_dvec3(),
        inverse.z_axis.as_dvec3(),
    );
    let mut basis = DMat3::IDENTITY;
    for _ in 0..24 {
        let (p, q) = [(0usize, 1usize), (0, 2), (1, 2)]
            .into_iter()
            .max_by(|&(p, q), &(a, b)| matrix.col(q)[p].abs().total_cmp(&matrix.col(b)[a].abs()))
            .unwrap();
        let off = matrix.col(q)[p];
        if off.abs()
            <= matrix
                .x_axis
                .x
                .abs()
                .max(matrix.y_axis.y.abs())
                .max(matrix.z_axis.z.abs())
                * 1e-12
        {
            break;
        }
        let tau = (matrix.col(q)[q] - matrix.col(p)[p]) / (2. * off);
        let t = tau.signum() / (tau.abs() + (1. + tau * tau).sqrt());
        let t = if tau == 0. { 1. } else { t };
        let c = 1. / (1. + t * t).sqrt();
        let s = t * c;
        let mut cols = [DVec3::X, DVec3::Y, DVec3::Z];
        cols[p][p] = c;
        cols[p][q] = -s;
        cols[q][p] = s;
        cols[q][q] = c;
        let turn = DMat3::from_cols(cols[0], cols[1], cols[2]);
        matrix = turn.transpose() * matrix * turn;
        basis *= turn;
    }
    let values = DVec3::new(matrix.x_axis.x, matrix.y_axis.y, matrix.z_axis.z);
    let minimum = values.min_element();
    if minimum <= 0. || values.max_element() <= minimum / ratio as f64 {
        return inverse;
    }
    let limited = basis
        * DMat3::from_diagonal(values.min(DVec3::splat(minimum / ratio as f64)))
        * basis.transpose();
    Mat3::from_cols(
        limited.x_axis.as_vec3(),
        limited.y_axis.as_vec3(),
        limited.z_axis.as_vec3(),
    )
}
pub fn from_convexes(
    convexes: &[l4d2_formats::phy::Convex],
    mass: f32,
    scale: f32,
) -> Option<MassProperties> {
    if !(mass > 0. && mass.is_finite() && scale > 0. && scale.is_finite()) {
        return None;
    }
    let mut volume = 0f64;
    let mut first = DVec3::ZERO;
    let mut second = DMat3::ZERO;
    let outer = |v: DVec3| DMat3::from_cols(v * v.x, v * v.y, v * v.z);
    for hull in convexes {
        if !hull.triangles.is_empty() {
            for face in &hull.triangles {
                let [a, b, c] = face.map(|p| {
                    Vec3::from_array(crate::binary::source_position(p.to_array())).as_dvec3()
                });
                let signed = a.dot(b.cross(c)) / 6.;
                let sum = a + b + c;
                volume += signed;
                first += sum * (signed / 4.);
                second += (outer(sum) + outer(a) + outer(b) + outer(c)) * (signed / 20.);
            }
            continue;
        }
        let vertices = crate::rigid_contact::Hull::native(hull).vertices;
        let mut planes = Vec::<(Vec3, f32)>::new();
        for &(normal, distance) in &hull.planes {
            let normal = Vec3::from_array(crate::binary::source_normal(normal.to_array()));
            let length = normal.length();
            if length < 1e-6 {
                continue;
            }
            let n = normal / length;
            let d = distance * crate::collision::UNIT / length;
            if planes
                .iter()
                .any(|(other, dist)| other.dot(n) > 0.999999 && (dist - d).abs() < 0.00001)
            {
                continue;
            }
            planes.push((n, d));
            let mut face = vertices
                .iter()
                .copied()
                .filter(|v| (v.dot(n) - d).abs() < 0.00003)
                .collect::<Vec<_>>();
            if face.len() < 3 {
                continue;
            }
            let center = face.iter().copied().sum::<Vec3>() / face.len() as f32;
            let u = (face[0] - center).normalize_or_zero();
            if u.length_squared() < 0.5 {
                continue;
            }
            let v = n.cross(u);
            face.sort_by(|a, b| {
                let a = *a - center;
                let b = *b - center;
                a.dot(v)
                    .atan2(a.dot(u))
                    .total_cmp(&b.dot(v).atan2(b.dot(u)))
            });
            let a = face[0].as_dvec3();
            for index in 1..face.len() - 1 {
                let b = face[index].as_dvec3();
                let c = face[index + 1].as_dvec3();
                let signed = a.dot(b.cross(c)) / 6.;
                let sum = a + b + c;
                volume += signed;
                first += sum * (signed / 4.);
                second += (outer(sum) + outer(a) + outer(b) + outer(c)) * (signed / 20.);
            }
        }
    }
    if volume.abs() < 1e-10 || !volume.is_finite() {
        return None;
    }
    if volume < 0. {
        volume = -volume;
        first = -first;
        second = -second;
    }
    let center = first / volume;
    let central = second - outer(center) * volume;
    let trace = central.x_axis.x + central.y_axis.y + central.z_axis.z;
    let tensor = (DMat3::IDENTITY * trace - central) * (mass as f64 / volume * scale as f64);
    // Reject open/degenerate geometry rather than producing explosive torque.
    if !tensor.is_finite()
        || tensor.x_axis.x <= 0.
        || tensor.y_axis.y <= 0.
        || tensor.z_axis.z <= 0.
        || tensor.determinant() <= 1e-18
    {
        return None;
    }
    let inv = tensor.inverse();
    Some(MassProperties {
        center: center.as_vec3(),
        inverse: Mat3::from_cols(
            inv.x_axis.as_vec3(),
            inv.y_axis.as_vec3(),
            inv.z_axis.as_vec3(),
        ),
        volume: volume as f32,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_phy_bindings_match_owned_created_cores() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-phy-cores.json")).unwrap();
        let vector = |v: &serde_json::Value| Vec3::new(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32, v[2].as_f64().unwrap() as f32);
        let mut checked=0;let mut round_trip_losses=0;
        for case in fixture["cases"].as_array().unwrap() {
            for body in case["solids"].as_array().unwrap() {
                let solid = l4d2_formats::phy::PhySolid {
                    convexes: vec![], bounds:None, compact_tree:None,mass_properties: Some(l4d2_formats::phy::PhyMassProperties {
                        center_ivp: vector(&body["compact_center_ivp"]).to_array().into(),
                        unit_inertia_ivp: vector(&body["compact_unit_inertia_ivp"]).to_array().into(),
                    }),
                };
                let (center, inverse)=from_phy(&solid,case["mass"].as_f64().unwrap() as f32,
                    case["inertia_scale"].as_f64().unwrap() as f32,case["rot_inertia_limit"].as_f64().unwrap() as f32).unwrap();
                let expected_center=Vec3::from_array(crate::binary::source_position(vector(&body["center_source"]).to_array()));
                assert!(center.distance(expected_center)<0.000001,"{} body{}",case["model"],body["solid"]);
                let authored=phy_inertia(&solid,case["mass"].as_f64().unwrap() as f32,case["inertia_scale"].as_f64().unwrap() as f32,case["rot_inertia_limit"].as_f64().unwrap() as f32).unwrap();
                assert_eq!(authored,vector(&body["core_inertia_ivp"]),"{} body{} original inertia",case["model"],body["solid"]);
                if authored.recip().recip()!=authored{round_trip_losses+=1;}
                let expected=vector(&body["core_inverse_inertia_ivp"]);
                let actual=Vec3::new(inverse.x_axis.x,inverse.y_axis.y,inverse.z_axis.z);
                assert!(((actual-expected).abs()/expected).max_element()<0.000002,
                    "{} body{} actual{actual:?} expected{expected:?}",case["model"],body["solid"]);
                if let Some(angles) = case["angles_source"].as_array() {
                    let angles=angles.iter().map(|a|a.as_f64().unwrap() as f32).collect::<Vec<_>>();
                    let rotation=Mat3::from_quat(Quat::from_euler(EulerRot::YZX,
                        angles[1].to_radians(),-angles[0].to_radians(),angles[2].to_radians()));
                    let source_values=std::array::from_fn::<_,9,_>(|i|body["world_inverse_inertia_source"][i].as_f64().unwrap() as f32);
                    let source_tensor=Mat3::from_cols_array(&source_values).transpose();
                    let source_to_renderer=Mat3::from_cols(Vec3::X,-Vec3::Z,Vec3::Y);
                    let expected_world=source_to_renderer*source_tensor*source_to_renderer.transpose();
                    let actual_world=rotation*inverse*rotation.transpose();
                    let scale=expected_world.to_cols_array().into_iter().map(f32::abs).fold(0f32,f32::max);
                    assert!((actual_world-expected_world).to_cols_array().into_iter().all(|v|v.abs()<scale*0.000003),
                        "{} body{} world inertia frame",case["model"],body["solid"]);
                }
                checked+=1;
            }
        }
        assert_eq!(checked,131);assert_eq!(round_trip_losses,45,"retain original inertia instead of reciprocal reconstruction");
    }
    #[test]
    fn installed_phy_parser_preserves_authored_native_bindings() {
        let root=std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists(){return;}
        let mut assets=crate::source_assets::SourceAssets::new(root,vec![]).unwrap();
        let fixture:serde_json::Value=serde_json::from_str(include_str!("../tools/fixtures/native-ivp-phy-cores.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap().iter().take(5) {
            let data=assets.read(case["model"].as_str().unwrap()).unwrap();
            let solids=l4d2_formats::phy::parse_phy(&data).unwrap();
            assert_eq!(solids.len(),case["solids"].as_array().unwrap().len());
            for (solid, expected) in solids.iter().zip(case["solids"].as_array().unwrap()) {
                let (center,inverse)=from_phy(solid,10.,1.,0.1).unwrap();
                let v=|key:&str|Vec3::from_array(std::array::from_fn(|i|expected[key][i].as_f64().unwrap() as f32));
                assert!(center.distance(Vec3::from_array(crate::binary::source_position(v("center_source").to_array())))<0.000001);
                let actual=Vec3::new(inverse.x_axis.x,inverse.y_axis.y,inverse.z_axis.z);
                assert!(((actual-v("core_inverse_inertia_ivp")).abs()/actual).max_element()<0.000002);
            }
        }
    }
    #[test]
    fn ragdoll_inertia_limit_preserves_principal_axes_and_well_conditioned_tensors() {
        let original = Mat3::from_diagonal(Vec3::new(1., 100., 50.));
        let expected = Mat3::from_diagonal(Vec3::new(1., 10., 10.));
        let rotation = Mat3::from_quat(Quat::from_euler(EulerRot::XYZ, 0.4, -0.7, 0.2));
        for r in [Mat3::IDENTITY, rotation] {
            let actual = limit_rotational_inertia(r * original * r.transpose(), 0.1);
            let expected = r * expected * r.transpose();
            assert!(actual.is_finite());
            assert!(actual.determinant() > 0.);
            assert!(
                actual
                    .to_cols_array()
                    .iter()
                    .zip(expected.to_cols_array())
                    .all(|(a, b)| (*a - b).abs() < 0.0001)
            );
        }
        let isotropic = Mat3::IDENTITY * 3.;
        assert_eq!(limit_rotational_inertia(isotropic, 0.1), isotropic);
    }
    fn cube(center: l4d2r_math::Vec3, half: f32) -> l4d2_formats::phy::Convex {
        let v = l4d2r_math::Vec3::new;
        let mut vertices = Vec::new();
        for x in [-half, half] {
            for y in [-half, half] {
                for z in [-half, half] {
                    vertices.push(center + v(x, y, z));
                }
            }
        }
        let mut edges = Vec::new();
        for i in 0..8 {
            for j in i + 1..8 {
                if ((i ^ j) as u32).count_ones() == 1 {
                    edges.push((vertices[i], vertices[j]));
                }
            }
        }
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut n = l4d2r_math::Vec3::ZERO;
            n[axis] = 1.;
            planes.push((n, center[axis] + half));
            planes.push((-n, -center[axis] + half));
        }
        l4d2_formats::phy::Convex {
            topology: None,
            triangle_headers: vec![],
            triangles: vec![],
            planes,
            edges,
            bevels: vec![],
            mins: center - l4d2r_math::Vec3::splat(half),
            maxs: center + l4d2r_math::Vec3::splat(half),
        }
    }
    #[test]
    fn translated_cube_matches_analytic_tensor_and_authored_scale() {
        let half = 1. / crate::collision::UNIT;
        let shape = cube(l4d2r_math::Vec3::new(70., -25., 13.), half);
        let p = from_convexes(&[shape.clone()], 12., 1.).unwrap();
        assert!((p.volume - 8.).abs() < 0.0001);
        assert!(
            p.center
                .distance(Vec3::from_array(crate::binary::source_position([
                    70., -25., 13.
                ])))
                < 0.00001
        );
        assert!(
            (p.inverse - Mat3::IDENTITY * 0.125)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < 0.00001)
        );
        let scaled = from_convexes(&[shape], 12., 2.).unwrap();
        assert!(
            (scaled.inverse - p.inverse * 0.5)
                .to_cols_array()
                .iter()
                .all(|v| v.abs() < 0.00001)
        );
    }
    #[test]
    fn compound_volume_uses_parallel_axis_inertia() {
        let unit = crate::collision::UNIT;
        let a = cube(l4d2r_math::Vec3::new(-2. / unit, 0., 0.), 1. / unit);
        let b = cube(l4d2r_math::Vec3::new(2. / unit, 0., 0.), 1. / unit);
        let p = from_convexes(&[a, b], 24., 1.).unwrap();
        assert!(p.center.length() < 0.00001);
        assert!((p.volume - 16.).abs() < 0.0001);
        assert!((p.inverse.x_axis.x - 1. / 16.).abs() < 0.00001);
        assert!((p.inverse.y_axis.y - 1. / 112.).abs() < 0.00001);
        assert!(from_convexes(&[], 1., 1.).is_none());
    }
}
