//! Falling damage recovered from the owned server's Terror game rules.
use crate::{
    collision::{Collision, HEIGHT, UNIT},
    game_state::{Companion, Match},
};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub safe: f32,
    pub fatal: f32,
    pub max_incap_damage: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            safe: 560.,
            fatal: 720.,
            max_incap_damage: 200.,
        }
    }
}
impl Settings {
    pub fn damage(self, speed: f32) -> f32 {
        if !speed.is_finite() || speed <= self.safe || self.fatal <= self.safe {
            return 0.;
        }
        100. * ((speed - self.safe) / (self.fatal - self.safe)).powi(2)
    }
    pub fn land(self, state: &mut Match, speed: f32) -> f32 {
        if state.dead || state.infected {
            return 0.;
        }
        let damage = self.damage(speed);
        if damage <= 0. {
            return 0.;
        }
        state.damage(damage);
        // Owned 10332c20: DMG_FALL at/above the incap threshold is fatal
        // when this damage would otherwise incapacitate the survivor.
        if state.incap && damage >= self.max_incap_damage {
            state.dead = true;
            state.incap = false;
            state.incap_health = 0.;
        }
        damage
    }
}

/// Ordinary bot locomotion, including sb_stop gravity. A landing consumes
/// its downward speed once; stairs/standing support cannot repeat damage.
pub fn move_bot(
    member: &mut Companion,
    collision: &Collision,
    wish: Vec3,
    dt: f32,
    settings: Settings,
) -> f32 {
    let grounded = collision.grounded(member.position, HEIGHT);
    let mut impact = 0.;
    if grounded {
        impact = (-member.vertical / UNIT).max(0.);
        member.vertical = 0.;
    } else {
        member.vertical -= crate::player_movement::GRAVITY * dt * 0.5;
    }
    let velocity = wish + Vec3::Y * member.vertical;
    let (position, remaining) =
        collision.walk_velocity(member.position, velocity, dt, HEIGHT, grounded);
    // Retail CGameMovement::StayOnGround: a walking survivor follows the
    // floor down steps and slopes (up to the 18-unit step height) instead of
    // briefly falling off every stair.
    let position = if grounded { collision.snap_ground(position, HEIGHT) } else { position };
    if !grounded && member.vertical < 0. && collision.grounded(position, HEIGHT) {
        impact = (-member.vertical / UNIT).max(0.);
        member.vertical = 0.;
    } else if !grounded {
        member.vertical = remaining.y - crate::player_movement::GRAVITY * dt * 0.5;
    }
    member.position = position;
    settings.land(&mut member.state, impact)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_quadratic_curve_and_bad_inputs() {
        let s = Settings::default();
        for (speed, damage) in [
            (0., 0.),
            (560., 0.),
            (600., 6.25),
            (640., 25.),
            (720., 100.),
            (800., 225.),
        ] {
            assert!((s.damage(speed) - damage).abs() < 0.001);
        }
        for speed in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -800.] {
            assert_eq!(s.damage(speed), 0.);
        }
    }
}
