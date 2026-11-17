//! Owned retail client 10224210 / 10222720 and server 1032ab90.
//! Native model split and random ranges; shared IVP contacts remain reconstructed.
use bevy::prelude::*;
pub(crate) const PARTS: [&str; 5] = [
    "models/infected/limbs/exploded_boomer_head.mdl",
    "models/infected/limbs/exploded_boomer_rarm.mdl",
    "models/infected/limbs/exploded_boomer_steak1.mdl",
    "models/infected/limbs/exploded_boomer_steak2.mdl",
    "models/infected/limbs/exploded_boomer_steak3.mdl",
];
pub(crate) fn corpse(model: &str) -> Option<&'static str> {
    match model {
        "models/infected/boomer.mdl" | "models/infected/boomer_l4d1.mdl" => Some("models/infected/limbs/exploded_boomer.mdl"),
        "models/infected/boomette.mdl" => Some("models/infected/limbs/exploded_boomette.mdl"),
        _ => None,
    }
}
pub(crate) fn paths() -> impl Iterator<Item = String> {
    PARTS.into_iter().chain(["models/infected/limbs/exploded_boomer.mdl", "models/infected/limbs/exploded_boomette.mdl"])
        .map(str::to_owned)
}
pub(crate) fn forces(rng: &mut l4d2r_game_core::random::UniformRandomStream) -> [Vec3; 5] {
    let mut result = [Vec3::new(0., 0., 100.); 5];
    for force in &mut result[1..] {
        let x = rng.random_int(-100, 100) as f32;
        let y = rng.random_int(-100, 100) as f32;
        let z = rng.random_int(25, 75) as f32;
        *force = Vec3::new(x, y, z);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_boomer_split_uses_twelve_integer_draws_and_fixed_head_force() {
        use l4d2r_game_core::random::UniformRandomStream;
        for seed in [0, 1, 42, 9273, -19] {
            let mut actual = UniformRandomStream::new(seed);
            let mut expected = actual.clone();
            let result = forces(&mut actual);
            assert_eq!(result[0], Vec3::new(0., 0., 100.));
            for force in &result[1..] {
                assert_eq!(force.x, expected.random_int(-100, 100) as f32);
                assert_eq!(force.y, expected.random_int(-100, 100) as f32);
                assert_eq!(force.z, expected.random_int(25, 75) as f32);
            }
            assert_eq!(actual.random_int(0, 100000), expected.random_int(0, 100000));
        }
        assert!(corpse("models/infected/boomette.mdl").unwrap().ends_with("exploded_boomette.mdl"));
        assert!(corpse(corpse("models/infected/boomer.mdl").unwrap()).is_none());
        assert!(corpse("models/infected/smoker.mdl").is_none());
    }
}
