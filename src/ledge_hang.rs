//! Survivor ledge hanging, from the owned server:
//! - CTerrorGameMovement 102a8f80: while falling, a survivor whose landing
//!   is estimated (102a7b60) to deal at least 95 damage grabs the ledge they
//!   left, unless they are the last standing survivor (z_grab_ledges_solo 0).
//! - CTerrorPlayer 10320710: the grab saves health and temporary health and
//!   sets the hang health to survivor_ledge_grab_health (300). The survivor
//!   counts as incapacitated, so teammates help them up with the normal
//!   revive (survivor_revive_duration 5).
//! - 1033ffa0: the hang stage is hang health / grab health x 100, with
//!   breaks at 66.6 and 33.3.
//! - 1031f900: a ledge save does not count toward the incap limit. The
//!   saved health comes back scaled by how much hang health is left (see
//!   `Match::ledge_saved`).
//!
//! When the hang health runs out the survivor lets go and falls; normal
//! fall damage then applies (retail has separate "dropped" and "fell to
//! death" music cues, so the drop is not always fatal).
use crate::{
    collision::{Collision, HEIGHT, RADIUS, UNIT},
    game_state::{Match, Squad},
};
use bevy::prelude::*;

/// _DAT_10684080: minimum estimated landing damage that grabs a ledge.
pub const GRAB_DAMAGE: f32 = 95.;
/// survivor_ledge_grab_health default.
pub const GRAB_HEALTH: f32 = 300.;
/// survivor_hanging_eye_height default (85 units above the hang origin).
pub const HANGING_EYE: f32 = 85. * UNIT;
/// survivor_incap_health: the save formula divides by it (1031f900).
const INCAP_HEALTH: f32 = 300.;
/// survivor_revive_health: temporary health given back on a save.
const REVIVE_HEALTH: f32 = 30.;
/// The hang origin sits this far below the ledge top, so the hanging eye
/// is just above the top.
const HANG_DROP: f32 = 80. * UNIT;
/// The ledge top may be at most this far above the falling feet (the
/// 64-unit probe in 102a8f80).
const REACH: f32 = 64. * UNIT;
/// 102a7b10: no new grab within 1 second of the last hang ending.
const REGRAB_DELAY: f32 = 1.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hang {
    /// Feet while hanging.
    pub hang: Vec3,
    /// Where a rescue puts the survivor, on top of the ledge.
    pub stand: Vec3,
    /// Health and temporary health when the grab happened.
    pub health: f32,
    pub temporary: f32,
    /// Hang stage music last played: 0 two hands, 1 one hand, 2 fingers.
    pub stage: u8,
    /// `Match::elapsed` at the grab, for the grab animation.
    pub grabbed: f32,
}

/// Player-side state kept between ticks.
#[derive(Resource, Default)]
pub struct Tracker {
    /// Feet of the last tick spent on the ground.
    last_ground: Option<Vec3>,
    /// When the last hang ended (`Match::elapsed`).
    ended: Option<f32>,
}

impl Match {
    /// Eye height above the feet while downed: hanging survivors look over
    /// the ledge, incapacitated ones lie on the floor.
    pub fn downed_eye(&self) -> f32 {
        if self.ledge.is_some() { HANGING_EYE } else { 0.5 }
    }
    /// Start hanging (10320710).
    pub fn grab_ledge(&mut self, hang: Vec3, stand: Vec3, grab_health: f32) {
        self.ledge = Some(Hang {
            hang,
            stand,
            health: self.health,
            temporary: self.temporary,
            stage: 0,
            grabbed: self.elapsed,
        });
        self.incap = true;
        self.incap_health = grab_health;
        self.reload = 0.;
        self.action = 0.;
        self.burst_left = 0;
        self.medical_kind = 0;
    }
    /// Pulled back up (1031f900). Health is set to 1 + 30 temporary, then,
    /// when enough hang health is left, the saved health comes back minus
    /// the share of hang health lost: lost = total x (1 - hang / 300),
    /// taken from temporary health first. Not counted as an incap.
    pub fn ledge_saved(&mut self) {
        let Some(hang) = self.ledge.take() else { return };
        self.ledge_climb = Some(hang.stand);
        self.finish_hang();
        let (health, temporary) = (hang.health.floor(), hang.temporary.floor());
        let total = health + temporary;
        let kept = (self.incap_health / INCAP_HEALTH * total).floor();
        if total > 1. && kept > 1. {
            let lost = total - kept;
            if lost < temporary {
                self.health = health;
                self.temporary = temporary - lost;
            } else {
                self.health = health - lost + temporary;
                self.temporary = 0.;
            }
        }
        self.incap_health = INCAP_HEALTH;
    }
    /// Let go after the hang health ran out: 1 health + 30 temporary, as
    /// with no hang health left in the save formula.
    pub fn ledge_release(&mut self) {
        if self.ledge.take().is_none() {
            return;
        }
        self.finish_hang();
        self.incap_health = INCAP_HEALTH;
    }
    fn finish_hang(&mut self) {
        self.incap = false;
        self.health = 1.;
        self.temporary = REVIVE_HEALTH;
    }
}

/// Retail 102a7b60, simplified: the damage of landing on the floor below
/// at the current fall speed. No floor within 3000 units counts as fatal.
pub fn landing_damage(
    c: &Collision,
    feet: Vec3,
    vertical: f32,
    settings: crate::fall_damage::Settings,
) -> f32 {
    let depth = 3000. * UNIT;
    let extents = Vec3::new(RADIUS - 0.002, 0.01, RADIUS - 0.002);
    let down = c.trace(feet + Vec3::Y * 0.02, feet - Vec3::Y * depth, extents);
    if down.start_solid {
        return 0.;
    }
    if down.fraction >= 1. {
        return f32::INFINITY;
    }
    let drop = (depth + 0.02) * down.fraction / UNIT;
    let speed = (-vertical / UNIT).max(0.);
    let gravity = crate::player_movement::GRAVITY / UNIT;
    settings.damage((speed * speed + 2. * gravity * drop).sqrt())
}

/// The ledge a survivor just left: the wall face below `last_ground`
/// toward which they hang, with a clear hang spot and a clear spot on top.
pub fn find_ledge(c: &Collision, feet: Vec3, last_ground: Vec3) -> Option<(Vec3, Vec3)> {
    let top = last_ground.y;
    if !(0. ..=REACH).contains(&(top - feet.y)) {
        return None;
    }
    let toward = ((last_ground - feet) * Vec3::new(1., 0., 1.)).normalize_or_zero();
    if toward == Vec3::ZERO {
        return None;
    }
    // Find the wall face just below the ledge top.
    let start = Vec3::new(feet.x, top - 4. * UNIT, feet.z);
    let reach = 96. * UNIT;
    let wall = c.trace(start, start + toward * reach, Vec3::ZERO);
    if wall.start_solid || wall.fraction >= 1. {
        return None;
    }
    let face = start + toward * reach * wall.fraction;
    let mut hang = face - toward * (RADIUS + UNIT);
    hang.y = top - HANG_DROP;
    if !c.clear_hull(hang, HEIGHT) {
        return None;
    }
    let mut stand = face + toward * (RADIUS + 2. * UNIT);
    stand.y = top + UNIT;
    let stand = c.snap_ground(stand, HEIGHT);
    let stand = if c.clear_hull(stand, HEIGHT) && (stand.y - top).abs() < 18. * UNIT {
        stand
    } else if c.clear_hull(last_ground, HEIGHT) {
        last_ground
    } else {
        return None;
    };
    Some((hang, stand))
}

/// Hold a hanging player on the ledge, let go when the hang health is gone,
/// and move a saved player onto the ledge. Returns true while hanging.
pub fn hold(
    tracker: &mut Tracker,
    feet: &mut Vec3,
    velocity: &mut Vec3,
    grounded: &mut bool,
    s: &mut Match,
    audio: Option<&mut crate::game_audio::Queue>,
) -> bool {
    if let Some(stand) = s.ledge_climb.take() {
        *feet = stand;
        *velocity = Vec3::ZERO;
        *grounded = true;
        tracker.ended = Some(s.elapsed);
        tracker.last_ground = Some(stand);
    }
    let Some(hang) = s.ledge.as_mut() else { return false };
    if s.dead {
        return true;
    }
    *feet = hang.hang;
    *velocity = Vec3::ZERO;
    *grounded = false;
    if s.incap_health <= 0. {
        s.ledge_release();
        tracker.ended = Some(s.elapsed);
        tracker.last_ground = None;
        if let Some(audio) = audio {
            audio.play("Event.LedgeHangFalling", None);
        }
        return false;
    }
    let stage = stage(s.incap_health / GRAB_HEALTH * 100.);
    if stage > hang.stage {
        hang.stage = stage;
        if let Some(audio) = audio {
            audio.play(
                if stage == 1 { "Event.LedgeHangOneHand" } else { "Event.LedgeHangFingers" },
                None,
            );
        }
    }
    true
}

/// The hanging survivor's animation (third-person view): the grab
/// (Collapse_to_Hanging, 1.07 s), then the stage's hang loop. While being
/// helped up, the climb (GetUpFrom_Ledge, 5 s) runs over the revive.
pub fn activity(s: &Match, revive: f32) -> Option<&'static str> {
    let hang = s.ledge.as_ref()?;
    if revive > 0. {
        return Some("ACT_TERROR_LEDGE_CLIMB");
    }
    if s.elapsed - hang.grabbed < 1.067 {
        return Some("ACT_TERROR_FALL_GRAB_LEDGE");
    }
    Some(match stage(s.incap_health / GRAB_HEALTH * 100.) {
        0 => "ACT_TERROR_LEDGE_HANG_FIRM",
        1 => "ACT_TERROR_LEDGE_HANG_WEAK",
        _ => "ACT_TERROR_LEDGE_HANG_DANGLE",
    })
}

/// 1033ffa0 hang stage: 0 firm (>= 66.6), 1 weak, 2 dangling (< 33.3).
pub fn stage(percent: f32) -> u8 {
    if percent < 33.3 {
        2
    } else if percent < 66.6 {
        1
    } else {
        0
    }
}

/// Grab a ledge while falling (102a8f80). Call after the player moved.
pub fn detect(
    tracker: &mut Tracker,
    c: &Collision,
    feet: Vec3,
    velocity: Vec3,
    grounded: bool,
    s: &mut Match,
    squad: Option<&Squad>,
    director: Option<&crate::source_director::Director>,
    audio: Option<&mut crate::game_audio::Queue>,
) -> bool {
    if grounded {
        tracker.last_ground = Some(feet);
        return false;
    }
    if s.infected || s.dead || s.incap || s.pinned || velocity.y >= 0. {
        return false;
    }
    let Some(last_ground) = tracker.last_ground else { return false };
    if tracker.ended.is_some_and(|t| s.elapsed - t < REGRAB_DELAY) {
        return false;
    }
    // The last survivor standing does not grab ledges (z_grab_ledges_solo).
    let standing = squad.map_or(0, |q| {
        q.0.iter().filter(|m| !m.state.dead && !m.state.incap).count()
    }) + 1;
    let solo = director.map_or(0., |d| d.special_cvar("z_grab_ledges_solo", 0.));
    if standing < 2 && solo == 0. {
        return false;
    }
    let settings = director.map_or_else(Default::default, |d| d.fall_damage);
    if landing_damage(c, feet, velocity.y, settings) < GRAB_DAMAGE {
        return false;
    }
    let Some((hang, stand)) = find_ledge(c, feet, last_ground) else {
        return false;
    };
    let health = director.map_or(GRAB_HEALTH, |d| d.special_cvar("survivor_ledge_grab_health", GRAB_HEALTH));
    s.grab_ledge(hang, stand, health.max(1.));
    tracker.last_ground = None;
    println!(
        "Ledge grab: hang {:?} stand {:?}",
        hang / UNIT,
        stand / UNIT
    );
    if let Some(audio) = audio {
        audio.play("Event.LedgeHangTwoHands", None);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Match {
        let mut s = Match::new(&crate::game_state::tests::pistol());
        s.paused = false;
        s
    }

    #[test]
    fn hanging_animation_grabs_then_follows_the_stage_and_the_climb() {
        let mut s = state();
        assert_eq!(activity(&s, 0.), None);
        s.grab_ledge(Vec3::ZERO, Vec3::X, 300.);
        assert_eq!(activity(&s, 0.), Some("ACT_TERROR_FALL_GRAB_LEDGE"));
        s.elapsed += 2.;
        assert_eq!(activity(&s, 0.), Some("ACT_TERROR_LEDGE_HANG_FIRM"));
        s.incap_health = 150.;
        assert_eq!(activity(&s, 0.), Some("ACT_TERROR_LEDGE_HANG_WEAK"));
        s.incap_health = 50.;
        assert_eq!(activity(&s, 0.), Some("ACT_TERROR_LEDGE_HANG_DANGLE"));
        assert_eq!(activity(&s, 1.), Some("ACT_TERROR_LEDGE_CLIMB"));
        assert!(s.survivor_third_person());
    }
    #[test]
    fn hang_stages_follow_the_retail_thresholds() {
        assert_eq!(stage(100.), 0);
        assert_eq!(stage(66.6), 0);
        assert_eq!(stage(66.5), 1);
        assert_eq!(stage(33.3), 1);
        assert_eq!(stage(33.2), 2);
    }

    #[test]
    fn ledge_save_restores_scaled_health_without_counting_an_incap() {
        // 80 health + 20 temporary, saved with 270 of 300 hang health left:
        // kept = 90, lost = 10, taken from temporary health.
        let mut s = state();
        s.health = 80.;
        s.temporary = 20.;
        s.grab_ledge(Vec3::ZERO, Vec3::Y, 300.);
        assert!(s.incap && s.incap_health == 300.);
        s.incap_health = 270.;
        s.revive();
        assert!(!s.incap && s.ledge.is_none());
        assert_eq!((s.health, s.temporary), (80., 10.));
        assert_eq!(s.revives, 0);
        assert!(!s.black_white);
        assert_eq!(s.ledge_climb, Some(Vec3::Y));
        // Losing more than the temporary health comes out of real health.
        let mut s = state();
        s.health = 60.;
        s.temporary = 10.;
        s.grab_ledge(Vec3::ZERO, Vec3::ZERO, 300.);
        s.incap_health = 150.;
        s.revive();
        assert_eq!((s.health, s.temporary), (35., 0.));
        // Almost no hang health left: 1 health + 30 temporary.
        let mut s = state();
        s.grab_ledge(Vec3::ZERO, Vec3::ZERO, 300.);
        s.incap_health = 2.;
        s.revive();
        assert_eq!((s.health, s.temporary), (1., 30.));
    }

    #[test]
    fn damage_while_hanging_never_kills_and_lets_go_at_zero() {
        let mut s = state();
        s.grab_ledge(Vec3::new(0., 5., 0.), Vec3::ZERO, 300.);
        s.damage(400.);
        assert!(!s.dead && s.incap && s.incap_health == 0.);
        let mut tracker = Tracker::default();
        let (mut feet, mut velocity, mut grounded) = (Vec3::ZERO, Vec3::NEG_Y, true);
        let hanging = hold(&mut tracker, &mut feet, &mut velocity, &mut grounded, &mut s, None);
        assert!(!hanging && !s.incap && !s.dead && s.ledge.is_none());
        assert_eq!((s.health, s.temporary), (1., 30.));
        assert_eq!(feet, Vec3::new(0., 5., 0.));
        assert_eq!(velocity, Vec3::ZERO);
        assert!(!grounded);
    }

    #[test]
    fn hold_keeps_the_player_on_the_ledge_and_a_save_moves_them_on_top() {
        let mut s = state();
        let hang = Vec3::new(1., 2., 3.);
        let stand = Vec3::new(1., 4., 2.);
        s.grab_ledge(hang, stand, 300.);
        let mut tracker = Tracker::default();
        let (mut feet, mut velocity, mut grounded) = (Vec3::ZERO, Vec3::NEG_Y, false);
        assert!(hold(&mut tracker, &mut feet, &mut velocity, &mut grounded, &mut s, None));
        assert_eq!(feet, hang);
        s.incap_health = 290.;
        s.revive();
        assert!(!hold(&mut tracker, &mut feet, &mut velocity, &mut grounded, &mut s, None));
        assert_eq!(feet, stand);
        assert!(grounded);
        assert!(s.ledge_climb.is_none());
    }
}
