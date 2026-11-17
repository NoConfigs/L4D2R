//! Authored pellet counts/scatter and distance attenuation; retail spread shape (deterministic local seed).
use crate::game_state::Weapon;
use bevy::prelude::*;
#[derive(Clone, Copy, Default)]
pub struct Accuracy {
    pub duck: f32,
    pub stand: f32,
    pub air: f32,
    pub movement: f32,
    pub per_shot: f32,
    pub maximum: f32,
    pub decay: f32,
}
impl Accuracy {
    /// The spread a gun settles to (server 103e8e60), in degrees. On the
    /// ground: the stance minimum plus the movement share, speed capped at
    /// 220 units/s and scaled by 0.75 standing or 0.5 crouched; in the air:
    /// MaxMovementSpread. Incapacitated (not the Magnum) adds
    /// survivor_incapacitated_accuracy_penalty 0.4 x 10, a tongue drag
    /// tongue_victim_accuracy_penalty 0.133 x 10; a laser sight scales it by
    /// upgrade_laser_sight_spread_factor 0.4.
    pub fn target(self, grounded: bool, duck: bool, speed: f32, penalties: Penalties) -> f32 {
        let (min, factor) = if grounded {
            (
                if duck && !penalties.incap { self.duck } else { self.stand },
                speed.clamp(0., 220.) * if duck { 0.5 } else { 0.75 },
            )
        } else {
            (self.air, 220.)
        };
        let mut target = (self.movement - min) * factor / 220. + min;
        if penalties.incap && !penalties.magnum {
            target += 0.4 * 10.;
        }
        if penalties.dragged {
            target += 0.133 * 10.;
        }
        if penalties.laser {
            target *= 0.4;
        }
        target
    }
    /// One tick of the spread easing toward `target` (SpreadDecay per
    /// second, clamped to 0..90).
    pub fn ease(self, spread: f32, target: f32, dt: f32) -> f32 {
        (spread - self.decay * dt * (spread - target)).clamp(0., 90.)
    }
    /// The spread after a shot (103e7090): + SpreadPerShot (x0.4 with a
    /// laser sight), capped at MaxSpread.
    pub fn after_shot(self, spread: f32, laser: bool) -> f32 {
        (spread + self.per_shot * if laser { 0.4 } else { 1. }).min(self.maximum)
    }
}
/// Conditions that raise or lower a gun's spread.
#[derive(Clone, Copy, Default)]
pub struct Penalties {
    pub incap: bool,
    pub magnum: bool,
    pub dragged: bool,
    pub laser: bool,
}
pub fn directions(weapon: &Weapon, rotation: Quat, shot: u64) -> Vec<Vec3> {
    directions_with_spread(weapon, rotation, shot, 0.)
}
/// A laser sight changes the spread value (Accuracy::target/after_shot),
/// not the pellet pattern (FireBullet 103c7470 ignores upgrades).
pub fn directions_with_upgrade(
    weapon: &Weapon,
    rotation: Quat,
    shot: u64,
    cone: f32,
    _laser: bool,
) -> Vec<Vec3> {
    directions_with_spread(weapon, rotation, shot, cone)
}
/// Retail shot directions. CTerrorGun (server 103e9370) offsets the aim by
/// a uniform random pitch and yaw within the current spread; every pellet
/// shares that aim. In FireBullet (103c7470) the first pellet flies exactly
/// there and each later one adds a polar offset: a radius uniform in
/// +-PelletScatterYaw at a uniform angle, with the pitch part scaled by
/// PelletScatterPitch / PelletScatterYaw.
pub fn directions_with_spread(weapon: &Weapon, rotation: Quat, shot: u64, cone: f32) -> Vec<Vec3> {
    let mut seed = (shot as u32).wrapping_mul(747796405).wrapping_add(2891336453);
    let mut random = |lo: f32, hi: f32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        lo + (seed as f32 / u32::MAX as f32) * (hi - lo)
    };
    let (mut pitch, mut yaw) = (0., 0.);
    if cone > 0. {
        yaw = random(-cone, cone);
        pitch = random(-cone, cone);
    }
    let (scatter_pitch, scatter_yaw) = (weapon.scatter.x, weapon.scatter.y);
    let ratio = if scatter_pitch != 0. && scatter_yaw != 0. { scatter_pitch / scatter_yaw } else { 1. };
    (0..weapon.pellets.max(1))
        .map(|pellet| {
            let (mut p, mut y) = (pitch, yaw);
            if pellet > 0 && scatter_yaw != 0. {
                let radius = random(-scatter_yaw, scatter_yaw);
                let angle = random(0., 180.) * 2. * std::f32::consts::PI / 180.;
                p += angle.sin() * radius * ratio;
                y += angle.cos() * radius;
            }
            rotation * Quat::from_euler(EulerRot::YXZ, y.to_radians(), p.to_radians(), 0.) * Vec3::NEG_Z
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_spread_targets_and_easing() {
        let a = Accuracy { duck: 0.05, stand: 0.4, air: 1.5, movement: 5., per_shot: 1., maximum: 30., decay: 5. };
        let none = Penalties::default();
        assert_eq!(a.target(true, false, 0., none), 0.4);
        assert_eq!(a.target(true, true, 0., none), 0.05);
        // Full speed standing: 0.75 of the movement share.
        assert!((a.target(true, false, 220., none) - (0.4 + 4.6 * 0.75)).abs() < 1e-5);
        assert!((a.target(true, false, 400., none) - a.target(true, false, 220., none)).abs() < 1e-6);
        assert_eq!(a.target(false, false, 0., none), 5.);
        let incap = Penalties { incap: true, ..none };
        assert!((a.target(true, true, 0., incap) - 4.4).abs() < 1e-5);
        assert!((a.target(true, false, 0., Penalties { laser: true, ..none }) - 0.16).abs() < 1e-6);
        assert_eq!(a.after_shot(29.5, false), 30.);
        assert!((a.after_shot(0., true) - 0.4).abs() < 1e-6);
        // Easing: SpreadDecay x dt of the gap per tick.
        assert!((a.ease(10., 0.4, 0.1) - (10. - 0.5 * 9.6)).abs() < 1e-5);
    }
    #[test]
    fn laser_leaves_the_pellet_pattern_alone() {
        let mut weapon = crate::game_state::tests_weapon();
        weapon.pellets = 8;
        weapon.scatter = Vec2::splat(5.);
        assert_eq!(
            directions_with_upgrade(&weapon, Quat::IDENTITY, 42, 4., false),
            directions_with_upgrade(&weapon, Quat::IDENTITY, 42, 4., true)
        );
    }
    #[test]
    fn pellet_spread_is_stable_normalized_and_bounded() {
        let mut w = crate::game_state::tests_weapon();
        w.pellets = 10;
        w.scatter = Vec2::splat(5.);
        let rays = directions(&w, Quat::IDENTITY, 12);
        assert_eq!(rays.len(), 10);
        assert_eq!(rays, directions(&w, Quat::IDENTITY, 12));
        assert!(
            rays.iter()
                .all(|r| (r.length() - 1.).abs() < 0.001 && r.dot(Vec3::NEG_Z) > 0.98)
        );
        assert_ne!(rays[0], rays[1]);
    }
    #[test]
    fn retail_first_pellet_is_centred_and_scatter_is_a_disc() {
        let mut w = crate::game_state::tests_weapon();
        w.pellets = 10;
        w.scatter = Vec2::new(5., 10.);
        for shot in 0..50 {
            let rays = directions(&w, Quat::IDENTITY, shot);
            assert_eq!(rays[0], Vec3::NEG_Z);
            for r in &rays[1..] {
                // Yaw within the yaw scatter, pitch within half of it.
                let yaw = r.x.atan2(-r.z).to_degrees().abs();
                let pitch = r.y.asin().to_degrees().abs();
                assert!(yaw <= 10.001 && pitch <= 5.001, "{yaw} {pitch}");
                assert!((yaw / 10.).powi(2) + (pitch / 5.).powi(2) <= 1.01);
            }
        }
    }
}
