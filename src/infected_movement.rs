//! Local crowd steering; native locomotion/avoidance behavior remains unverified.
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
const SPACING: f32 = 0.8;
pub const RUN_SPEED: f32 = 250. * crate::collision::UNIT;
/// A common's own run speed: ZombieBotLocomotion's constructor (server
/// 103f8bd0) stores z_speed x RandomFloat(0.9, 1.1) for the common's life.
/// Drawn deterministically from the actor id here.
pub fn run_speed(id: u64) -> f32 {
    let mut h = id.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    RUN_SPEED * (0.9 + 0.2 * (h >> 40) as f32 / (1u64 << 24) as f32)
}
/// Use the same swept step/slide and shallow support recovery as player
/// movement. The older position-only walk could remain start-solid at a tread.
pub fn walk(
    c: &crate::collision::Collision,
    start: Vec3,
    wish: Vec3,
    vertical: f32,
    dt: f32,
) -> (Vec3, f32) {
    let grounded = c.grounded(start, crate::collision::HEIGHT);
    let vertical = if grounded {
        0.
    } else {
        vertical - 800. * crate::collision::UNIT * dt
    };
    let (point, velocity) = c.walk_velocity(
        start,
        wish + Vec3::Y * vertical,
        dt,
        crate::collision::HEIGHT,
        grounded,
    );
    (point, velocity.y)
}
/// What a chasing actor did this tick when its walk made no progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chase {
    Walked,
    Pounded,
    Climbing,
    /// No progress for STALL_TIME: the caller should reroute.
    Stuck,
}
const STALL_TIME: f32 = 1.5;
/// Run-up from rest. Retail commons recorded on c1m1_hotel after
/// director_force_panic_event (12 November) reached 56, 112, 158, 201, 234
/// and 253 units/s on successive 0.1 s samples: about 470 units/s².
pub const RUN_ACCELERATION: f32 = 470. * crate::collision::UNIT;
/// One chase tick for a common: steer along `toward` (already a route
/// direction), keep crowd spacing, walk, and when the walk stalls try a door
/// pound, then a climb, then obstruction steering. Shared by the game system
/// and the navigation harness so both exercise the same movement.
#[allow(clippy::too_many_arguments)]
pub fn chase(
    c: &crate::collision::Collision,
    crowd: &mut Crowd,
    id: u64,
    before: Vec3,
    toward: Vec3,
    vertical: f32,
    dt: f32,
    mut pound: impl FnMut(Vec3, Vec3) -> bool,
) -> (Vec3, f32, Chase) {
    let toward = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
    let push = crowd.separate(id, before);
    let reached = crowd.pace.get(&id).map_or(0., |p| p.0);
    let speed = run_speed(id).min(reached + RUN_ACCELERATION * dt);
    let (point, vertical, outcome) = chase_at(c, crowd, id, before, toward, push, speed, vertical, dt, &mut pound);
    let moved = ((point - before) * Vec3::new(1., 0., 1.)).length() / dt.max(1e-4);
    crowd.pace.insert(id, (moved.min(speed), true));
    (point, vertical, outcome)
}
#[allow(clippy::too_many_arguments)]
fn chase_at(
    c: &crate::collision::Collision,
    crowd: &mut Crowd,
    id: u64,
    before: Vec3,
    toward: Vec3,
    push: Vec3,
    speed: f32,
    vertical: f32,
    dt: f32,
    pound: &mut impl FnMut(Vec3, Vec3) -> bool,
) -> (Vec3, f32, Chase) {
    use crate::collision::HEIGHT;
    let wish = (toward * speed + push).clamp_length_max(speed);
    let grounded = c.grounded(before, HEIGHT);
    let (mut point, mut vertical) = walk(c, before, wish, vertical, dt);
    // Blocked means little progress along the route, not just little motion:
    // a common pressed against a wall or door at an angle slides along it
    // and used to run sideways down the whole wall instead of pounding,
    // climbing or going round.
    let along = ((point - before) * Vec3::new(1., 0., 1.)).dot(toward);
    if toward == Vec3::ZERO {
        crowd.stall.remove(&id);
        return (point, vertical, Chase::Walked);
    }
    let blocked = along < dt * speed * 0.35;
    // A door met at an angle is pounded too, not slid along: any tick whose
    // walk was cut short tries the pound (cheap to skip otherwise).
    let clipped = ((point - before) * Vec3::new(1., 0., 1.)).length() < wish.length() * dt * 0.9;
    if (blocked || clipped) && crowd.pound_ready(id) {
        if pound(before, toward) {
            crowd.stall.remove(&id);
            return (point, vertical, Chase::Pounded);
        }
        // Nothing breakable ahead: probing every map breakable at four
        // heights each tick for a whole horde pressed on walls cost ~2 ms
        // a frame (Hotel horde p50 9.6 -> 11.9 ms), so wait before retrying.
        crowd.pound_retry.insert(id, 0.3);
    }
    if blocked && grounded && push.length() <= 0.3 {
        // Pressed into a wall nearly square: the collision slide runs along
        // it, which is the sideways run. Hold position instead and leave it
        // to the climb, steer, sidestep and reroute below.
        let slide = (point - before) * Vec3::new(1., 0., 1.);
        if slide.normalize_or_zero().dot(toward) < 0.5 {
            point = before;
        }
    }
    if blocked {
        if grounded && crowd.begin_climb(id, c, before, toward) {
            crowd.stall.remove(&id);
            return (point, 0., Chase::Climbing);
        }
        if grounded {
            // Fully held for a moment (a rail end cutting the line to the
            // next waypoint): a brief sidestep up to 90 degrees is allowed.
            let pinned = crowd.pinned(id);
            let direction = crowd.obstruction_direction(id, c, before, toward, HEIGHT, pinned);
            if direction != Vec3::ZERO {
                let candidate = c.walk_velocity(
                    before,
                    direction * speed + Vec3::Y * vertical,
                    dt,
                    HEIGHT,
                    grounded,
                );
                // Keep the steered move only if it really heads along the route:
                // a steer that turns into a slide along the wall is the
                // sideways run this replaced.
                let step = (candidate.0 - before) * Vec3::new(1., 0., 1.);
                let heading = step.normalize_or_zero().dot(toward);
                if (step.dot(toward) > along && heading >= 0.5)
                    || (pinned && heading > -0.05 && step.length() > speed * dt * 0.3)
                {
                    point = candidate.0;
                    vertical = candidate.1.y;
                }
            }
        }
    }
    let along = ((point - before) * Vec3::new(1., 0., 1.)).dot(toward);
    // Held up by the crowd is not stuck: rerouting a jostling horde sent
    // its members off on detours around the very edge they needed.
    if push.length() > 0.3 {
        crowd.stall.remove(&id);
    } else if crowd.stalled(id, point, blocked && along < dt * speed * 0.35, dt) {
        return (point, vertical, Chase::Stuck);
    }
    (point, vertical, Chase::Walked)
}
#[derive(Clone)]
struct Climb {
    start: Vec3,
    end: Vec3,
    elapsed: f32,
    reaction: u8,
}
/// Infected ladder climb speed. Not measured against retail NextBot ladder
/// locomotion; infected player ghosts climb at 200 units/s.
pub const LADDER_SPEED: f32 = 200. * crate::collision::UNIT;
/// Enemy reaction codes while on a NAV ladder (ACT_CLIMB_UP / ACT_CLIMB_DOWN).
pub const LADDER_UP: u8 = 20;
pub const LADDER_DOWN: u8 = 21;
/// One tick of an infected NAV-ladder traversal: the new position and the
/// reaction code to show, or `None` when not on a ladder.
pub fn advance_ladder(crowd: &mut Crowd, id: u64, c: &crate::collision::Collision, feet: Vec3, dt: f32) -> Option<(Vec3, u8)> {
    let next = crowd.ladders.advance(id, c, feet, LADDER_SPEED, dt)?;
    let code = if !crowd.ladders.active(id) {
        0
    } else if next.y < feet.y - 1e-4 {
        LADDER_DOWN
    } else {
        LADDER_UP
    };
    Some((next, code))
}
pub fn climb_duration(reaction: u8) -> f32 {
    (reaction.saturating_sub(3) as f32 * 24. * crate::collision::UNIT) / 2. + 0.25
}
pub fn climb_activity(reaction: u8) -> String {
    format!(
        "ACT_TERROR_CLIMB_{}_FROM_STAND",
        reaction.saturating_sub(3) as usize * 24
    )
}
fn ledge(c: &crate::collision::Collision, start: Vec3, toward: Vec3) -> Option<Vec3> {
    use crate::collision::{HEIGHT, RADIUS, UNIT};
    let direction = toward.normalize_or_zero();
    let front = c.trace(
        start + Vec3::Y * 0.4,
        start + Vec3::Y * 0.4 + direction * 0.9,
        Vec3::splat(0.1),
    );
    if front.start_solid || front.fraction >= 1. || front.normal.y.abs() > 0.5 {
        return None;
    }
    // Front of the obstacle measured from the actor's centre (the probe box
    // is 0.1 m wide). Narrow ledges need standing spots just past the face,
    // not only the older fixed 0.9/1.4/2 m (c1m2: a 63-unit, 32-unit-deep
    // step with a drop behind it).
    let face = front.fraction * 0.9 + 0.1;
    let ceiling = ceiling(c, start);
    let mut distances = vec![face + RADIUS + 0.03, face + RADIUS + 0.2];
    distances.extend([0.9, 1.4, 2.]);
    distances.retain(|d| *d > face);
    for distance in distances {
        let ahead = start + direction * distance;
        let low = ahead + Vec3::Y * (18. * UNIT);
        let high = Vec3::new(ahead.x, ceiling, ahead.z);
        let hit = c.trace(high, low, Vec3::ZERO);
        if hit.start_solid || hit.fraction >= 1. || hit.normal.y < 0.7 {
            continue;
        }
        let end = high.lerp(low, hit.fraction) + Vec3::Y * 0.004;
        if end.y - start.y > 168. * UNIT || !c.clear_hull(end, HEIGHT) {
            continue;
        }
        if climbable(c, start, end) {
            return Some(end);
        }
    }
    vault(c, start, direction, face, ceiling)
}
/// Height the downward ledge probes start from: the 168-unit climb limit
/// above the feet, or just under the ceiling over the actor. Starting inside
/// the floor of the storey above made every probe start solid (c1m1's
/// burning-floor balcony railings, where NAV drops infected to the floor
/// below).
fn ceiling(c: &crate::collision::Collision, start: Vec3) -> f32 {
    use crate::collision::UNIT;
    let low = start + Vec3::Y * (18. * UNIT);
    let high = start + Vec3::Y * (168. * UNIT + 0.1);
    let up = c.trace(low, high, Vec3::splat(0.005));
    if up.start_solid {
        return high.y;
    }
    low.lerp(high, up.fraction).y - 0.01
}
/// Clear straight-up-then-across hull path from `start` to `end`.
fn climbable(c: &crate::collision::Collision, start: Vec3, end: Vec3) -> bool {
    use crate::collision::{HEIGHT, RADIUS};
    let center = start + Vec3::Y * HEIGHT * 0.5;
    let raised = Vec3::new(start.x, end.y, start.z) + Vec3::Y * HEIGHT * 0.5;
    let half = Vec3::new(RADIUS, HEIGHT * 0.5, RADIUS);
    let up = c.trace(center, raised, half);
    let across = c.trace(raised, end + Vec3::Y * HEIGHT * 0.5, half);
    !up.start_solid && up.fraction >= 1. && !across.start_solid && across.fraction >= 1.
}
/// Climb over a thin obstacle with nothing to stand on beyond it, ending in
/// the air at its top so the actor drops down the far side: railings and
/// ledge clip walls on a NAV drop link (c1m2's 45-unit TOOLSCLIP wall above a
/// 128-unit drop). The top is found with a hull-mask trace, which also sees
/// clip brushes that ray traces ignore.
fn vault(c: &crate::collision::Collision, start: Vec3, direction: Vec3, face: f32, ceiling: f32) -> Option<Vec3> {
    use crate::collision::{HEIGHT, UNIT};
    let probe = Vec3::splat(0.005);
    let inside = start + direction * (face + 0.02);
    let low = Vec3::new(inside.x, start.y + 18. * UNIT, inside.z);
    let high = Vec3::new(inside.x, ceiling, inside.z);
    let top = c.trace(high, low, probe);
    if top.start_solid || top.fraction >= 1. || top.normal.y < 0.7 {
        return None;
    }
    let height = high.y + (low.y - high.y) * top.fraction;
    if height - start.y > 168. * UNIT {
        return None;
    }
    // The probe finds the top under the obstacle's centre line; sloped or
    // stepped tops (c1m1's balcony rails) are higher under the hull's
    // corners, so a few lifts are tried as well.
    for lift in [0.004, 6. * UNIT, 12. * UNIT, 20. * UNIT] {
        if height + lift - start.y > 168. * UNIT {
            break;
        }
        for beyond in [0.45, 0.7, 1.0] {
            let end = Vec3::new(start.x, height + lift, start.z) + direction * (face + beyond);
            if !c.clear_hull(end, HEIGHT) {
                continue;
            }
            // Reached only when no standable top was found: go over and let
            // the actor fall to whatever lies beyond (a drop or the same floor).
            if climbable(c, start, end) {
                return Some(end);
            }
        }
    }
    None
}
#[derive(Default)]
pub struct Crowd {
    cells: HashMap<(i32, i32), Vec<(u64, Vec3)>>,
    pub alive: HashSet<u64>,
    fallback: HashMap<u64, (f32, Vec3)>,
    climbs: HashMap<u64, Climb>,
    climb_retry: HashMap<u64, f32>,
    stall: HashMap<u64, (Vec3, f32, f32)>,
    /// NAV ladder traversals in progress (crate::ai_ladder).
    pub ladders: crate::ai_ladder::LadderClimbs,
    pound_retry: HashMap<u64, f32>,
    /// Ground speed each chaser reached last tick and whether it chased
    /// since the previous rebuild (a common that stops chasing starts over
    /// from rest).
    pace: HashMap<u64, (f32, bool)>,
}
fn cell(p: Vec3) -> (i32, i32) {
    (
        (p.x / SPACING).floor() as i32,
        (p.z / SPACING).floor() as i32,
    )
}
impl Crowd {
    pub fn rebuild(&mut self, actors: impl Iterator<Item = (u64, Vec3)>, dt: f32) {
        self.cells.clear();
        self.alive.clear();
        for (id, p) in actors {
            self.alive.insert(id);
            self.cells.entry(cell(p)).or_default().push((id, p));
        }
        self.climbs.retain(|id, _| self.alive.contains(id));
        self.stall.retain(|id, _| self.alive.contains(id));
        self.pace.retain(|id, (_, chased)| std::mem::take(chased) && self.alive.contains(id));
        let alive = &self.alive;
        self.ladders.retain(|id| alive.contains(&id));
        self.pound_retry.retain(|id, delay| {
            *delay -= dt;
            *delay > 0. && self.alive.contains(id)
        });
        self.climb_retry.retain(|id, delay| {
            *delay -= dt;
            self.alive.contains(id)
        });
        self.fallback.retain(|id, (remaining, _)| {
            *remaining -= dt;
            self.alive.contains(id)
        });
    }
    /// Track a chasing actor's progress: true once it has stayed within half
    /// a metre for STALL_TIME, or made no progress along its route for twice
    /// that (an actor sliding along a wall away from its waypoint); then the
    /// timers restart.
    pub fn stalled(&mut self, id: u64, point: Vec3, blocked: bool, dt: f32) -> bool {
        let (anchor, still, sliding) = self.stall.entry(id).or_insert((point, 0., 0.));
        if point.distance(*anchor) > 0.5 {
            (*anchor, *still) = (point, 0.);
        } else {
            *still += dt;
        }
        *sliding = if blocked { *sliding + dt } else { 0. };
        if *still >= STALL_TIME || *sliding >= STALL_TIME * 2. {
            *self.stall.get_mut(&id).unwrap() = (point, 0., 0.);
            return true;
        }
        false
    }
    pub fn pound_ready(&self, id: u64) -> bool {
        !self.pound_retry.contains_key(&id)
    }
    pub fn cancel_climb(&mut self, id: u64) {
        self.climbs.remove(&id);
        self.ladders.cancel(id);
    }
    pub fn begin_climb(
        &mut self,
        id: u64,
        c: &crate::collision::Collision,
        start: Vec3,
        direction: Vec3,
    ) -> bool {
        if self.climb_retry.get(&id).is_some_and(|t| *t > 0.) {
            return false;
        }
        self.climb_retry.insert(id, 0.5);
        let Some(end) = ledge(c, start, direction) else {
            return false;
        };
        let reaction = 3
            + ((end.y - start.y) / (24. * crate::collision::UNIT))
                .ceil()
                .clamp(1., 7.) as u8;
        self.climbs.insert(
            id,
            Climb {
                start,
                end,
                elapsed: 0.,
                reaction,
            },
        );
        true
    }
    pub fn advance_climb(
        &mut self,
        id: u64,
        c: &crate::collision::Collision,
        current: Vec3,
        dt: f32,
    ) -> Option<(Vec3, u8, f32)> {
        let climb = self.climbs.get_mut(&id)?;
        climb.elapsed += dt;
        let duration = climb_duration(climb.reaction);
        let t = (climb.elapsed / duration).min(1.);
        let raised = Vec3::new(climb.start.x, climb.end.y, climb.start.z);
        let desired = if t < 0.65 {
            climb.start.lerp(raised, t / 0.65)
        } else {
            raised.lerp(climb.end, (t - 0.65) / 0.35)
        };
        let position = c.slide(current, desired - current, crate::collision::HEIGHT);
        if position.distance_squared(desired) > 0.01 {
            self.climbs.remove(&id);
            return Some((position, 0, 0.));
        }
        let reaction = climb.reaction;
        let remaining = (duration - climb.elapsed).max(0.);
        if t >= 1. {
            self.climbs.remove(&id);
            return Some((position, 0, 0.));
        }
        Some((position, reaction, remaining))
    }
    /// Another actor touches this one (hulls within SPACING, two 16-unit
    /// common half-widths) on the side it is moving toward.
    pub fn touching_ahead(&self, id: u64, position: Vec3, heading: Vec3) -> bool {
        let (x, z) = cell(position);
        let heading = (heading * Vec3::new(1., 0., 1.)).normalize_or_zero();
        (-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                self.cells.get(&(x + dx, z + dz)).into_iter().flatten().any(|&(other, p)| {
                    let delta = (p - position) * Vec3::new(1., 0., 1.);
                    other != id
                        && (p.y - position.y).abs() <= 0.9
                        && delta.length() < SPACING
                        && delta.dot(heading) > 0.
                })
            })
        })
    }
    pub fn separate(&self, id: u64, position: Vec3) -> Vec3 {
        let (x, z) = cell(position);
        let mut push = Vec3::ZERO;
        for dx in -1..=1 {
            for dz in -1..=1 {
                for &(other, p) in self.cells.get(&(x + dx, z + dz)).into_iter().flatten() {
                    if other == id || (p.y - position.y).abs() > 0.9 {
                        continue;
                    }
                    let delta = (position - p) * Vec3::new(1., 0., 1.);
                    let distance = delta.length();
                    if distance >= SPACING {
                        continue;
                    }
                    let direction = if distance > 0.001 {
                        delta / distance
                    } else {
                        // Coincident actors need an antisymmetric tie break. The
                        // direction is stable across query order and frame rate.
                        let seed = id.min(other).wrapping_mul(0x9e3779b97f4a7c15) ^ id.max(other);
                        let angle = (seed % 1024) as f32 * std::f32::consts::TAU / 1024.;
                        Vec3::new(angle.cos(), 0., angle.sin()) * if id < other { 1. } else { -1. }
                    };
                    push += direction * ((SPACING - distance) / SPACING) * 1.6;
                }
            }
        }
        push.clamp_length_max(1.8)
    }
    pub fn obstruction_direction(
        &mut self,
        id: u64,
        collision: &crate::collision::Collision,
        position: Vec3,
        toward: Vec3,
        height: f32,
        wide: bool,
    ) -> Vec3 {
        let direction = match self.fallback.get(&id) {
            Some((remaining, direction)) if *remaining > 0. => *direction,
            _ => {
                let direction = collision.steer(position, toward, height);
                self.fallback.insert(id, (0.25, direction));
                direction
            }
        };
        // Normally no more than ~60 degrees off the route: a 90-degree pick
        // ran commons sideways along walls and doors. With nothing better
        // they stall, and the stall reroutes them; only a fully pinned actor
        // (`wide`) may sidestep square to its route.
        if !wide && direction.dot((toward * Vec3::new(1., 0., 1.)).normalize_or_zero()) < 0.45 {
            return Vec3::ZERO;
        }
        direction
    }
    /// No movement at all for a moment (see `stalled`), early in a blocked
    /// spell: one sidestep per blocked spell, so a wedged actor does
    /// not zig-zag along the wall, and waits for its reroute instead.
    pub fn pinned(&self, id: u64) -> bool {
        self.stall.get(&id).is_some_and(|s| s.1 > 0.3 && s.2 < 0.5)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chasing_common_does_not_run_along_a_wall_and_reports_stuck() {
        use crate::collision::Collision;
        // A wide wall square across the route, the goal straight behind it.
        let mut collision = Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(1., 0., -20.), Vec3::new(1.2, 8., 20.)); // too tall to climb
        let c = collision.for_common();
        let mut crowd = Crowd::default();
        let start = Vec3::new(0.3, 0.004, 0.);
        let (mut point, mut vertical) = (start, 0.);
        let mut stuck = false;
        for _ in 0..120 {
            crowd.rebuild([(1, point)].into_iter(), 1. / 30.);
            let toward = Vec3::new(0.98, 0., 0.2); // nearly square into the wall
            let (p, v, outcome) = chase(&c, &mut crowd, 1, point, toward, vertical, 1. / 30., |_, _| false);
            (point, vertical) = (p, v);
            stuck |= outcome == Chase::Stuck;
        }
        assert!(point.x < 1., "crossed the wall: {point:?}");
        // The old 90-degree obstruction steer ran it metres along the wall.
        assert!((point.z - start.z).abs() < 2., "ran along the wall: {point:?}");
        assert!(stuck, "a common held at a wall must ask for a reroute");
    }
    #[test]
    fn common_hulls_escape_shallow_rail_corners_without_crossing_the_rail() {
        use crate::collision::{Collision, RADIUS, UNIT};
        for overlap in [0.005, 3. * UNIT] {
            let mut collision = Collision::test_solid_floor();
            collision.test_add_box(Vec3::new(0.6, 0., -2.), Vec3::new(0.7, 3., 2.));
            collision.test_add_box(Vec3::new(-2., 0., 0.6), Vec3::new(2., 3., 0.7));
            let collision = collision.for_common();
            let start = Vec3::new(0.6 - RADIUS + overlap, 0.004, 0.6 - RADIUS + overlap);
            let mut point = start;
            let mut vertical = 0.;
            for _ in 0..30 {
                (point, vertical) = walk(&collision, point, Vec3::NEG_Z * RUN_SPEED, vertical, 1. / 30.);
            }
            assert!(point.x < 0.6, "crossed the rail: {point:?}");
            if overlap < 2. * UNIT {
                assert!(start.z - point.z > 4., "still caught on rail corner: {point:?}");
                assert!(point.y.abs() < 0.01);
            } else {
                assert!(point.distance(start) < 1e-6, "deeply embedded actors must not be moved through geometry: {point:?}");
            }
        }
    }
    #[test]
    fn hotel_first_stair_flight_is_walkable_down_and_back_up() {
        let path = std::path::Path::new(
            "C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/left4dead2/maps/c1m1_hotel.bsp",
        );
        if !path.exists() {
            return;
        }
        let bsp = l4d2_formats::bsp::Bsp::parse(std::fs::read(path).unwrap()).unwrap();
        let c =
            crate::collision::Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(
                l4d2r_game_core::trace::Tracer::new(std::sync::Arc::new(bsp)),
            )))
            .for_common();
        for x in [240., 248., 256.] {
            for floor_offset in [0.03125, -0.5] {
                let start = Vec3::from_array(crate::binary::source_position([
                    x,
                    5660.,
                    2848. + floor_offset,
                ]));
                for hz in [30, 60] {
                    let mut point = start;
                    let mut vertical = 0.;
                    for _ in 0..hz {
                        (point, vertical) =
                            walk(&c, point, Vec3::NEG_Z * RUN_SPEED, vertical, 1. / hz as f32);
                    }
                    assert!(
                        start.z - point.z > 200. * crate::collision::UNIT,
                        "{hz}Hz stalled descending: {point:?}"
                    );
                    assert!(
                        start.y - point.y > 70. * crate::collision::UNIT,
                        "{hz}Hz did not descend: {point:?}"
                    );
                    for _ in 0..hz {
                        (point, vertical) =
                            walk(&c, point, Vec3::Z * RUN_SPEED, vertical, 1. / hz as f32);
                    }
                    assert!(
                        (point.z - start.z).abs() < 16. * crate::collision::UNIT,
                        "{hz}Hz stalled ascending: {point:?}"
                    );
                    assert!(
                        (point.y - start.y).abs() < 2. * crate::collision::UNIT,
                        "{hz}Hz did not ascend: {point:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn climbs_require_a_supported_top_and_clear_headroom_and_complete() {
        use crate::collision::Collision;
        let mut collision = Collision::test_solid_floor();
        let start = Vec3::Y * 0.004;
        assert!(ledge(&collision, start, Vec3::X).is_none());
        collision.test_add_box(Vec3::new(0.6, 0., -2.), Vec3::new(1.4, 1.2, 2.));
        let mut crowd = Crowd::default();
        assert!(crowd.begin_climb(1, &collision, start, Vec3::X));
        let mut current = start;
        let mut finished = false;
        for _ in 0..180 {
            let Some((point, reaction, _)) = crowd.advance_climb(1, &collision, current, 1. / 60.)
            else {
                break;
            };
            assert!(point.is_finite());
            current = point;
            if reaction == 0 {
                finished = true;
                break;
            }
        }
        assert!(
            finished && current.y > 1.19 && current.x > 0.8,
            "{current:?}"
        );
        assert!(collision.grounded(current, crate::collision::HEIGHT));
        collision.test_add_box(Vec3::new(-1., 2., -2.), Vec3::new(3., 2.2, 2.));
        assert!(ledge(&collision, start, Vec3::X).is_none());
    }
    #[test]
    fn cancelled_climbs_do_not_keep_moving_a_shoved_actor() {
        let mut crowd = Crowd::default();
        crowd.climbs.insert(
            1,
            Climb {
                start: Vec3::ZERO,
                end: Vec3::ONE,
                elapsed: 0.,
                reaction: 5,
            },
        );
        crowd.cancel_climb(1);
        assert!(
            crowd
                .advance_climb(1, &crate::collision::Collision::default(), Vec3::ZERO, 0.1)
                .is_none()
        );
    }
    #[test]
    fn coincident_actors_separate_symmetrically_and_ignore_other_floors() {
        let mut crowd = Crowd::default();
        crowd.rebuild(
            [(1, Vec3::ZERO), (2, Vec3::ZERO), (3, Vec3::Y * 3.)].into_iter(),
            0.016,
        );
        let a = crowd.separate(1, Vec3::ZERO);
        let b = crowd.separate(2, Vec3::ZERO);
        assert!(a.is_finite() && a.length() > 1.);
        assert!((a + b).length() < 0.001);
        assert_eq!(crowd.separate(3, Vec3::Y * 3.), Vec3::ZERO);
        crowd.rebuild([(1, Vec3::ZERO)].into_iter(), 0.016);
        assert_eq!(crowd.separate(1, Vec3::ZERO), Vec3::ZERO);
    }
    #[test]
    fn neighbours_across_cell_edges_are_found_without_a_global_pair_scan() {
        let mut crowd = Crowd::default();
        let a = Vec3::new(0.79, 0., -0.01);
        let b = Vec3::new(0.81, 0., 0.01);
        crowd.rebuild([(1, a), (2, b), (3, Vec3::X * 100.)].into_iter(), 0.016);
        assert!(crowd.separate(1, a).dot(a - b) > 0.);
        assert_eq!(crowd.separate(3, Vec3::X * 100.), Vec3::ZERO);
    }
}
/// Navigation harness: commons chase from many NAV areas toward distant goals
/// on a real map using the shared chase step. Reports stuck actors.
/// Run: cargo test --release --bin l4d2-rewrite common_navigation_harness -- --ignored --nocapture
/// Map: L4D2R_NAV_MAP=c1m1_hotel (default), pairs: L4D2R_NAV_PAIRS=120.
#[cfg(test)]
mod navigation_harness {
    use super::*;
    #[test]
    #[ignore = "manual navigation harness on an installed campaign map"]
    fn common_navigation_harness() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let map = std::env::var("L4D2R_NAV_MAP").unwrap_or_else(|_| "c1m1_hotel".into());
        let pairs: usize = std::env::var("L4D2R_NAV_PAIRS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
        let path = root.join(format!("left4dead2/maps/{map}.bsp"));
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let mesh = l4d2_formats::nav::parse_nav(&std::fs::read(path.with_extension("nav")).unwrap()).unwrap();
        let mut nav = crate::l4d2r_navigation::Navigation::load(&path);
        // Initial func_nav_blocker state, as l4d2r_cinematic::setup feeds it.
        for (eid, e) in logic.world.ents.iter().enumerate() {
            if e.alive && matches!(e.kind, l4d2r_game_core::entities::Kind::NavBlocker { blocked: true }) {
                if let Some(bounds) = logic.bounds(eid) {
                    nav.map_blocker_team(eid, bounds, true, e.me.kv_i32("teamToBlock").unwrap_or(0));
                }
            }
        }
        let world = |a: [f32; 3]| Vec3::from_array(crate::binary::source_position(a));
        let centers: Vec<Vec3> = mesh.areas.iter().map(|a| world(a.center())).collect();
        // Deterministic pair selection: 15-45 m apart, both with standable floor.
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rand = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        let common = collision.for_common();
        let mut chosen = Vec::new();
        let mut guard = 0;
        while chosen.len() < pairs && guard < pairs * 200 {
            guard += 1;
            let a = (rand() as usize) % centers.len();
            let b = (rand() as usize) % centers.len();
            let d = centers[a].distance(centers[b]);
            if !(15. ..=45.).contains(&d) { continue; }
            let start = centers[a] + Vec3::Y * 0.02;
            if !common.clear_hull(start, crate::collision::HEIGHT) { continue; }
            chosen.push((a, b));
        }
        let mut ok = 0;
        let mut no_route = 0;
        let mut total_pounds = 0;
        let mut ladders_taken = 0;
        // Fast ticks moving more than 70 degrees off the route direction
        // (running sideways along walls and doors).
        let (mut sideways, mut running) = (0usize, 0usize);
        let (mut streak, mut longest) = (0usize, 0usize);
        let mut stuck = Vec::new();
        let dt = 1. / 30.;
        let only: Option<usize> = std::env::var("L4D2R_NAV_ONLY").ok().and_then(|v| v.parse().ok());
        let src = |p: Vec3| [p.x / 0.0254, -p.z / 0.0254, p.y / 0.0254];
        for (k, &(a, b)) in chosen.iter().enumerate() {
            if only.is_some_and(|o| o != k) { continue; }
            let id = 1000 + k as u64;
            let mut crowd = Crowd::default();
            let mut position = centers[a] + Vec3::Y * 0.02;
            let goal = centers[b];
            let mut vertical = 0.;
            let mut pound_wait = 0.;
            let mut last = (position, 0.);
            let mut t = 0.;
            let mut reached = false;
            let mut pounds = 0;
            let mut limit = 8. + 2.5 * centers[a].distance(goal) / RUN_SPEED;
            nav.begin_infected_tick(&std::collections::HashSet::from([id]));
            if nav.toward_infected(id, position, goal, 0.).length() < 1e-4 {
                no_route += 1;
                continue;
            }
            while t < limit {
                t += dt;
                crowd.rebuild([(id, position)].into_iter(), dt);
                nav.begin_infected_tick(&crowd.alive);
                if let Some((p, reaction, _)) = crowd.advance_climb(id, &common, position, dt) {
                    position = p;
                    vertical = 0.;
                    if reaction != 0 { continue; }
                }
                if let Some((p, code)) = advance_ladder(&mut crowd, id, &common, position, dt) {
                    position = p;
                    vertical = 0.;
                    if code != 0 { continue; }
                }
                if pound_wait > 0. { pound_wait -= dt; continue; }
                if (goal - position).xz().length() < 1.2 && (goal.y - position.y).abs() < 1.5 {
                    reached = true;
                    break;
                }
                let toward = nav.toward_infected(id, position, goal, dt);
                if let Some(step) = nav.ladder_infected(id, position, goal) {
                    crowd.ladders.begin(id, &common, position, step);
                    ladders_taken += 1;
                    last = (position, t);
                    continue;
                }
                if only.is_some() && ((t / dt) as i32) % 10 == 0 {
                    let q = src(position);
                    println!("TRACE t={t:.2} at=[{:.0},{:.0},{:.0}] toward=[{:.2},{:.2}] {}", q[0], q[1], q[2], toward.x, -toward.z,
                        nav.debug_infected(id).chars().take(400).collect::<String>());
                }
                let mut pounded = false;
                let (p, v, outcome) = chase(&common, &mut crowd, id, position, toward, vertical, dt, |from, dir| {
                    pounded = logic.pound_obstacle(from, dir, &common);
                    pounded
                });
                let flat = ((p - position) * Vec3::new(1., 0., 1.), (toward * Vec3::new(1., 0., 1.)).normalize_or_zero());
                if flat.0.length() > RUN_SPEED * dt * 0.5 && flat.1 != Vec3::ZERO && outcome == Chase::Walked {
                    running += 1;
                    if flat.0.normalize().dot(flat.1) < 70f32.to_radians().cos() {
                        sideways += 1;
                        streak += 1;
                        longest = longest.max(streak);
                    } else {
                        streak = 0;
                    }
                }
                position = p;
                vertical = v;
                if outcome == Chase::Stuck {
                    nav.reroute_infected(id, position, goal);
                }
                if outcome == Chase::Pounded {
                    pound_wait = 0.8;
                    pounds += 1;
                    limit += 0.8; // door bashing is not a navigation failure
                    collision.sync_entities(&logic);
                    // Pounding a door is progress, not a stall.
                    last = (position, t);
                }
                if t - last.1 >= 6. {
                    if position.distance(last.0) < 0.3 { break; }
                    last = (position, t);
                }
            }
            total_pounds += pounds;
            if !reached {
                // Classify: route direction, height to next waypoint, and the
                // tallest obstacle the hull meets within 0.6 m along it.
                nav.begin_infected_tick(&std::collections::HashSet::from([id]));
                let toward = nav.toward_infected(id, position, goal, 0.);
                let flat = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
                let mut wall = String::from("none");
                for h in (2..=170).step_by(8) {
                    let from = position + Vec3::Y * (h as f32 * 0.0254);
                    let hit = common.trace(from, from + flat * 0.6, Vec3::new(0.15, 0.01, 0.15));
                    if hit.fraction < 1. || hit.start_solid { wall = format!("{h}"); }
                }
                let eye = position + Vec3::Y * 1.0;
                let ray = common.trace(eye, eye + flat * 0.6, Vec3::ZERO).fraction;
                let world = collision.trace(eye, eye + flat * 0.6, Vec3::new(0.15, 0.01, 0.15)).fraction;
                println!("CLASS #{k} dz_next={:.0} wall_top_u={wall} toward_len={:.2} ray={ray:.2} survivor_hull={world:.2}", toward.y / 0.0254, toward.length());
            }
            if reached { ok += 1; } else {
                stuck.push((k, src(centers[a]), src(position), src(goal), t));
            }
        }
        println!("NAVHARNESS map={map} pairs={} no_route={no_route} reached={ok} stuck={} pounds={total_pounds} ladders={ladders_taken} sideways={sideways}/{running} longest_sideways_s={:.2}", chosen.len(), stuck.len(), longest as f32 * dt);
        for (k, a, p, g, t) in &stuck {
            println!("STUCK #{k} start=[{:.0},{:.0},{:.0}] at=[{:.0},{:.0},{:.0}] goal=[{:.0},{:.0},{:.0}] t={t:.1}",
                a[0], a[1], a[2], p[0], p[1], p[2], g[0], g[1], g[2]);
        }
    }
}
