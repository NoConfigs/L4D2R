//! Initial 100b7660 five-row cache construction. The once-per-PSI free-axis
//! limit/motor pass and initial velocity solve are separate from geometry.
use crate::ivp_hinge_rows::{Cache, Entry};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub core: crate::ivp_joint_linear::Geometry,
    pub axis: [f32; 3],
}

// Scalar 100bfbd0 orders X's first two products differently from Y/Z.
// Both differ from the SIMD anchor transform used by the ragdoll callback.
pub(super) fn transform(r: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        (r[0][1] * v[1] + r[0][0] * v[0]) + r[0][2] * v[2],
        (r[1][0] * v[0] + r[1][1] * v[1]) + r[1][2] * v[2],
        (r[2][0] * v[0] + r[2][1] * v[1]) + r[2][2] * v[2],
    ]
}
pub(super) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}
pub(crate) fn prepare(bodies: &[Geometry; 2], perpendicular_a: [f32; 3], inverse_dt: f32) -> Result<Cache, &'static str> {
    let anchors: [[f32; 3]; 2] = std::array::from_fn(|side| {
        let b = bodies[side].core;
        // 100eda40 constructs an object origin before 100bfb30 transforms
        // the local joint anchor. Preserve both rounded position additions.
        let origin = b.object_offset.map_or(b.position, |offset| std::array::from_fn(|i| {
            ((b.rotation[i][0] * offset[0] + b.rotation[i][1] * offset[1])
                + b.rotation[i][2] * offset[2]) + b.position[i]
        }));
        let rotated = transform(b.rotation, b.anchor);
        std::array::from_fn(|i| rotated[i] + origin[i])
    });
    let axis = transform(bodies[0].core.rotation, bodies[0].axis);
    let attached = transform(bodies[1].core.rotation, bodies[1].axis);
    let perpendicular = transform(bodies[0].core.rotation, perpendicular_a);
    let second = cross(perpendicular, axis);
    let mut cache = Cache { bias: [0.; 5], inverse: [[0.; 8]; 5],
        rows: [[Entry::default(); 5]; 2], relative: [0.; 8], impulse: [0.; 8] };
    for i in 0..3 { cache.bias[i] = (anchors[1][i] - anchors[0][i]) * inverse_dt; }
    cache.bias[3] = dot(attached, second) * inverse_dt;
    cache.bias[4] = -(dot(attached, perpendicular) * inverse_dt);
    for side in 0..2 {
        let b = bodies[side].core;
        let [x, y, z] = std::array::from_fn(|i| anchors[side][i] - b.position[i]);
        let rt = std::array::from_fn(|i| std::array::from_fn(|j| b.rotation[j][i]));
        let angular = if side == 0 {
            [[0., z, -y], [-z, 0., x], [y, -x, 0.], perpendicular, second]
        } else {
            [[0., z * -1., y], [z, 0., x * -1.], [y * -1., x, 0.],
                perpendicular.map(|v| v * -1.), second.map(|v| v * -1.)]
        };
        for i in 0..5 {
            let angular = transform(rt, angular[i]);
            cache.rows[side][i] = Entry {
                linear: std::array::from_fn(|j| if i < 3 && i == j { if side == 0 { 1. } else { -1. } } else { 0. }),
                angular, index: i,
                // Native leaves response storage untouched for skipped cores.
                // It is unread by matrix assembly/application; canonicalize
                // our newly owned cache rather than importing stale scratch.
                response: if b.flags & 0x12 != 0 { [0.; 3] }
                    else { std::array::from_fn(|j| angular[j] * b.inverse[j]) },
            };
        }
    }
    // 100be650: form the angular dot product first, then add the mass term.
    // Flattening the decompiled expression into a left-associated sum fails
    // native controls even when every Jacobian/response bit is correct.
    for side in 0..2 {
        let b = bodies[side].core;
        if b.flags & 0x12 != 0 { continue; }
        for i in 0..5 { for j in 0..=i {
            let current = cache.rows[side][i]; let previous = cache.rows[side][j];
            let a = previous.angular; let r = current.response;
            let angular = if i == 4 && j < 4 {
                (a[0] * r[0] + a[1] * r[1]) + a[2] * r[2]
            } else { (a[1] * r[1] + a[0] * r[0]) + a[2] * r[2] };
            let contribution = dot(current.linear, previous.linear) * b.inverse[3] + angular;
            cache.inverse[j][i] = contribution + cache.inverse[j][i];
            if i != j { cache.inverse[i][j] = contribution + cache.inverse[i][j]; }
        }}
    }
    crate::ivp_hinge_rows::invert(&mut cache.inverse)?;
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn native_initial_hinge_geometry_rebuilds_all_five_rows_bias_and_inverse() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-initial.json")).unwrap();
        let f = |v: &Value| f32::from_bits(v.as_u64().unwrap() as u32);
        let mut count = 0; let mut offset_cores = 0; let mut static_cores = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let p = &call["before"];
            let bodies = std::array::from_fn(|side| {
                let c = &p["cores"][side];
                let offset = c["object_flags"].as_u64().unwrap() & 0x800 == 0;
                offset_cores += usize::from(offset);
                static_cores += usize::from(c["flags"].as_u64().unwrap() & 0x12 != 0);
                Geometry { core: crate::ivp_joint_linear::Geometry {
                    flags: c["flags"].as_u64().unwrap() as u16,
                    inverse: std::array::from_fn(|i| f(&c["inverse"][i])),
                    rotation: std::array::from_fn(|i| std::array::from_fn(|j| f(&c["rotation"][i * 3 + j]))),
                    position: std::array::from_fn(|i| f(&c["position"][i])),
                    object_offset: offset.then(|| std::array::from_fn(|i| f(&c["offset"][i]))),
                    anchor: std::array::from_fn(|i| f(&p["geometry"][side * 8 + i])),
                }, axis: std::array::from_fn(|i| f(&p["geometry"][side * 8 + 4 + i])) }
            });
            let cache = prepare(&bodies, std::array::from_fn(|i| f(&p["geometry"][16 + i])), f(&call["inverse_dt_bits"])).unwrap();
            assert_eq!(json!(cache.bias.map(f32::to_bits)), call["after"]["bias"], "bias{index}");
            assert_eq!(json!(cache.inverse.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>()), call["after"]["matrix"], "inverse{index}");
            let rows = cache.rows.map(|side| side.map(|row| json!({"linear":row.linear.map(f32::to_bits),
                "angular":row.angular.map(f32::to_bits),"index":row.index,"response":row.response.map(f32::to_bits)})));
            for side in 0..2 { for row in 0..5 {
                let mut actual = rows[side][row].clone();
                if bodies[side].core.flags & 0x12 != 0 { actual["response"] = Value::Null; }
                assert_eq!(actual, call["after"]["rows"][side][row], "rows{index}/{side}/{row}");
            }}
            count += 1;
        }
        assert_eq!(count, 168); assert!(offset_cores > 200); assert!(static_cores > 20);
    }
}
