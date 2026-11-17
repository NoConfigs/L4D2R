//! Recovered 100baba0 world directions and angular coordinates. These inputs
//! are core rotation matrices and already-authored local joint frames;
//! constructor selection and quaternion-to-matrix conversion are separate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Coordinates {
    pub value: [f32; 3],
    /// Bisector, reference X, attached X, reference Z (native workspace order).
    pub directions: [[f32; 4]; 4],
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(crate) fn transform(rotation: [[f32; 3]; 3], vector: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| (vector[2] * rotation[i][2] + vector[1] * rotation[i][1]) + vector[0] * rotation[i][0])
}
pub(crate) fn from_core_frames(rotations: [[[f32; 3]; 3]; 2], local: [[[f32; 3]; 3]; 2]) -> Coordinates {
    from_world(transform(rotations[0], local[0][0]), transform(rotations[0], local[0][2]),
        transform(rotations[1], local[1][0]), transform(rotations[1], local[1][2]))
}
pub(crate) fn from_world(reference_x: [f32; 3], reference_z: [f32; 3], attached_x: [f32; 3], attached_z: [f32; 3]) -> Coordinates {
    let sum = std::array::from_fn(|i| attached_x[i] + reference_x[i]);
    let sum_square = dot(sum, sum);
    let bisector_inverse = crate::ivp_joint_angular::reciprocal_sqrt(sum_square);
    let bisector = sum.map(|x| bisector_inverse * x);
    let perpendicular = cross(bisector, reference_z);
    let square = dot(perpendicular, perpendicular);
    let inverse = crate::ivp_joint_angular::reciprocal_sqrt(square);
    let mask = if square > f32::EPSILON { u32::MAX } else { 0 };
    let perpendicular = perpendicular.map(|x| f32::from_bits((inverse * x).to_bits() & mask));
    let twist = 0.0 - crate::ivp_joint_angle::native_atan2(dot(attached_z, perpendicular), dot(cross(perpendicular, bisector), attached_z));
    let mut directions = [bisector, reference_x, attached_x, reference_z].map(|v| [v[0], v[1], v[2], 0.0]);
    // 10181520=[1,1,1,0.5]; W blends the squared sum instead of its
    // fourth vector component, then becomes the angular correction factor.
    directions[0][3] = (bisector_inverse * 0.5) * sum_square;
    Coordinates { value: [twist, dot(attached_x, reference_x), dot(attached_x, reference_z)], directions }
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
mod tests {
    use super::*;
    #[test]
    fn native_frame_matrices_reconstruct_every_captured_direction_and_coordinate_bit() {
        let data: serde_json::Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-frame.json")).unwrap();
        let mut count = 0;
        for (index, case) in data["calls"].as_array().unwrap().iter().enumerate() {
            let input = &case["before"];
            let f = |value: &serde_json::Value| f32::from_bits(value.as_u64().unwrap() as u32);
            let rotations = std::array::from_fn(|side| std::array::from_fn(|row| std::array::from_fn(|col|
                f(&input["cores"][side]["rotation"][row * 3 + col]))));
            let local = std::array::from_fn(|side| std::array::from_fn(|axis| std::array::from_fn(|component|
                f(&input["frames"][side * 12 + axis * 4 + component]))));
            let result = from_core_frames(rotations, local);
            for i in 0..3 {
                assert_eq!(result.value[i].to_bits(), input["coordinates"][i].as_u64().unwrap() as u32, "call{index} coordinate{i}");
            }
            for axis in 0..4 { for component in 0..4 {
                assert_eq!(result.directions[axis][component].to_bits(), input["directions"][axis * 4 + component].as_u64().unwrap() as u32,
                    "call{index} direction{axis}/{component}");
            }}
            count += 1;
        }
        assert_eq!(count, 1368);
    }
    #[test]
    fn native_frames_then_angular_preparation_and_impulses_match_complete_dynamic_packets() {
        use crate::ivp_joint_angular::{Axis, CoreInput, Row, prepare_rows, solve};
        use serde_json::{Value, json};
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-frame.json")).unwrap();
        let f = |v: &Value| f32::from_bits(v.as_u64().unwrap() as u32);
        let vec = |v: &Value| [f(&v[0]), f(&v[1]), f(&v[2])];
        let mut count = 0; let mut fresh = 0;
        for (index, case) in data["calls"].as_array().unwrap().iter().enumerate() {
            let input = &case["before"];
            let rotations = std::array::from_fn(|side| std::array::from_fn(|row| std::array::from_fn(|col|
                f(&input["cores"][side]["rotation"][row * 3 + col]))));
            let local = std::array::from_fn(|side| std::array::from_fn(|axis| std::array::from_fn(|component|
                f(&input["frames"][side * 12 + axis * 4 + component]))));
            let coordinates = from_core_frames(rotations, local);
            let cores = std::array::from_fn(|side| {
                let c = &input["cores"][side];
                CoreInput { flags: c["flags"].as_u64().unwrap() as u16, rotation: rotations[side], inverse_inertia: vec(&c["inverse_inertia"]) }
            });
            assert!(cores.iter().all(|c| c.flags & 0x12 == 0), "dynamic initial-speed path only");
            let mut axes = std::array::from_fn(|i| {
                let a = &input["axes"][i];
                Axis { motor: a["motor"].as_u64().unwrap() != 0, min: f(&a["min"]), max: f(&a["max"]),
                    tau: f(&a["tau"]), force: f(&a["force"]), velocity: f(&a["velocity"]), target: f(&a["target"]) }
            });
            let mut rows = std::array::from_fn(|i| {
                let r = &input["rows"][i]; let mode = r["mode"].as_u64().unwrap() as u32;
                if mode != 1 { return Row { mode, ..Default::default() }; }
                Row { mode, jacobian: [vec(&r["ja"]), vec(&r["jb"])], response: [vec(&r["ra"]), vec(&r["rb"])],
                    k: f(&r["k"]), inverse: f(&r["inverse"]), mask: r["mask"].as_u64().unwrap() as u32 }
            });
            fresh += usize::from(rows.iter().any(|r| r.mode == 0));
            let enabled = std::array::from_fn(|i| input["axes"][i]["enabled"].as_u64().unwrap() & 1 != 0);
            let xyz = coordinates.directions.map(|v| [v[0], v[1], v[2]]);
            prepare_rows(&cores, enabled, xyz, &mut rows).unwrap();
            let mut angular = [vec(&input["cores"][0]["angular"]), vec(&input["cores"][1]["angular"])];
            // Alpha/beta/gamma remain dispatcher inputs; W comes from the
            // reconstructed frame, rather than the native expected factor.
            let mut factors = std::array::from_fn(|i| f(&input["factors"][i]));
            factors[3] = coordinates.directions[0][3];
            solve(&mut angular, &mut axes, &rows, coordinates.value, factors,
                f(&case["dt_bits"]), f(&case["inverse_dt_bits"])).unwrap();
            let mut actual = input.clone();
            actual["coordinates"] = json!(coordinates.value.map(f32::to_bits));
            actual["directions"] = json!(coordinates.directions.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());
            actual["factors"][3] = json!(factors[3].to_bits());
            for side in 0..2 { actual["cores"][side]["angular"] = json!(angular[side].map(f32::to_bits)); }
            for i in 0..3 {
                actual["axes"][i]["target"] = json!(axes[i].target.to_bits());
                let r = rows[i];
                actual["rows"][i] = if r.mode == 2 { json!({"mode":2}) } else {
                    json!({"mode":1,"ja":r.jacobian[0].map(f32::to_bits),"jb":r.jacobian[1].map(f32::to_bits),
                        "ra":r.response[0].map(f32::to_bits),"rb":r.response[1].map(f32::to_bits),
                        "k":r.k.to_bits(),"inverse":r.inverse.to_bits(),"mask":r.mask})
                };
            }
            assert_eq!(&actual, &case["after"], "chained native dynamic call{index}");
            count += 1;
        }
        assert_eq!((count, fresh), (1368, 132));
    }
}
