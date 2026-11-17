//! Recovered ragdoll frame setup and angular coordinates. The rest of the
//! local contact/deactivation solver is still reconstructed, not native IVP.
use bevy::prelude::*;
use crate::articulated::{Joint, Rigid};

#[derive(Clone, Debug)]
pub(crate) struct Frames {
    pub reference: Mat3,
    pub attached: Mat3,
    pub min: Vec3,
    pub max: Vec3,
    pub source_axes: [usize; 3],
}

fn inertia(body: &Rigid) -> Mat3 {
    let rotation = Mat3::from_quat(body.rotation);
    rotation * body.inverse_inertia * rotation.transpose()
}

impl Frames {
    pub fn prepare(j: &Joint, a: &Rigid, b: &Rigid) -> Option<Self> {
        // Native CreateRagdollConstraint uses a hinge kernel for one free
        // angular axis. That kernel remains separate from this ragdoll port.
        let free = (0..3).filter(|&axis| j.max[axis] > j.min[axis]).count();
        if free == 1 { return None; }
        // Native IVP local vectors -> renderer local vectors. Both engines
        // conjugate Source orientations, but with opposite +/-90deg X bases.
        let basis = Mat3::from_diagonal(Vec3::new(1., -1., -1.));
        let attached_base = Mat3::from_quat(j.frame) * basis;
        let reference_base = basis;
        if free == 0 {
            // 100bd600 case zero keeps the authored frames and the identity
            // permutation. Its default twist/plane rows are locked at zero,
            // but its cone lower bound is cos(0.1), not a rigid weld.
            // Owned setup readback: [[0,0],[0.995004177,100],[0,0]].
            return Some(Self {
                reference: reference_base,
                attached: attached_base,
                min: Vec3::new(0., 0.995004177, 0.),
                max: Vec3::new(0., 100., 0.),
                source_axes: [0, 2, 1],
            });
        }
        // Source clockwise convention, axis table 1007c6e0=[0,2,1].
        let low = Vec3::new(-j.max.x, j.min.z, -j.max.y);
        let high = Vec3::new(-j.min.x, j.max.z, -j.min.y);
        let span = high - low;
        let ia = inertia(a);
        let ib = inertia(b);
        let ra = a.rotation * j.anchor_a;
        let rb = b.rotation * j.anchor_b;
        let mut twist = 0;
        let mut best = -1.;
        // 100bd600 / 100be650: angular inverse mass plus the anchor lever
        // contributions select the preferred twist axis. Strict > keeps the
        // first axis on exact ties, as in the owned callback.
        for axis in 0..3 {
            let na = a.rotation * attached_base.col(axis);
            let nb = b.rotation * reference_base.col(axis);
            let score = na.dot(ia * na) + nb.dot(ib * nb)
                + ra.cross(na).length_squared() * a.inverse_mass
                + rb.cross(nb).length_squared() * b.inverse_mass;
            if score > best { best = score; twist = axis; }
        }
        let mut plane = (twist + 1) % 3;
        let mut cone = (twist + 2) % 3;
        if span[cone] < span[plane] { std::mem::swap(&mut cone, &mut plane); }
        let center = (low[cone] + high[cone]) * 0.5;
        let mut rotation_axis = Vec3::ZERO;
        rotation_axis[cone] = 1.;
        let centered = reference_base * Mat3::from_quat(Quat::from_axis_angle(rotation_axis, center));
        let permutation = [twist, cone, plane];
        // Frame columns are twist/plane/cone; the separate limit permutation
        // is twist/cone/plane. Confusing the two swaps the swing Jacobians.
        let select = |m: Mat3| Mat3::from_cols(m.col(twist), m.col(plane), m.col(cone));
        let source = [0, 2, 1];
        Some(Self {
            reference: select(centered), attached: select(attached_base),
            min: Vec3::new(-high[twist], (span[cone] * 0.5).cos(), -high[plane].sin()),
            max: Vec3::new(-low[twist], 100., -low[plane].sin()),
            source_axes: permutation.map(|axis| source[axis]),
        })
    }

    /// (native coordinate, world impulse axis, coordinate speed / angular speed)
    pub fn rows(&self, a: &Rigid, b: &Rigid) -> [(f32, Vec3, f32); 3] {
        let rotation = |q: Quat| {
            let m = Mat3::from_quat(q);
            std::array::from_fn(|i| [m.x_axis[i], m.y_axis[i], m.z_axis[i]])
        };
        let transform = crate::ivp_joint_frame::transform;
        let reference_x = transform(rotation(b.rotation), self.reference.x_axis.to_array());
        let reference_z = transform(rotation(b.rotation), self.reference.z_axis.to_array());
        let attached_x = transform(rotation(a.rotation), self.attached.x_axis.to_array());
        let attached_z = transform(rotation(a.rotation), self.attached.z_axis.to_array());
        let coordinates = crate::ivp_joint_frame::from_world(reference_x, reference_z, attached_x, attached_z);
        let bisector = Vec3::new(coordinates.directions[0][0], coordinates.directions[0][1], coordinates.directions[0][2]);
        let cone = Vec3::from_array(attached_x).cross(Vec3::from_array(reference_x));
        let plane = Vec3::from_array(attached_x).cross(Vec3::from_array(reference_z));
        // 100b9290 scales each swing cross product by max(1, 0.1/length).
        // With unit impulse axes this is an effective Jacobian length floor,
        // not a full normalization. The native epsilon applies to length squared.
        let swing = |axis: Vec3| {
            let square = axis.length_squared();
            if square <= f32::EPSILON { (Vec3::ZERO, 0.) }
            else {
                let length = square.sqrt();
                (axis / length, length.max(0.1))
            }
        };
        let (cone_axis, cone_factor) = swing(cone);
        let (plane_axis, plane_factor) = swing(plane);
        // An exactly opposed authored X pair has no bisector. Keep that
        // undefined native fragment out of the reconstructed local solver;
        // its clamp cannot accept NaNs. Degenerate retail group recovery is
        // still separate from the finite native controls above.
        let twist = if coordinates.value[0].is_finite() && bisector.is_finite() {
            (coordinates.value[0], -bisector, 1.)
        } else { (0., Vec3::ZERO, 0.) };
        [twist,
            (coordinates.value[1], cone_axis, cone_factor),
            (coordinates.value[2], plane_axis, plane_factor)]
    }
}

/// Installed 100ba0d0 operation order. XMM0 is y, XMM1 is x.
/// The estimate backend is native SSE on x86; other architectures retain a
/// division fallback whose bit parity is not established.
pub(super) fn native_atan2(y:f32,x:f32)->f32 {crate::ivp_joint_angle::native_atan2(y,x)}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exactly_opposed_authored_axes_do_not_send_nan_twist_rows_to_the_local_solver() {
        let body = |rotation| Rigid {
            bone: 0, position: Vec3::ZERO, rotation, velocity: Vec3::ZERO, angular: Vec3::ZERO,
            center: Vec3::ZERO, inverse_mass: 1., inverse_inertia: Mat3::IDENTITY,
            vertices: vec![], hulls: vec![], damping: 0., angular_damping: 0.,
            surface: Default::default(), world_contacts: vec![], pose_cache: None,
        };
        // Exact half-turn, rather than sin/cos(pi), makes the X pair cancel
        // identically. Exercise the actual live Frames::rows adapter.
        let a = body(Quat::from_xyzw(0., 0., 1., 0.));
        let b = body(Quat::IDENTITY);
        let frames = Frames { reference: Mat3::IDENTITY, attached: Mat3::IDENTITY,
            min: Vec3::ZERO, max: Vec3::ZERO, source_axes: [0, 1, 2] };
        let rows = frames.rows(&a, &b);
        assert!(rows.iter().all(|(coordinate, axis, factor)| coordinate.is_finite() && axis.is_finite() && factor.is_finite()));
        assert_eq!(rows[0], (0., Vec3::ZERO, 0.));
    }
    #[test]
    fn native_simd_angle_helper_matches_all_finite_capture_bits() {
        let data: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-atan.json")).unwrap();
        let mut count = 0;
        for (index, case) in data["cases"].as_array().unwrap().iter().enumerate() {
            for lane in 0..4 {
                let input = |key: &str| f32::from_bits(case[key][lane].as_u64().unwrap() as u32);
                assert_eq!(native_atan2(input("y"), input("x")).to_bits(),
                    case["angle"][lane].as_u64().unwrap() as u32, "case{index} lane{lane}");
                count += 1;
            }
        }
        assert_eq!(count, 12288);
    }
    #[test]
    fn native_atan_quadrants_and_range_reduction() {
        for y in -20..=20 { for x in -20..=20 {
            if x==0 && y==0 { continue; }
            let y=y as f32/7.; let x=x as f32/7.;
            assert!((native_atan2(y,x)-y.atan2(x)).abs()<0.0001,"{y} {x}");
        }}
    }
}
