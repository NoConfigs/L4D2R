//! Initial hinge free-axis limit/motor arithmetic, after native angle calculation.
//! Angle construction (x87 FPATAN), cache construction and group ownership are
//! separate stages; this component accepts the measured coordinate explicitly.
use crate::ivp_joint_angular::{Axis, CoreInput, reciprocal};

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]
}
fn min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
fn max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }

pub(crate) fn solve(
    cores: &[CoreInput; 2], angular: &mut [[f32; 4]; 2], axis: &mut Axis,
    enabled: bool, world: [f32; 3], coordinate: f32,
    dt: f32, inverse_dt: f32, alpha: f32, beta: f32,
) -> Result<(), &'static str> {
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        return Err("native reciprocal estimate backend unavailable");
    }
    if !enabled && !axis.motor { return Ok(()); }
    let mut response = [[0.; 3]; 2];
    let mut k = 0.; let mut relative = 0.;
    for side in 0..2 {
        let c = cores[side];
        let world = if side == 0 { world } else { world.map(|v| 0. - v) };
        let r = c.rotation;
        let j = std::array::from_fn(|i| (world[2] * r[2][i] + world[1] * r[1][i]) + world[0] * r[0][i]);
        if c.flags & 0x12 != 0 { continue; }
        response[side] = std::array::from_fn(|i| c.inverse_inertia[i] * j[i]);
        let body_k = dot(response[side], j);
        let w = angular[side]; let body_relative = dot([w[0], w[1], w[2]], j);
        if side == 0 { k = body_k; relative = body_relative; }
        else { k += body_k; relative += body_relative; }
    }
    let inverse = if k > f32::EPSILON { reciprocal(k) } else { 0. };
    let mut origin = coordinate; let mut impulse = 0.;
    if axis.motor {
        let difference = axis.target - coordinate;
        let pi = f32::from_bits(0x40490fdb); let turn = f32::from_bits(0x40c90fdb);
        let wrap = if difference > pi { turn } else if difference < -pi { -turn } else { 0. };
        let difference = difference - wrap;
        origin = wrap + coordinate;
        let desired = (((alpha * 0.8) * difference) * inverse_dt - beta * relative) * inverse;
        // The native SIMD instructions multiply force*dt before the refined
        // reciprocal. Left-associating the decompiler's flat product differs.
        let ratio = min(f32::from_bits(reciprocal(desired).to_bits() & 0x7fffffff) * (axis.force * dt), 1.);
        impulse = ratio * desired;
        axis.target = (axis.velocity * dt + axis.target) - (difference - ratio * difference);
    }
    let scale = (axis.tau * alpha) * (inverse_dt * inverse);
    let predicted = (k * impulse + relative) * dt + origin;
    let correction = if enabled {
        min((predicted - axis.min) * scale, 0.) + max((predicted - axis.max) * scale, 0.)
    } else { 0. };
    impulse -= correction;
    for side in 0..2 {
        for i in 0..3 { angular[side][i] = impulse * response[side][i] + angular[side][i]; }
        angular[side][3] = 0.;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn native_initial_hinge_limit_motor_matches_angular_and_persistent_target_bits() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-motor.json")).unwrap();
        let f = |v: &Value| f32::from_bits(v.as_u64().unwrap() as u32);
        let mut total = 0; let mut targets_changed = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let p = &call["before"];
            let cores = std::array::from_fn(|side| {
                let c = &p["cores"][side];
                CoreInput { flags: c["flags"].as_u64().unwrap() as u16,
                    inverse_inertia: std::array::from_fn(|i| f(&c["inverse"][i])),
                    rotation: std::array::from_fn(|i| std::array::from_fn(|j| f(&c["rotation"][i * 3 + j]))) }
            });
            let r = cores[0].rotation;
            let v: [f32; 3] = std::array::from_fn(|i| f(&p["geometry"][4 + i]));
            let world = [(r[0][1] * v[1] + r[0][0] * v[0]) + r[0][2] * v[2],
                (r[1][0] * v[0] + r[1][1] * v[1]) + r[1][2] * v[2],
                (r[2][0] * v[0] + r[2][1] * v[1]) + r[2][2] * v[2]];
            let mut angular = std::array::from_fn(|side| std::array::from_fn(|i| f(&p["cores"][side]["angular"][i])));
            let mut axis = Axis { motor: p["motor"] == 1, min: f(&p["axis"][0]), max: f(&p["axis"][1]),
                tau: f(&p["axis"][2]), force: f(&p["axis"][3]), velocity: f(&p["axis"][4]), target: f(&p["axis"][5]) };
            // Deliberate component boundary: native angle is an input here.
            // The independent Python angle hypothesis is not a Rust x87 port.
            solve(&cores, &mut angular, &mut axis, p["limit"] == 1, world, f(&call["angle"]),
                f(&call["dt_bits"]), f(&call["inverse_dt_bits"]), f(&call["p4"]), f(&call["p5"])).unwrap();
            for side in 0..2 {
                assert_eq!(json!(angular[side].map(f32::to_bits)), call["motor_after"]["cores"][side]["angular"], "motor{index}/{side}");
            }
            assert_eq!(axis.target.to_bits() as u64, call["motor_after"]["axis"][5].as_u64().unwrap(), "target{index}");
            targets_changed += usize::from(p["axis"][5] != call["motor_after"]["axis"][5]); total += 1;
        }
        assert_eq!(total, 168); assert!(targets_changed > 50);
    }
}
