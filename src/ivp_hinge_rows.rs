//! Recovered 100b7210 cached hinge rows, 100beb20 application and 100bf5d0
//! matrix inverse. Initial geometry/limit/motor and world dispatch are separate.
pub(crate) use crate::ivp_joint_linear::Core;
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Entry {
    pub linear: [f32; 3],
    pub angular: [f32; 3],
    pub response: [f32; 3],
    pub index: usize,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cache {
    pub bias: [f32; 5],
    pub inverse: [[f32; 8]; 5],
    pub rows: [[Entry; 5]; 2],
    pub relative: [f32; 8],
    pub impulse: [f32; 8],
}
fn dot(a: [f32; 3], b: [f32; 4]) -> f32 {
    (a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]
}
pub(crate) fn solve(cores: &mut [Core; 2], cache: &mut Cache, tau: f32, damping: f32, alpha: f32, beta: f32) -> Result<(), &'static str> {
    solve_phase(cores, cache, tau, damping, alpha, beta, false)
}
pub(crate) fn solve_initial(cores: &mut [Core; 2], cache: &mut Cache, tau: f32, damping: f32, alpha: f32, beta: f32) -> Result<(), &'static str> {
    solve_phase(cores, cache, tau, damping, alpha, beta, true)
}
fn solve_phase(cores: &mut [Core; 2], cache: &mut Cache, tau: f32, damping: f32, alpha: f32, beta: f32, initial: bool) -> Result<(), &'static str> {
    if cache.rows.iter().flatten().any(|r| r.index >= 5) { return Err("invalid five-row hinge index"); }
    let mut relative = [0.; 8];
    for side in 0..2 {
        // Initial 100be650 skips the complete assembly for frozen/static
        // cores. Cached 100b7210 still includes their relative velocities.
        if initial && cores[side].flags & 0x12 != 0 { continue; }
        // Static/frozen cores participate in the relative-speed sum; only
        // the subsequent impulse application is skipped by flags & 0x12.
        for row in cache.rows[side] {
            let angular = dot(row.angular, cores[side].angular);
            let v = cores[side].linear;
            let linear = if initial {
                (row.linear[0] * v[0] + row.linear[1] * v[1]) + row.linear[2] * v[2]
            } else { dot(row.linear, v) };
            let contribution = if initial || side == 1 { linear + angular } else { angular + linear };
            relative[row.index] = contribution + relative[row.index];
        }
    }
    let tau = tau * alpha;
    let damping = (damping * -1.) * beta;
    let rhs: [f32; 5] = std::array::from_fn(|i| relative[i] * damping + cache.bias[i] * tau);
    let impulse = std::array::from_fn(|i| {
        let mut value = cache.inverse[0][i] * rhs[0] + 0.;
        for col in 1..5 { value += cache.inverse[col][i] * rhs[col]; }
        value
    });
    let mut end = *cores;
    for side in 0..2 {
        if end[side].flags & 0x12 != 0 { continue; }
        for row in cache.rows[side] {
            let impulse = impulse[row.index];
            for i in 0..3 { end[side].angular[i] = row.response[i] * impulse + end[side].angular[i]; }
            let scale = impulse * end[side].inverse_mass;
            for i in 0..3 { end[side].linear[i] = row.linear[i] * scale + end[side].linear[i]; }
        }
    }
    *cores = end; cache.relative = relative; cache.impulse = impulse;
    Ok(())
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding { pub bodies: [usize; 2], pub cache: Cache, pub tau: f32, pub damping: f32 }
pub(crate) fn solve_cached_sweeps_with(
    bodies: &mut [Core], bindings: &mut [Binding], additional: u32,
    mut observe: impl FnMut(usize, f32, f32, &[Core], &[Binding]),
) -> Result<(), &'static str> {
    if bindings.iter().any(|b| b.bodies[0] == b.bodies[1] || b.bodies.iter().any(|&i| i >= bodies.len())
        || b.cache.rows.iter().flatten().any(|r| r.index >= 5)) { return Err("invalid cached hinge binding"); }
    // Initial geometry and the once-per-PSI limit/motor pass are inputs to
    // this cached stage. It must not pretend to own the missing first pass.
    for batch in crate::ivp_joint_group::Group::new(additional).batches().skip(1) {
        for slot in 0..bindings.len() {
            let slot = if batch.reverse { bindings.len() - 1 - slot } else { slot };
            let b = &mut bindings[slot]; let ids = b.bodies;
            let mut pair = [bodies[ids[0]], bodies[ids[1]]];
            solve(&mut pair, &mut b.cache, b.tau, b.damping, batch.p4, batch.p5)?;
            bodies[ids[0]] = pair[0]; bodies[ids[1]] = pair[1];
            observe(slot, batch.p4, batch.p5, bodies, bindings);
        }
    }
    Ok(())
}

pub(crate) fn invert(matrix: &mut [[f32; 8]; 5]) -> Result<(), &'static str> {
    let mut a: [[f32; 5]; 5] = std::array::from_fn(|row| std::array::from_fn(|col| matrix[row][col]));
    let mut b: [[f32; 5]; 5] = std::array::from_fn(|row| std::array::from_fn(|col| if row == col { 1. } else { 0. }));
    for i in 0..5 {
        let pivot = a[i][i];
        // Invalid/singular caller inputs are outside the finite native controls.
        // Reject them before committing any part of the caller's matrix.
        if !pivot.is_finite() || pivot == 0. { return Err("invalid hinge matrix pivot"); }
        let reciprocal = 1. / pivot;
        for j in i..5 { a[i][j] *= reciprocal; }
        for j in 0..=i { b[i][j] *= reciprocal; }
        for k in i + 1..5 {
            let factor = a[k][i];
            if factor != 0. {
                for j in 0..=i { b[k][j] -= b[i][j] * factor; }
                for j in i..5 { a[k][j] -= a[i][j] * factor; }
            }
        }
    }
    for i in (0..5).rev() { for k in (0..i).rev() {
        let factor = a[k][i];
        if factor != 0. {
            for j in (0..5).rev() { b[k][j] -= b[i][j] * factor; }
            for j in (i..5).rev() { a[k][j] -= a[i][j] * factor; }
        }
    }}
    // Native writes only the active 5x5 values; the stride-eight padding
    // belongs to the caller and retains its original bits.
    for row in 0..5 { matrix[row][..5].copy_from_slice(&b[row]); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn f(v: &Value) -> f32 { f32::from_bits(v.as_u64().unwrap() as u32) }
    fn core(c: &Value) -> Core {
        Core { flags: c["flags"].as_u64().unwrap() as u16, angular: std::array::from_fn(|i| f(&c["angular"][i])),
            linear: std::array::from_fn(|i| f(&c["linear"][i])), inverse_mass: f(&c["inverse"][3]) }
    }
    fn cache(p: &Value) -> Cache {
        Cache { bias: std::array::from_fn(|i| f(&p["bias"][i])),
            inverse: std::array::from_fn(|row| std::array::from_fn(|col| f(&p["matrix"][row * 8 + col]))),
            rows: std::array::from_fn(|side| std::array::from_fn(|row| {
                let r = &p["rows"][side][row]; Entry { index: r["index"].as_u64().unwrap() as usize,
                    linear: std::array::from_fn(|i| f(&r["linear"][i])), angular: std::array::from_fn(|i| f(&r["angular"][i])), response: std::array::from_fn(|i| f(&r["response"][i])) }
            })), relative: std::array::from_fn(|i| f(&p["relative"][i])), impulse: std::array::from_fn(|i| f(&p["impulse"][i])) }
    }
    fn snapshot(p: &Value, bodies: &[Core; 2], c: &Cache) -> Value {
        let mut actual = p.clone();
        for side in 0..2 {
            actual["cores"][side]["flags"] = json!(bodies[side].flags);
            actual["cores"][side]["inverse"][3] = json!(bodies[side].inverse_mass.to_bits());
            actual["cores"][side]["angular"] = json!(bodies[side].angular.map(f32::to_bits));
            actual["cores"][side]["linear"] = json!(bodies[side].linear.map(f32::to_bits));
        }
        actual["bias"] = json!(c.bias.map(f32::to_bits));
        actual["matrix"] = json!(c.inverse.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());
        actual["relative"] = json!(c.relative.map(f32::to_bits)); actual["impulse"] = json!(c.impulse.map(f32::to_bits));
        actual["rows"] = json!(c.rows.map(|side| side.map(|r| json!({"linear":r.linear.map(f32::to_bits),"angular":r.angular.map(f32::to_bits),"response":r.response.map(f32::to_bits),"index":r.index}))));
        actual
    }
    #[test]
    fn native_cached_hinge_sweeps_keep_source_owned_shared_velocities_and_caches() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-cached.json")).unwrap();
        let all = data["calls"].as_array().unwrap(); let mut compared = 0; let mut groups = 0;
        for case in data["cases"].as_array().unwrap() {
            let scenario = case["scenario"].as_u64().unwrap(); let order = case["order"].as_array().unwrap();
            for tick in 0..3 {
                let expected = all.iter().filter(|c| c["scenario"].as_u64() == Some(scenario) && c["step"].as_u64() == Some(tick)).collect::<Vec<_>>();
                // Seed physical inputs and prepared caches once at the cached
                // stage boundary, after native initial limit/motor/geometry.
                let mut inputs = vec![None; case["count"].as_u64().unwrap() as usize + 1];
                for call in &expected { for c in call["before"]["cores"].as_array().unwrap() {
                    let id = c["id"].as_u64().unwrap() as usize;
                    if inputs[id].is_none() { inputs[id] = Some(core(c)); }
                }}
                let mut bodies = inputs.into_iter().map(Option::unwrap).collect::<Vec<_>>();
                let mut bindings = order.iter().map(|id| {
                    let id = id.as_u64().unwrap(); let seed = expected.iter().find(|c| c["joint"].as_u64() == Some(id)).unwrap(); let p = &seed["before"];
                    Binding { bodies: [id as usize + 1, id as usize], cache: cache(p), tau: f(&p["tau"]), damping: f(&p["damping"]) }
                }).collect::<Vec<_>>();
                let mut index = 0;
                solve_cached_sweeps_with(&mut bodies, &mut bindings, case["authored_iterations"].as_i64().unwrap().max(0) as u32,
                    |slot, alpha, beta, bodies, bindings| {
                        let c = expected[index]; let b = &bindings[slot];
                        assert_eq!(order[slot], c["joint"]);
                        assert_eq!((alpha.to_bits(), beta.to_bits()), (c["p4"].as_u64().unwrap() as u32, c["p5"].as_u64().unwrap() as u32));
                        for side in 0..2 { assert_eq!(b.bodies[side] as u64, c["before"]["cores"][side]["id"].as_u64().unwrap()); }
                        let pair = b.bodies.map(|id| bodies[id]);
                        assert_eq!(snapshot(&c["before"], &pair, &b.cache), c["after"], "hinge group{scenario} tick{tick} callback{index}");
                        index += 1;
                    }).unwrap();
                assert_eq!(index, expected.len()); compared += index;
            }
            groups += 1;
        }
        assert_eq!((groups, compared), (21, 1596));
    }
    #[test]
    fn native_cached_hinge_packets_match_all_five_rows_and_static_core_updates() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-cached.json")).unwrap();
        let mut total = 0; let mut skipped = 0;
        for (index, call) in data["calls"].as_array().unwrap().iter().enumerate() {
            let p = &call["before"];
            let mut cores = std::array::from_fn(|side| {
                let c = &p["cores"][side];
                Core { flags: c["flags"].as_u64().unwrap() as u16,
                    angular: std::array::from_fn(|i| f(&c["angular"][i])), linear: std::array::from_fn(|i| f(&c["linear"][i])), inverse_mass: f(&c["inverse"][3]) }
            });
            skipped += cores.iter().filter(|c| c.flags & 0x12 != 0).count();
            let mut cache = Cache { bias: std::array::from_fn(|i| f(&p["bias"][i])),
                inverse: std::array::from_fn(|row| std::array::from_fn(|col| f(&p["matrix"][row * 8 + col]))),
                rows: std::array::from_fn(|side| std::array::from_fn(|row| {
                    let r = &p["rows"][side][row]; Entry { index: r["index"].as_u64().unwrap() as usize,
                        linear: std::array::from_fn(|i| f(&r["linear"][i])), angular: std::array::from_fn(|i| f(&r["angular"][i])), response: std::array::from_fn(|i| f(&r["response"][i])) }
                })), relative: std::array::from_fn(|i| f(&p["relative"][i])), impulse: std::array::from_fn(|i| f(&p["impulse"][i])) };
            solve(&mut cores, &mut cache, f(&p["tau"]), f(&p["damping"]), f(&call["p4"]), f(&call["p5"])).unwrap();
            let mut actual = p.clone();
            for side in 0..2 {
                actual["cores"][side]["flags"] = json!(cores[side].flags);
                actual["cores"][side]["inverse"][3] = json!(cores[side].inverse_mass.to_bits());
                actual["cores"][side]["angular"] = json!(cores[side].angular.map(f32::to_bits));
                actual["cores"][side]["linear"] = json!(cores[side].linear.map(f32::to_bits));
            }
            actual["bias"] = json!(cache.bias.map(f32::to_bits));
            actual["matrix"] = json!(cache.inverse.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>());
            actual["relative"] = json!(cache.relative.map(f32::to_bits)); actual["impulse"] = json!(cache.impulse.map(f32::to_bits));
            actual["rows"] = json!(cache.rows.map(|side| side.map(|r| json!({"linear":r.linear.map(f32::to_bits),"angular":r.angular.map(f32::to_bits),"response":r.response.map(f32::to_bits),"index":r.index}))));
            assert_eq!(&actual, &call["after"], "cached hinge call{index}"); total += 1;
        }
        assert_eq!((total, skipped), (1596, 180));
    }
    #[test]
    fn native_five_row_inverse_matches_scaled_positive_matrices_and_preserves_padding_bits() {
        let data: Value = serde_json::from_str(include_str!("../tools/fixtures/native-ivp-hinge-inverse.json")).unwrap();
        let mut total = 0;
        for (index, case) in data["cases"].as_array().unwrap().iter().enumerate() {
            let mut matrix = std::array::from_fn(|row| std::array::from_fn(|col| f(&case["before"][row * 8 + col])));
            invert(&mut matrix).unwrap();
            assert_eq!(json!(matrix.into_iter().flatten().map(f32::to_bits).collect::<Vec<_>>()), case["after"], "matrix{index}"); total += 1;
        }
        assert_eq!(total, 128);
    }
    #[test]
    fn invalid_row_indices_and_singular_matrices_cannot_partially_commit() {
        let mut cores = [Core { flags: 0, angular: [1., 2., 3., -0.], linear: [4., 5., 6., -0.], inverse_mass: 1. }; 2];
        let before = cores;
        let mut cache = Cache { bias: [0.; 5], inverse: [[0.; 8]; 5], rows: [[Entry::default(); 5]; 2], relative: [11.; 8], impulse: [12.; 8] };
        cache.rows[1][4].index = 5;
        assert!(solve(&mut cores, &mut cache, 1., 1., 1., 1.).is_err());
        for side in 0..2 {
            assert_eq!(cores[side].angular.map(f32::to_bits), before[side].angular.map(f32::to_bits));
            assert_eq!(cores[side].linear.map(f32::to_bits), before[side].linear.map(f32::to_bits));
        }
        assert_eq!((cache.relative, cache.impulse), ([11.; 8], [12.; 8]));
        let mut matrix = [[-0.; 8]; 5];
        for i in 0..4 { matrix[i][i] = 1.; }
        let original = matrix;
        assert!(invert(&mut matrix).is_err());
        assert_eq!(matrix.map(|r| r.map(f32::to_bits)), original.map(|r| r.map(f32::to_bits)));
    }
}
