//! Movement parameters measured in the installed original Hotel fixtures.
use crate::collision::UNIT;
use bevy::prelude::*;
pub const JUMP: f32 = 249.8 * UNIT;
pub const GRAVITY: f32 = 800. * UNIT;
pub const JUMP_DUCK_SHIFT: f32 = 18. * UNIT;
/// Source-style presentation smoothing. Physics feet/hulls remain authoritative.
#[derive(Default)]
pub(crate) struct StairView {
    height: Option<f32>,
    eye: Option<f32>,
    position: Option<Vec3>,
}
impl StairView {
    pub fn sample(&mut self, feet: Vec3, eye: f32, grounded: bool, allow: bool, dt: f32) -> f32 {
        let teleport = self
            .position
            .is_some_and(|p| p.distance_squared(feet) >= 1.);
        let reset = !allow || !grounded || teleport || self.eye != Some(eye);
        let mut height = self.height.unwrap_or(feet.y);
        if reset {
            height = feet.y;
        } else {
            let difference = feet.y - height;
            let movement = (150. * UNIT * dt.max(0.)).min(difference.abs());
            height += difference.signum() * movement;
            height = height.clamp(feet.y - 18. * UNIT, feet.y + 18. * UNIT);
        }
        self.height = Some(height);
        self.eye = Some(eye);
        self.position = Some(feet);
        height
    }
}
#[derive(Clone, Copy, Default)]
pub(crate) struct Stance {
    pub fraction: f32,
    pub jump_duck: bool,
}
impl Stance {
    pub fn update(&mut self, held: bool, can_stand: bool, dt: f32) {
        if self.jump_duck {
            self.fraction = 1.;
            return;
        }
        if held {
            self.fraction = (self.fraction + dt / 0.4).min(1.);
        } else if can_stand {
            self.fraction = (self.fraction - dt / 0.2).max(0.);
        }
    }
    pub fn eye(&self) -> f32 {
        let f = self.fraction * self.fraction * (3. - 2. * self.fraction);
        (62. - 18. * f) * UNIT
    }
}
pub fn horizontal(velocity: Vec3, wish: Vec3, speed: f32, ground: bool, dt: f32) -> Vec3 {
    horizontal_with_acceleration(velocity, wish, speed, speed, ground, dt)
}
pub fn horizontal_with_acceleration(
    velocity: Vec3,
    wish: Vec3,
    speed: f32,
    acceleration_speed: f32,
    ground: bool,
    dt: f32,
) -> Vec3 {
    let mut v = Vec3::new(velocity.x, 0., velocity.z);
    if ground {
        let magnitude = v.length();
        if magnitude > 0. {
            let drop = magnitude.max(75. * UNIT) * 4. * dt;
            v *= ((magnitude - drop).max(0.)) / magnitude;
        }
    }
    let direction = wish.normalize_or_zero();
    let cap = if ground { speed } else { speed.min(30. * UNIT) };
    let add = cap - v.dot(direction);
    if add > 0. && direction != Vec3::ZERO {
        v += direction * add.min(if ground { 5. } else { 10. } * acceleration_speed * dt);
    }
    Vec3::new(v.x, velocity.y, v.z)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stairs_ease_both_directions_and_reset_for_air_crouch_platforms_and_teleports() {
        let feet = |units| Vec3::Y * (units * UNIT);
        for direction in [-1., 1.] {
            let mut view = StairView::default();
            assert_eq!(
                view.sample(feet(100.), EYE_TEST, true, true, 1. / 120.),
                100. * UNIT
            );
            let target = 100. + direction * 12.;
            let first = view.sample(feet(target), EYE_TEST, true, true, 1. / 120.);
            assert!(((first / UNIT - 100.) - direction * 1.25).abs() < 0.001);
            for _ in 0..12 {
                view.sample(feet(target), EYE_TEST, true, true, 1. / 120.);
            }
            assert!(
                (view.sample(feet(target), EYE_TEST, true, true, 1. / 120.) / UNIT - target).abs()
                    < 0.001
            );
            assert_eq!(
                view.sample(feet(80.), EYE_TEST, false, true, 1. / 120.),
                80. * UNIT
            );
            assert_eq!(
                view.sample(feet(90.), 44. * UNIT, true, true, 1. / 120.),
                90. * UNIT
            );
            assert_eq!(
                view.sample(feet(70.), 44. * UNIT, true, false, 1. / 120.),
                70. * UNIT
            );
            assert_eq!(
                view.sample(feet(200.), 44. * UNIT, true, true, 1. / 120.),
                200. * UNIT
            );
        }
    }
    const EYE_TEST: f32 = 62. * UNIT;
    #[test]
    fn stair_camera_offset_is_bounded_and_rate_is_frame_partition_independent() {
        let mut a = StairView::default();
        let mut b = StairView::default();
        for view in [&mut a, &mut b] {
            view.sample(Vec3::ZERO, EYE_TEST, true, true, 0.);
        }
        for _ in 0..6 {
            a.sample(Vec3::Y * (12. * UNIT), EYE_TEST, true, true, 1. / 60.);
        }
        for _ in 0..12 {
            b.sample(Vec3::Y * (12. * UNIT), EYE_TEST, true, true, 1. / 120.);
        }
        assert!((a.height.unwrap() - b.height.unwrap()).abs() < 1e-5);
        let value = a.sample(Vec3::Y * (-12. * UNIT), EYE_TEST, true, true, 1. / 120.);
        assert!((value + 12. * UNIT).abs() <= 18. * UNIT + 1e-5);
    }
    #[test]
    fn original_ground_velocity_steps_and_stop_friction() {
        let mut v = Vec3::ZERO;
        for expected in [36.666667, 63.333333, 90., 114.666667, 136.044444] {
            v = horizontal(v, Vec3::X, 220. * UNIT, true, 1. / 30.);
            assert!((v.x / UNIT - expected).abs() < 0.003);
        }
        v = horizontal(
            Vec3::X * 220. * UNIT,
            Vec3::ZERO,
            220. * UNIT,
            true,
            1. / 30.,
        );
        assert!((v.x / UNIT - 190.666667).abs() < 0.003);
        let crouch = horizontal(Vec3::X * 12.5 * UNIT, Vec3::X, 75. * UNIT, true, 1. / 30.);
        assert!((crouch.x / UNIT - 15.).abs() < 0.003);
    }
    #[test]
    fn stance_matches_eye_curve_and_cannot_stand_under_obstacle() {
        let mut s = Stance::default();
        s.update(true, true, 0.2);
        assert!((s.eye() / UNIT - 53.).abs() < 0.001);
        s.update(true, true, 0.2);
        assert!((s.eye() / UNIT - 44.).abs() < 0.001);
        s.update(false, false, 1.);
        assert_eq!(s.fraction, 1.);
        s.update(false, true, 0.2);
        assert!((s.eye() / UNIT - 62.).abs() < 0.001);
    }
}
