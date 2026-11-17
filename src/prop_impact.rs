//! Physics props hurting survivors: retail's player impact damage
//! (server 101425e0 with the player table at 1077bff4; DMG_CRUSH).
//!
//! The rewrite's prop solver has no player shadow, so a prop passes through
//! survivors instead of colliding. A moving prop whose bounds touch a
//! survivor's hull counts as a hit that stops the prop's approach toward
//! them (its whole approach speed is the speed lost). That upper bound gives
//! the measured retail result for a car dropped on a survivor (it incapacitated
//! them from full health, probe `l4d2r_tankhit2_1115.nut`); the survivor's own
//! speed change and the Tank-hit fling/stagger reaction (103317e0) are not
//! modelled.
use bevy::prelude::*;

/// Linear table: (speed² in (units/s)², damage); a step lookup (10142500).
const LINEAR: [(f32, f32); 5] = [
    (150. * 150., 5.),
    (250. * 250., 10.),
    (350. * 350., 50.),
    (500. * 500., 100.),
    (1000. * 1000., 500.),
];
const MIN_SPEED_SQ: f32 = 24. * 24.;
const MIN_MASS: f32 = 2.;
const SMALL_MASS_MAX: f32 = 5.;
const SMALL_MASS_CAP: f32 = 5.;
const SMALL_MASS_MIN_SPEED_SQ: f32 = 36. * 36.;
const LARGE_MASS_MIN: f32 = 500.;
const LARGE_MASS_SCALE: f32 = 4.;
const LARGE_MASS_FALLING_SCALE: f32 = 5.;
/// The player physics shadow's mass (kg).
const PLAYER_MASS: f32 = 85.;
/// Seconds before the same prop can hurt the same survivor again.
const REPEAT: f32 = 0.5;

/// Damage for a prop of `mass` kg moving at `velocity` (Source units/s, z
/// up) that loses `speed_lost` units/s against a survivor.
pub fn impact_damage(mass: f32, velocity: Vec3, speed_lost: f32) -> f32 {
    let speed_sq = velocity.length_squared();
    if mass < MIN_MASS || speed_sq < MIN_SPEED_SQ {
        return 0.;
    }
    if mass < SMALL_MASS_MAX && speed_sq < SMALL_MASS_MIN_SPEED_SQ {
        return 0.;
    }
    let mut energy = speed_lost * speed_lost * mass;
    if mass >= LARGE_MASS_MIN {
        energy *= LARGE_MASS_SCALE;
        // Falling onto the survivor scales up to largeMassFallingScale by
        // how vertical the speed change is.
        let speed = velocity.length();
        if speed_lost > 0. && velocity.z < 0. && speed > 0. {
            energy *= (LARGE_MASS_FALLING_SCALE - 1.) * (velocity.z / speed).abs() + 1.;
        }
    }
    let value = energy / PLAYER_MASS;
    let damage = LINEAR.iter().rev().find(|(threshold, _)| value >= *threshold).map_or(0., |(_, d)| *d);
    if mass < SMALL_MASS_MAX { damage.min(SMALL_MASS_CAP) } else { damage }
}

/// A survivor's hull in Source units: feet (renderer space) to 72 up,
/// 16 each side.
fn hull(feet: Vec3) -> (Vec3, Vec3) {
    let f = Vec3::new(feet.x, -feet.z, feet.y) / crate::collision::UNIT;
    (f - Vec3::new(16., 16., 0.), f + Vec3::new(16., 16., 72.))
}
fn overlaps(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    a.0.x <= b.1.x && a.1.x >= b.0.x && a.0.y <= b.1.y && a.1.y >= b.0.y && a.0.z <= b.1.z && a.1.z >= b.0.z
}

/// Props a Tank punched, by map clock time (the prop's tank-hit time,
/// server 0x1678; it counts for 2 s, 10198b20).
static TANK_HIT: std::sync::Mutex<Option<std::collections::HashMap<usize, f32>>> = std::sync::Mutex::new(None);
const TANK_HIT_WINDOW: f32 = 2.;
/// A Tank's punch landed on prop `id` at map time `now`.
pub fn mark_tank_hit(id: usize, now: f32) {
    TANK_HIT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(id, now);
}
/// Survivor reaction to a Tank-hit prop (103317e0): flung when the crush
/// damage reaches the Tank's punch damage, else staggered from 10 up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reaction {
    /// Renderer-space prop centre and velocity.
    Fling { from: Vec3, velocity: Vec3 },
    Stagger { away: Vec3 },
}
pub fn reaction(damage: f32, punch_damage: f32) -> Option<bool> {
    if damage >= punch_damage {
        Some(true)
    } else if damage >= 10. {
        Some(false)
    } else {
        None
    }
}
static REACTIONS: std::sync::Mutex<Vec<(usize, Reaction)>> = std::sync::Mutex::new(Vec::new());
/// Reactions waiting for the AI knock system (survivor index, reaction).
pub fn take_reactions() -> Vec<(usize, Reaction)> {
    std::mem::take(&mut *REACTIONS.lock().unwrap_or_else(|e| e.into_inner()))
}
/// (prop, survivor) -> time of the last hit.
static RECENT: std::sync::Mutex<Option<std::collections::HashMap<(usize, usize), f32>>> = std::sync::Mutex::new(None);

/// After the prop step: every awake prop touching a survivor hull hurts
/// them. Survivor index 0 is the player, i + 1 the bots.
pub fn apply(
    now: f32,
    punch_damage: f32,
    logic: &crate::l4d2r_cinematic::MapLogic,
    survivors: &[(usize, Vec3)],
    mut hurt: impl FnMut(usize, f32),
) {
    let mut guard = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let recent = guard.get_or_insert_with(Default::default);
    recent.retain(|_, at| (0. ..REPEAT).contains(&(now - *at)));
    for (&id, body) in &logic.physics.0 {
        if body.asleep {
            continue;
        }
        let velocity = Vec3::new(body.velocity.x, -body.velocity.z, body.velocity.y) / crate::collision::UNIT;
        if velocity.length_squared() < MIN_SPEED_SQ || body.mass < MIN_MASS {
            continue;
        }
        let Some((mins, maxs)) = logic.prop_bounds.get(&id) else { continue };
        let m = logic.world.world_matrix(id);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for i in 0..8 {
            let c = l4d2r_math::Vec3::new(
                if i & 1 == 0 { mins.x } else { maxs.x },
                if i & 2 == 0 { mins.y } else { maxs.y },
                if i & 4 == 0 { mins.z } else { maxs.z },
            );
            let w = m.transform_point3(c);
            let w = Vec3::new(w.x, w.y, w.z);
            lo = lo.min(w);
            hi = hi.max(w);
        }
        for &(index, feet) in survivors {
            let h = hull(feet);
            if !overlaps((lo, hi), h) || recent.contains_key(&(id, index)) {
                continue;
            }
            // The approach speed toward the survivor's centre is lost.
            let centre = (h.0 + h.1) * 0.5;
            let toward = (centre - (lo + hi) * 0.5).normalize_or_zero();
            let lost = velocity.dot(toward).max(0.);
            let damage = impact_damage(body.mass, velocity, lost);
            recent.insert((id, index), now);
            if damage > 0. {
                hurt(index, damage);
                let hit_at = TANK_HIT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).copied());
                if hit_at.is_some_and(|at| (0. ..TANK_HIT_WINDOW).contains(&(logic.world.time - at))) {
                    let bevy = |v: Vec3| Vec3::new(v.x, v.z, -v.y) * crate::collision::UNIT;
                    let from = bevy((lo + hi) * 0.5);
                    let r = match reaction(damage, punch_damage) {
                        Some(true) => Some(Reaction::Fling { from, velocity: bevy(velocity) }),
                        Some(false) => Some(Reaction::Stagger { away: (bevy(centre) - from) * Vec3::new(1., 0., 1.) }),
                        None => None,
                    };
                    if let Some(r) = r {
                        REACTIONS.lock().unwrap_or_else(|e| e.into_inner()).push((index, r));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_impact_table_and_mass_rules() {
        // A car dropped onto a survivor at ~300 units/s: far past the top step.
        assert_eq!(impact_damage(1500., Vec3::new(0., 0., -300.), 300.), 500.);
        // Light props: under 2 kg nothing, under 5 kg at most 5.
        assert_eq!(impact_damage(1.5, Vec3::X * 900., 900.), 0.);
        assert_eq!(impact_damage(4., Vec3::X * 900., 900.), 5.);
        // A 50 kg crate at 400 units/s: 400² x 50 / 85 = 94118 -> 10.
        assert_eq!(impact_damage(50., Vec3::X * 400., 400.), 10.);
        // A 20 kg chair at 300: 21176, under 150² -> 0.
        assert_eq!(impact_damage(20., Vec3::X * 300., 300.), 0.);
        // Tank-hit reactions: fling at the punch damage (24 on Normal),
        // stagger from 10.
        assert_eq!(reaction(500., 24.), Some(true));
        assert_eq!(reaction(10., 24.), Some(false));
        assert_eq!(reaction(5., 24.), None);
        // Barely moving: nothing.
        assert_eq!(impact_damage(1500., Vec3::X * 20., 20.), 0.);
    }
}
