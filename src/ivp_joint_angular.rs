//! Angular row preparation and cached impulses from installed 100b9290.
//! Frame coordinates and world integration remain separate; this is not dispatched
//! by the playable articulation solver.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Axis {
    pub motor: bool,
    pub min: f32,
    pub max: f32,
    pub tau: f32,
    pub force: f32,
    pub velocity: f32,
    pub target: f32,
}
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Row {
    pub mode: u32,
    pub jacobian: [[f32; 3]; 2],
    pub response: [[f32; 3]; 2],
    pub k: f32,
    pub inverse: f32,
    pub mask: u32,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoreInput {
    pub flags: u16,
    pub inverse_inertia: [f32; 3],
    pub rotation: [[f32; 3]; 3],
}
pub(super) fn reciprocal_sqrt(x: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    let r = unsafe { std::arch::x86_64::_mm_cvtss_f32(std::arch::x86_64::_mm_rsqrt_ss(std::arch::x86_64::_mm_set_ss(x))) };
    #[cfg(target_arch = "x86")]
    let r = unsafe { std::arch::x86::_mm_cvtss_f32(std::arch::x86::_mm_rsqrt_ss(std::arch::x86::_mm_set_ss(x))) };
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    let r = 1.0 / x.sqrt();
    ((3.0 - (r * r) * x) * r) * 0.5
}

pub(crate) fn prepare_rows(
    cores: &[CoreInput; 2], enabled: [bool; 3],
    directions: [[f32; 3]; 4], rows: &mut [Row; 3],
) -> Result<(), &'static str> {
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        return Err("native reciprocal estimate backend unavailable");
    }
    if rows.iter().any(|r| r.mode > 2) { return Err("invalid angular cache mode"); }
    for index in [0, 2, 1] {
        if rows[index].mode != 0 { continue; }
        let mut world = directions[0];
        if index != 0 {
            let a = directions[2]; let b = directions[if index == 2 { 3 } else { 1 }];
            world = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            let square = dot(world, world);
            if square <= f32::EPSILON {
                rows[index].mode = 2;
                continue;
            }
            // This is a length floor, rather than normalization to unit length.
            let scale = max(reciprocal_sqrt(square) * 0.1, 1.0);
            world = world.map(|x| scale * x);
        }
        let mut row = Row { mode: 1, mask: if enabled[index] { u32::MAX } else { 0 }, ..Default::default() };
        let mut stiffness = [0.0; 2];
        for side in 0..2 {
            let core = cores[side];
            // SUBPS from zero preserves the native signed-zero convention.
            let w = if side == 0 { world } else { world.map(|x| 0.0 - x) };
            let r = core.rotation;
            let j = std::array::from_fn(|i| (w[2] * r[2][i] + w[1] * r[1][i]) + w[0] * r[0][i]);
            row.jacobian[side] = j;
            if core.flags & 0x12 == 0 {
                let inv = core.inverse_inertia;
                row.response[side] = std::array::from_fn(|i| inv[i] * j[i]);
                stiffness[side] = ((inv[1] * j[1]) * j[1] + (inv[0] * j[0]) * j[0]) + (inv[2] * j[2]) * j[2];
            }
        }
        row.k = stiffness[0] + stiffness[1];
        row.inverse = if row.k > f32::EPSILON { reciprocal(row.k) } else { 0.0 };
        rows[index] = row;
    }
    Ok(())
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]
}
// SSE min/max select their second operand for equality and unordered input.
fn min(a: f32, b: f32) -> f32 { if a < b { a } else { b } }
fn max(a: f32, b: f32) -> f32 { if a > b { a } else { b } }
pub(super) fn reciprocal(x: f32) -> f32 {
    // The installed kernel uses RCPPS followed by this Newton step, not DIVPS.
    #[cfg(target_arch = "x86_64")]
    let r = unsafe { std::arch::x86_64::_mm_cvtss_f32(std::arch::x86_64::_mm_rcp_ss(std::arch::x86_64::_mm_set_ss(x))) };
    #[cfg(target_arch = "x86")]
    let r = unsafe { std::arch::x86::_mm_cvtss_f32(std::arch::x86::_mm_rcp_ss(std::arch::x86::_mm_set_ss(x))) };
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    let r = 1.0 / x; // solve rejects unsupported estimate backends first.
    (r + r) - (r * r) * x
}

pub(crate) fn solve(
    angular: &mut [[f32; 3]; 2], axes: &mut [Axis; 3], rows: &[Row; 3],
    coordinates: [f32; 3], factors: [f32; 4], dt: f32, inverse_dt: f32,
) -> Result<(), &'static str> {
    if !cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        return Err("native reciprocal estimate backend unavailable");
    }
    // Reject fresh/invalid rows before changing either body's velocity or
    // any persistent motor target. Mode 2 is the native degenerate swing.
    if rows.iter().any(|r| r.mode != 1 && r.mode != 2) {
        return Err("angular rows require preparation");
    }
    let [alpha, beta, _, delta] = factors;
    for index in [0, 2, 1] {
        let row = rows[index];
        if row.mode == 2 { continue; }
        let axis = &mut axes[index];
        let relative = dot(angular[0], row.jacobian[0]) + dot(angular[1], row.jacobian[1]);
        let coordinate = coordinates[index];
        let mut origin = coordinate;
        let mut impulse = 0.0;
        if axis.motor {
            if index == 0 {
                let mut difference = axis.target - coordinate;
                let wrap = if difference > std::f32::consts::PI { std::f32::consts::TAU }
                    else if difference < -std::f32::consts::PI { -std::f32::consts::TAU } else { 0.0 };
                difference -= wrap;
                origin = coordinate + wrap;
                let desired = (((0.8 * (delta * alpha)) * difference) * inverse_dt
                    - (delta * beta) * relative) * row.inverse;
                // Native multiplies the force budget by dt BEFORE the reciprocal.
                let scale = min(reciprocal(desired).abs() * (axis.force * dt), 1.0);
                impulse = scale * desired;
                axis.target = (axis.velocity * dt + axis.target) - (difference - scale * difference);
            } else {
                let difference = coordinate - axis.target;
                let desired = max((((0.8 * alpha) * difference) * inverse_dt
                    - beta * relative) * row.inverse, f32::EPSILON);
                let scale = min(1.0, reciprocal(desired.abs()) * (axis.force * dt));
                impulse = scale * desired;
                axis.target = (axis.velocity * dt + axis.target) + (difference - scale * difference);
            }
        }
        let predicted = if index == 0 { (row.k * impulse + relative) * dt + origin }
            else { coordinate - (row.k * impulse + relative) * dt };
        let limit_scale = if index == 0 { (axis.tau * (delta * alpha)) * (row.inverse * inverse_dt) }
            else { (axis.tau * alpha) * (row.inverse * inverse_dt) };
        let correction = min((predicted - axis.min) * limit_scale, 0.0)
            + max((predicted - axis.max) * limit_scale, 0.0);
        let correction = f32::from_bits(correction.to_bits() & row.mask);
        impulse = if index == 0 { impulse - correction } else { correction + impulse };
        for side in 0..2 {
            for component in 0..3 {
                angular[side][component] = row.response[side][component] * impulse + angular[side][component];
            }
        }
    }
    Ok(())
}

#[cfg(all(test, any(target_arch = "x86", target_arch = "x86_64")))]
mod tests {
    use super::*;
    use serde_json::Value;
    fn f(v: &Value) -> f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn v(v: &Value) -> [f32; 3] { [f(&v[0]), f(&v[1]), f(&v[2])] }
    #[test]
    fn cached_native_angular_rows_match_all_velocity_and_motor_target_bits() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-angular.json")).unwrap();
        let mut total = 0; let mut motors = 0; let mut skipped = 0;
        for (index, case) in data["calls"].as_array().unwrap().iter().enumerate() {
            let before = &case["before"]; let after = &case["after"];
            let mut angular = [v(&before["cores"][0]["angular"]), v(&before["cores"][1]["angular"])];
            let mut axes: [Axis; 3] = std::array::from_fn(|i| {
                let a = &before["axes"][i];
                Axis { motor: a["motor"].as_u64().unwrap() != 0, min: f(&a["min"]), max: f(&a["max"]),
                    tau: f(&a["tau"]), force: f(&a["force"]), velocity: f(&a["velocity"]), target: f(&a["target"]) }
            });
            let rows = std::array::from_fn(|i| {
                let r = &before["rows"][i]; let mode = r["mode"].as_u64().unwrap() as u32;
                if mode == 2 { skipped += 1; return Row { mode, ..Default::default() }; }
                Row { mode, jacobian: [v(&r["ja"]), v(&r["jb"])], response: [v(&r["ra"]), v(&r["rb"])],
                    k: f(&r["k"]), inverse: f(&r["inverse"]), mask: r["mask"].as_u64().unwrap() as u32 }
            });
            motors += usize::from(axes.iter().any(|a| a.motor));
            solve(&mut angular, &mut axes, &rows, v(&before["coordinates"]),
                std::array::from_fn(|i| f(&before["factors"][i])), f(&case["dt_bits"]), f(&case["inverse_dt_bits"])).unwrap();
            // Compare the entire native packet: only angular velocities and
            // active motor targets may change across this cached callback.
            let mut actual = before.clone();
            for side in 0..2 { actual["cores"][side]["angular"] = serde_json::json!(angular[side].map(f32::to_bits)); }
            for i in 0..3 { actual["axes"][i]["target"] = serde_json::json!(axes[i].target.to_bits()); }
            assert_eq!(&actual, after, "native cached call {index}");
            total += 1;
        }
        assert_eq!((total, motors, skipped), (1236, 1080, 658));
    }
    #[test]
    fn fresh_or_invalid_rows_cannot_partially_change_velocities_or_targets() {
        for mode in [0, 3, u32::MAX] {
            let mut angular = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
            let original = angular;
            let mut axes = [Axis { motor: true, target: 0.25, ..Default::default() }; 3];
            let rows = [Row { mode: 1, ..Default::default() }, Row { mode, ..Default::default() }, Row { mode: 2, ..Default::default() }];
            assert!(solve(&mut angular, &mut axes, &rows, [0.0; 3], [1.0; 4], 1.0 / 60.0, 60.0).is_err());
            assert_eq!(angular, original);
            assert!(axes.iter().all(|a| a.target.to_bits() == 0.25f32.to_bits()));
        }
    }
    #[test]
    fn fresh_native_dynamic_rows_and_initial_impulses_match_every_packet_bit() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-angular.json")).unwrap();
        let mut total = 0;
        for (index, case) in data["fresh_calls"].as_array().unwrap().iter().enumerate() {
            let before = &case["before"]; let after = &case["after"];
            let cores = std::array::from_fn(|i| {
                let c = &before["cores"][i];
                CoreInput { flags: c["flags"].as_u64().unwrap() as u16, inverse_inertia: v(&c["inverse_inertia"]),
                    rotation: std::array::from_fn(|r| std::array::from_fn(|col| f(&c["rotation"][r * 3 + col]))) }
            });
            assert!(cores.iter().all(|c| c.flags & 0x12 == 0), "initial dynamic speed path only");
            let mut axes = std::array::from_fn(|i| {
                let a = &before["axes"][i];
                Axis { motor: a["motor"].as_u64().unwrap() != 0, min: f(&a["min"]), max: f(&a["max"]),
                    tau: f(&a["tau"]), force: f(&a["force"]), velocity: f(&a["velocity"]), target: f(&a["target"]) }
            });
            let mut rows = [Row::default(); 3];
            let enabled = std::array::from_fn(|i| before["axes"][i]["enabled"].as_u64().unwrap() & 1 != 0);
            let directions = std::array::from_fn(|i| std::array::from_fn(|j| f(&before["directions"][i * 4 + j])));
            prepare_rows(&cores, enabled, directions, &mut rows).unwrap();
            let mut actual = before.clone();
            for i in 0..3 {
                let r = rows[i];
                actual["rows"][i] = if r.mode == 2 { serde_json::json!({"mode":2}) } else {
                    serde_json::json!({"mode":1,"ja":r.jacobian[0].map(f32::to_bits),"jb":r.jacobian[1].map(f32::to_bits),
                        "ra":r.response[0].map(f32::to_bits),"rb":r.response[1].map(f32::to_bits),
                        "k":r.k.to_bits(),"inverse":r.inverse.to_bits(),"mask":r.mask})
                };
            }
            assert_eq!(&actual["rows"], &after["rows"], "fresh rows {index}");
            let mut angular = [v(&before["cores"][0]["angular"]), v(&before["cores"][1]["angular"])];
            solve(&mut angular, &mut axes, &rows, v(&before["coordinates"]),
                std::array::from_fn(|i| f(&before["factors"][i])), f(&case["dt_bits"]), f(&case["inverse_dt_bits"])).unwrap();
            for side in 0..2 { actual["cores"][side]["angular"] = serde_json::json!(angular[side].map(f32::to_bits)); }
            for i in 0..3 { actual["axes"][i]["target"] = serde_json::json!(axes[i].target.to_bits()); }
            assert_eq!(&actual, after, "fresh callback {index}");
            total += 1;
        }
        assert_eq!(total, 132);
    }
}
