//! Bot visual acquisition and aim reconstructed from the owned retail binary.
//! Hearing, shared knowledge and full target priorities remain separate gaps.
use crate::{collision::UNIT, game_state::Weapon};
use bevy::prelude::*;
pub const VISION_RANGE: f32 = 1500. * UNIT;
/// sb_close_threat_range (200). Retail's threat scan (server 103a8850 /
/// 103a8620) takes known infected inside this range with a clear line of
/// sight whatever way the bot faces. A running or attacking infected that
/// close is always known: it is heard inside sb_near_hearing_range (500,
/// OnSound 103a2270).
pub const CLOSE_THREAT_RANGE: f32 = 200. * UNIT;
const VISION_COS_HALF_FOV: f32 = std::f32::consts::FRAC_1_SQRT_2;
// Survivor vision reset (103a73e0): RandomFloat(0.2, 0.3), retained
// per bot. Use an independent deterministic draw for reproducible local play;
// this is not the retail engine's shared RNG stream.
fn recognition_delay(index: usize) -> f32 {
    let seed = (index as u32)
        .wrapping_add(1)
        .wrapping_mul(747796405)
        .wrapping_add(2891336453);
    let seed = (seed ^ (seed >> 16)).wrapping_mul(2246822519);
    0.2 + 0.1 * ((seed ^ (seed >> 13)) as f64 / u32::MAX as f64) as f32
}
const SACCADE: f32 = 1000.;
// Owned server.dll FUN_104ba0c0: on-target dot threshold and saccade
// slowdown constants. See bot-aim-native-evidence-1016.txt.
const ON_TARGET: f32 = 0.98;
fn turn_towards(before: Vec3, desired: Vec3, dt: f32) -> Vec3 {
    if before == Vec3::ZERO {
        return desired;
    }
    let dot = before.dot(desired).clamp(-1., 1.);
    let rate = if dot > 0.7 {
        let scale = 1. + (dot - 0.7) * (0.02 - 1.) / (1. - 0.7);
        (scale * 1.57).sin() * SACCADE
    } else {
        SACCADE
    };
    let step = rate.to_radians() * dt.max(0.);
    let yaw = |v: Vec3| v.x.atan2(-v.z);
    let pitch = |v: Vec3| v.y.clamp(-1., 1.).asin();
    let yaw_delta = (yaw(desired) - yaw(before) + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    let y = yaw(before) + yaw_delta.clamp(-step, step);
    let p = pitch(before) + (pitch(desired) - pitch(before)).clamp(-step, step);
    Vec3::new(y.sin() * p.cos(), p.sin(), -y.cos() * p.cos())
}
#[derive(Default)]
struct Track {
    clock: f32,
    target: Option<u64>,
    acquired: f32,
    bloom: f32,
    recognition_delay: f32,
}
#[derive(Default)]
pub struct Aim(Vec<Track>);
pub fn in_view(delta: Vec3, facing: Vec3, obscured: bool) -> bool {
    // survivor_vision_range_obscured=750; normal range=1500 Source units.
    let range = if obscured {
        VISION_RANGE * 0.5
    } else {
        VISION_RANGE
    };
    delta.length_squared() <= range * range
        && facing.normalize_or_zero().dot(delta.normalize_or_zero()) >= VISION_COS_HALF_FOV
}
impl Aim {
    pub fn reset(&mut self) {
        self.0.clear();
    }
    pub fn tracking(&self, index: usize) -> bool {
        self.0.get(index).is_some_and(|t| t.target.is_some())
    }
    pub fn advance(&mut self, index: usize, dt: f32, weapon: &Weapon) {
        while self.0.len() <= index {
            self.0.push(Track {
                recognition_delay: recognition_delay(self.0.len()),
                ..default()
            });
        }
        let t = &mut self.0[index];
        t.clock += dt;
        t.bloom = weapon.accuracy.ease(t.bloom, weapon.accuracy.target(true, false, 0., Default::default()), dt);
    }
    pub fn forget(&mut self, index: usize) {
        if let Some(t) = self.0.get_mut(index) {
            t.target = None;
        }
    }
    /// Called only for a visible, spawned target. Hidden coordinates never
    /// enter the aim state; losing sight requires a new acquisition.
    pub fn observe(
        &mut self,
        index: usize,
        target: u64,
        delta: Vec3,
        aim: &mut Vec3,
        dt: f32,
    ) -> bool {
        let t = &mut self.0[index];
        if t.target != Some(target) {
            t.target = Some(target);
            t.acquired = t.clock;
        }
        let desired = delta.normalize_or_zero();
        let before = aim.normalize_or_zero();
        *aim = turn_towards(before, desired, dt);
        t.clock - t.acquired + 1e-5 >= t.recognition_delay && aim.dot(desired) > ON_TARGET
    }
    pub fn rays(
        &self,
        index: usize,
        weapon: &Weapon,
        aim: Vec3,
        serial: u64,
        speed: f32,
        laser: bool,
    ) -> Vec<Vec3> {
        // `speed` is the fraction of the gun's run speed.
        let units = speed * weapon.speed / crate::collision::UNIT;
        let target = weapon.accuracy.target(true, false, units, crate::ballistics::Penalties { laser, ..Default::default() });
        let cone = self.0[index].bloom.max(target);
        crate::ballistics::directions_with_upgrade(
            weapon,
            Quat::from_rotation_arc(Vec3::NEG_Z, aim.normalize_or_zero()),
            serial ^ ((index as u64).wrapping_mul(0x9e3779b97f4a7c15)),
            cone,
            laser,
        )
    }
    pub fn fired(&mut self, index: usize, weapon: &Weapon) {
        let t = &mut self.0[index];
        t.bloom = weapon.accuracy.after_shot(t.bloom, false);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_visual_cone_has_no_close_range_rear_exception() {
        for degrees in [0f32, 30., 44.9] {
            let delta = Quat::from_rotation_y(degrees.to_radians()) * Vec3::NEG_Z * 10.;
            assert!(in_view(delta, Vec3::NEG_Z, false), "{degrees}");
        }
        for degrees in [45.1f32, 60., 89., 180.] {
            let delta = Quat::from_rotation_y(degrees.to_radians()) * Vec3::NEG_Z;
            assert!(!in_view(delta, Vec3::NEG_Z, false), "{degrees}");
        }
        assert!(!in_view(Vec3::Y, Vec3::NEG_Z, false));
    }
    #[test]
    fn recognition_uses_per_bot_native_interval_and_restarts_on_new_target() {
        let w = crate::game_state::tests_weapon();
        let mut tracker = Aim::default();
        let dt = 1. / 120.;
        let mut delays = Vec::new();
        for index in 0..3 {
            tracker.advance(index, 0., &w);
            let delay = tracker.0[index].recognition_delay;
            assert!((0.2..=0.3).contains(&delay));
            delays.push(delay);
            let mut direction = Vec3::NEG_Z;
            assert!(!tracker.observe(index, 1, direction, &mut direction, dt));
            let mut elapsed = 0.;
            while elapsed + dt < delay - 1e-5 {
                tracker.advance(index, dt, &w);
                elapsed += dt;
                assert!(!tracker.observe(index, 1, Vec3::NEG_Z, &mut direction, dt));
            }
            tracker.advance(index, dt, &w);
            assert!(tracker.observe(index, 1, Vec3::NEG_Z, &mut direction, dt));
            assert!(!tracker.observe(index, 2, Vec3::NEG_Z, &mut direction, dt));
            assert_eq!(tracker.0[index].recognition_delay, delay);
        }
        assert!(delays.windows(2).all(|p| p[0] != p[1]));
    }
    #[test]
    fn native_turn_slows_near_target_and_wraps_yaw() {
        let dt = 1. / 60.;
        let forward = Vec3::NEG_Z;
        let far = turn_towards(forward, Vec3::X, dt);
        assert!((forward.angle_between(far).to_degrees() - SACCADE * dt).abs() < 0.001);
        let near = Quat::from_rotation_y(10f32.to_radians()) * forward;
        let advanced = turn_towards(forward, near, dt);
        assert!(forward.angle_between(advanced).to_degrees() < 2.);
        assert!(advanced.dot(near) > forward.dot(near));
        let left = Quat::from_rotation_y(179f32.to_radians()) * forward;
        let right = Quat::from_rotation_y(-179f32.to_radians()) * forward;
        assert!(turn_towards(left, right, dt).dot(Vec3::Z) > 0.999);
        assert!(turn_towards(forward, Vec3::X, 0.).abs_diff_eq(forward, 1e-6));
    }
    #[test]
    fn bile_reduces_detection_range_and_laser_reduces_bot_spread() {
        let delta = Vec3::NEG_Z * 1000. * UNIT;
        assert!(in_view(delta, Vec3::NEG_Z, false));
        assert!(!in_view(delta, Vec3::NEG_Z, true));
        assert!(in_view(Vec3::NEG_Z * 749. * UNIT, Vec3::NEG_Z, true));
        let mut w = crate::game_state::tests_weapon();
        w.accuracy.stand = 5.;
        let mut aim = Aim::default();
        aim.advance(0, 0., &w);
        for serial in 1..20 {
            let normal = aim.rays(0, &w, Vec3::NEG_Z, serial, 0., false)[0];
            let laser = aim.rays(0, &w, Vec3::NEG_Z, serial, 0., true)[0];
            assert!(laser.dot(Vec3::NEG_Z) > normal.dot(Vec3::NEG_Z));
        }
    }
    #[test]
    fn acquisition_does_not_snap_or_survive_lost_sight() {
        let w = crate::game_state::tests_weapon();
        let mut tracker = Aim::default();
        let mut aim = Vec3::NEG_Z;
        for i in 0..10 {
            tracker.advance(0, 1. / 60., &w);
            assert!(!tracker.observe(0, 1, Vec3::X, &mut aim, 1. / 60.), "{i}");
        }
        for _ in 0..10 {
            tracker.advance(0, 1. / 60., &w);
            tracker.observe(0, 1, Vec3::X, &mut aim, 1. / 60.);
        }
        assert!(tracker.observe(0, 1, Vec3::X, &mut aim, 1. / 60.));
        tracker.forget(0);
        assert!(!tracker.observe(0, 1, Vec3::X, &mut aim, 1. / 60.));
        assert!(!in_view(Vec3::Z * 10., Vec3::NEG_Z, false));
        assert!(!in_view(Vec3::NEG_Z * 40., Vec3::NEG_Z, false));
        assert!(!in_view(Vec3::Z, Vec3::NEG_Z, false));
    }
    #[test]
    fn bot_shots_use_weapon_spread_and_independent_seeds() {
        let mut w = crate::game_state::tests_weapon();
        w.accuracy.stand = 5.;
        w.accuracy.maximum = 15.;
        let mut aim = Aim::default();
        aim.advance(0, 0.1, &w);
        aim.advance(1, 0.1, &w);
        assert_ne!(
            aim.rays(0, &w, Vec3::NEG_Z, 1, 0., false),
            aim.rays(1, &w, Vec3::NEG_Z, 1, 0., false)
        );
        let hits = (0..100)
            .filter(|s| {
                crate::ray_box(
                    Vec3::Y,
                    aim.rays(0, &w, Vec3::NEG_Z, *s, 0., false)[0],
                    Vec3::new(-0.35, 0., -20.35),
                    Vec3::new(0.35, 1.8, -19.65),
                )
                .is_some()
            })
            .count();
        assert!(hits > 0 && hits < 80, "all shots must not auto-hit: {hits}");
    }
}
