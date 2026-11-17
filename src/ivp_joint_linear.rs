//! Installed 100ba210 after its angular callback: three coupled anchor rows.
//! Cache construction and initial/cached impulse orders are recovered here.
//! World/controller/contact ownership and playable dispatch remain separate.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Core {
    pub flags: u16,
    pub angular: [f32; 4],
    pub linear: [f32; 4],
    pub inverse_mass: f32,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cache {
    pub error: [f32; 4],
    pub jacobian: [[[f32; 4]; 3]; 2],
    pub response: [[[f32; 4]; 3]; 2],
    pub inverse: [[f32; 4]; 3],
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub flags: u16,
    /// Core-local angular inverse inertia, then linear inverse mass.
    pub inverse: [f32; 4],
    pub rotation: [[f32; 3]; 3],
    pub position: [f32; 3],
    pub object_offset: Option<[f32; 3]>,
    pub anchor: [f32; 3],
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase { Initial, Cached }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn translate(rotation: [[f32; 3]; 3], vector: [f32; 3], origin: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| ((vector[2] * rotation[i][2] + origin[i]) + vector[1] * rotation[i][1]) + vector[0] * rotation[i][0])
}
pub(crate) fn prepare(bodies: &[Geometry; 2]) -> Cache {
    let anchors: [[f32; 3]; 2] = std::array::from_fn(|side| {
        let b = bodies[side];
        let origin = b.object_offset.map_or(b.position, |offset| translate(b.rotation, offset, b.position));
        translate(b.rotation, b.anchor, origin)
    });
    let mut cache = Cache { error: [0.; 4], jacobian: [[[0.; 4]; 3]; 2],
        response: [[[0.; 4]; 3]; 2], inverse: [[0.; 4]; 3] };
    for i in 0..3 { cache.error[i] = anchors[1][i] - anchors[0][i]; }
    let mut k = [[0.; 4]; 3];
    for side in 0..2 {
        let b = bodies[side];
        if b.flags & 0x12 != 0 { continue; }
        // Keep the world anchor addition followed by subtraction. Algebraic
        // cancellation of the core position changes the native lever bits.
        let lever = std::array::from_fn(|i| anchors[side][i] - b.position[i]);
        for col in 0..3 {
            let axis = std::array::from_fn(|i| b.rotation[i][col]);
            let v = if side == 0 { cross(axis, lever) } else { cross(lever, axis) };
            cache.jacobian[side][col] = [v[0], v[1], v[2], 0.];
        }
        let padding = if side == 0 { cross([0.; 3], lever) } else { cross(lever, [0.; 3]) };
        for col in 0..3 {
            for i in 0..3 { cache.response[side][col][i] = b.inverse[i] * cache.jacobian[side][i][col]; }
            cache.response[side][col][3] = b.inverse[3] * padding[col];
        }
        for col in 0..3 { for i in 0..4 {
            let response = cache.response[side][col];
            let jacobian = cache.jacobian[side];
            k[col][i] = ((response[2] * jacobian[2][i] + k[col][i]) + response[1] * jacobian[1][i]) + response[0] * jacobian[0][i];
        }}
        // Native 100b8dd0 adds each body's mass to the diagonal separately.
        for i in 0..3 { k[i][i] += b.inverse[3]; }
    }
    let [a, b, c] = k.map(|v| [v[0], v[1], v[2]]);
    let cofactor = cross(a, b);
    let determinant = (c[1] * cofactor[1] + c[0] * cofactor[0]) + c[2] * cofactor[2];
    let reciprocal = crate::ivp_joint_angular::reciprocal(determinant);
    // Installed symmetric inverse stores these cofactor vectors directly,
    // with Z duplicated in W. A generic Mat3 inverse changes the operation
    // order and assumes a different packed layout.
    for (col, v) in [cross(b, c), cross(c, a), cofactor].into_iter().enumerate() {
        let v = v.map(|x| x * reciprocal);
        cache.inverse[col] = [v[0], v[1], v[2], v[2]];
    }
    cache
}
fn multiply(columns: [[f32; 4]; 3], vector: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| (vector[2] * columns[2][i] + vector[1] * columns[1][i]) + vector[0] * columns[0][i])
}
pub(crate) fn solve(
    cores: &mut [Core; 2], enabled: bool, cache: Option<&Cache>,
    damping: f32, tau: f32, alpha: f32, beta: f32, inverse_dt: f32, phase: Phase,
) -> Result<(), &'static str> {
    // Native disabled constraints do not read the unprepared linear cache.
    if !enabled { return Ok(()); }
    let cache = cache.ok_or("linear joint cache is not prepared")?;
    let bias = (tau * alpha) * inverse_dt;
    let damping = damping * beta;
    let rhs = std::array::from_fn(|i| {
        let a = cores[0]; let b = cores[1];
        let relative_b = (((0.0 - b.linear[i]) + b.angular[2] * cache.jacobian[1][2][i])
            + b.angular[1] * cache.jacobian[1][1][i]) + b.angular[0] * cache.jacobian[1][0][i];
        let relative_a = ((a.angular[2] * cache.jacobian[0][2][i] + a.linear[i])
            + a.angular[1] * cache.jacobian[0][1][i]) + a.angular[0] * cache.jacobian[0][0][i];
        // Native ANDPS masks the error vector's W before multiplying by bias.
        match phase {
            Phase::Cached => {
                let error = if i < 3 { cache.error[i] } else { 0.0 };
                bias * error - (relative_b + relative_a) * damping
            }
            Phase::Initial => {
                // Initial 100baba0 accumulates A before subtracting B, and
                // adds bias to the negated damping term. Cached 100ba210
                // uses the separate B/A sums above.
                let relative = (((relative_a - b.linear[i]) + b.angular[2] * cache.jacobian[1][2][i])
                    + b.angular[1] * cache.jacobian[1][1][i]) + b.angular[0] * cache.jacobian[1][0][i];
                (0.0 - relative * damping) + bias * cache.error[i]
            }
        }
    });
    let impulse = multiply(cache.inverse, rhs);
    for (side, core) in cores.iter_mut().enumerate() {
        if core.flags & 0x12 != 0 { continue; }
        let angular = multiply(cache.response[side], impulse);
        for i in 0..4 {
            let w = angular[i] + core.angular[i];
            let direction = if side == 0 { impulse[i] } else { 0.0 - impulse[i] };
            let v = core.inverse_mass * direction + core.linear[i];
            core.angular[i] = if i < 3 { w } else { 0.0 };
            core.linear[i] = if i < 3 { v } else { 0.0 };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn native_cached_anchor_impulses_match_all_dynamic_static_and_disabled_packet_bits() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-linear.json")).unwrap();
        let f = |v: &Value| f32::from_bits(v.as_u64().unwrap() as u32);
        let vector = |v: &Value| std::array::from_fn(|i| f(&v[i]));
        let mut active = 0; let mut disabled = 0; let mut static_skips = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let input = &call["before"];
            let mut cores = std::array::from_fn(|side| {
                let c = &input["cores"][side];
                Core { flags: c["flags"].as_u64().unwrap() as u16, angular: vector(&c["angular"]),
                    linear: vector(&c["linear"]), inverse_mass: f(&c["inverse"][3]) }
            });
            let enabled = input["enabled"].as_u64().unwrap() != 0;
            let cache = if enabled {
                active += 1;
                static_skips += cores.iter().filter(|c| c.flags & 0x12 != 0).count();
                let block = |offset: usize| std::array::from_fn(|col| std::array::from_fn(|i| f(&input["cache"][offset + col * 4 + i])));
                Some(Cache { error: std::array::from_fn(|i| f(&input["cache"][i])),
                    jacobian: [block(4), block(16)], response: [block(28), block(40)], inverse: block(52) })
            } else { disabled += 1; assert!(input["cache"].is_null()); None };
            solve(&mut cores, enabled, cache.as_ref(), f(&input["damping"]), f(&input["tau"]),
                f(&input["factors"][0]), f(&input["factors"][1]), f(&call["inverse_dt_bits"]), Phase::Cached).unwrap();
            let mut actual = input.clone();
            for side in 0..2 {
                actual["cores"][side]["angular"] = json!(cores[side].angular.map(f32::to_bits));
                actual["cores"][side]["linear"] = json!(cores[side].linear.map(f32::to_bits));
            }
            assert_eq!(&actual, &call["after"], "native linear callback {index}");
        }
        assert_eq!((active, disabled), (1032, 204));
        assert_eq!(static_skips, data["active_static_skips"].as_u64().unwrap() as usize);
        assert!(static_skips > 0);
    }
    #[test]
    fn missing_active_cache_cannot_partially_commit_and_disabled_cache_is_never_required() {
        let mut cores = [Core { flags: 0, angular: [1., 2., 3., -0.], linear: [4., 5., 6., -0.], inverse_mass: 0.5 }; 2];
        let original = cores;
        assert!(solve(&mut cores, true, None, 1., 1., 1., 1., 60., Phase::Cached).is_err());
        assert!(solve(&mut cores, false, None, f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN, Phase::Cached).is_ok());
        for side in 0..2 {
            assert_eq!(cores[side].angular.map(f32::to_bits), original[side].angular.map(f32::to_bits));
            assert_eq!(cores[side].linear.map(f32::to_bits), original[side].linear.map(f32::to_bits));
        }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn native_anchor_geometry_and_both_solve_orders_match_displaced_mass_centre_packets() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-joint-linear-prepare.json")).unwrap();
        let f = |v: &Value| f32::from_bits(v.as_u64().unwrap() as u32);
        let vector = |v: &Value| std::array::from_fn(|i| f(&v[i]));
        let mut prepared = 0; let mut initial = 0; let mut offset = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let input = &call["before"];
            let mut cores = std::array::from_fn(|side| {
                let c = &input["cores"][side];
                Core { flags: c["flags"].as_u64().unwrap() as u16, angular: vector(&c["angular"]),
                    linear: vector(&c["linear"]), inverse_mass: f(&c["inverse"][3]) }
            });
            let enabled = input["enabled"].as_u64().unwrap() != 0;
            let cache = if enabled {
                let geometry = std::array::from_fn(|side| {
                    let c = &input["cores"][side];
                    let object_offset = if c["object_flags"].as_u64().unwrap() & 0x800 == 0 {
                        offset += 1; Some(std::array::from_fn(|i| f(&c["offset"][i])))
                    } else { None };
                    Geometry { flags: cores[side].flags, inverse: vector(&c["inverse"]),
                        rotation: std::array::from_fn(|row| std::array::from_fn(|col| f(&c["rotation"][row * 3 + col]))),
                        position: std::array::from_fn(|i| f(&c["position"][i])), object_offset,
                        anchor: std::array::from_fn(|i| f(&c["anchor"][i])) }
                });
                prepared += 1; Some(prepare(&geometry))
            } else { assert!(input["cache"].is_null()); None };
            let mut actual = input.clone();
            if let Some(cache) = cache.as_ref() {
                let mut packed = cache.error.to_vec();
                packed.extend(cache.jacobian.into_iter().flatten().flatten());
                packed.extend(cache.response.into_iter().flatten().flatten());
                packed.extend(cache.inverse.into_iter().flatten());
                actual["cache"] = json!(packed.into_iter().map(f32::to_bits).collect::<Vec<_>>());
                assert_eq!(&actual["cache"], &input["cache"], "prepared cache {index}");
            }
            let phase = if call["phase"] == "initial" { initial += 1; Phase::Initial } else { Phase::Cached };
            solve(&mut cores, enabled, cache.as_ref(), f(&input["damping"]), f(&input["tau"]),
                f(&input["factors"][0]), f(&input["factors"][1]), f(&call["inverse_dt_bits"]), phase).unwrap();
            for side in 0..2 {
                actual["cores"][side]["angular"] = json!(cores[side].angular.map(f32::to_bits));
                actual["cores"][side]["linear"] = json!(cores[side].linear.map(f32::to_bits));
            }
            assert_eq!(&actual, &call["after"], "prepared linear packet {index}");
        }
        assert_eq!((prepared, initial), (1137, 105));
        assert!(offset > 0);
    }
}
