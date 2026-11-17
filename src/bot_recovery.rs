use crate::collision::{Collision, HEIGHT};
use bevy::prelude::*;

#[derive(Default)]
struct Progress {
    origin: Vec3,
    goal: Vec3,
    sample: f32,
    far_since: Option<f32>,
    retry: f32,
    ready: bool,
    /// Retail NextBot StuckMonitor: the anchor is `origin`; seconds spent
    /// within STUCK_RADIUS of it while trying to move.
    stuck_for: f32,
    last: f32,
}
/// NextBot STUCK_RADIUS (100 units) and escape time at the survivor run
/// speed, 100 / (0.1 * 220 + 0.1) s. A stuck survivor bot is teleported
/// (retail ResolveStuckSituation 1038cc90, called from the bots' OnStuck).
const STUCK_RADIUS: f32 = 100. * crate::collision::UNIT;
const STUCK_TIME: f32 = 100. / (0.1 * 220. + 0.1);
#[derive(Default)]
pub struct Recovery {
    bots: Vec<Progress>,
    clock: f32,
}
impl Recovery {
    pub fn due(&mut self, index: usize, now: f32, position: Vec3, goal: Vec3) -> bool {
        if now < self.clock {
            self.bots.clear();
        }
        self.clock = now;
        if self.bots.len() <= index {
            self.bots.resize_with(index + 1, Progress::default);
        }
        let p = &mut self.bots[index];
        let distance = position.distance(goal);
        if !p.ready {
            p.origin = position;
            p.goal = goal;
            p.sample = now;
            p.last = now;
            p.ready = true;
        }
        // Calls can pause (player airborne): never count a gap as stuck time.
        let dt = (now - p.last).clamp(0., 0.25);
        p.last = now;
        if distance > 30. {
            p.far_since.get_or_insert(now);
        } else {
            p.far_since = None;
        }
        let far = p.far_since.is_some_and(|start| now - start >= 2.);
        // Stuck: trying to move (farther than the follow range) but still
        // within STUCK_RADIUS of the anchor after STUCK_TIME, wherever the
        // goal went meanwhile.
        if distance <= 6. || position.distance(p.origin) > STUCK_RADIUS {
            p.origin = position;
            p.stuck_for = 0.;
        } else {
            p.stuck_for += dt;
        }
        let blocked = p.stuck_for >= STUCK_TIME;
        p.goal = goal;
        p.sample = now;
        if now >= p.retry && (far || blocked) {
            p.retry = now + 0.5;
            true
        } else {
            false
        }
    }
    pub fn reset(&mut self, index: usize) {
        if let Some(p) = self.bots.get_mut(index) {
            *p = Progress::default();
        }
    }
}
pub fn landing(
    collision: &Collision,
    player: Vec3,
    yaw: f32,
    index: usize,
    occupied: &[(Vec3, bool)],
) -> Option<Vec3> {
    for radius in [1.2, 2.4, 3.6] {
        for sample in 0..12 {
            let angle = yaw + (sample + index * 4) as f32 * std::f32::consts::TAU / 12.;
            let candidate = player + Vec3::new(angle.sin(), 0., angle.cos()) * radius;
            let hit = collision.trace(
                candidate + Vec3::Y * 0.4,
                candidate - Vec3::Y * 1.5,
                Vec3::ZERO,
            );
            if hit.start_solid || hit.fraction >= 1. || hit.normal.y < 0.7 {
                continue;
            }
            let point = candidate + Vec3::Y * 0.4 - Vec3::Y * 1.9 * hit.fraction + Vec3::Y * 0.004;
            if (point.y - player.y).abs() > 0.65
                || !collision.clear_hull(point, HEIGHT)
                || occupied.iter().enumerate().any(|(i, (p, alive))| {
                    i != index && *alive && p.distance_squared(point) < 0.85f32.powi(2)
                })
            {
                continue;
            }
            return Some(point);
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sustained_distance_and_blocking_trigger_but_progress_and_restart_do_not() {
        let mut recovery = Recovery::default();
        assert!(!recovery.due(0, 0., Vec3::X * 40., Vec3::ZERO));
        assert!(!recovery.due(0, 1.9, Vec3::X * 40., Vec3::ZERO));
        assert!(recovery.due(0, 2.1, Vec3::X * 40., Vec3::ZERO));
        // Ticks of 0.1 s from `from` to `to`; true if any tick was due.
        let run = |r: &mut Recovery, from: f32, to: f32, at: Vec3, goal: Vec3| {
            let mut any = false;
            let mut t = from;
            while t <= to + 1e-4 {
                any |= r.due(0, t, at, goal);
                t += 0.1;
            }
            any
        };
        recovery.reset(0);
        // Stuck 10 m from the goal: due after retail's 4.5 s escape time,
        // even though the goal (the moving player) changes meanwhile.
        assert!(!run(&mut recovery, 3., 7.4, Vec3::X * 10., Vec3::Z * 3.));
        assert!(run(&mut recovery, 7.5, 7.7, Vec3::X * 10., Vec3::Z * 6.));
        recovery.reset(0);
        // Moving more than 100 units restarts the clock.
        assert!(!run(&mut recovery, 10., 13., Vec3::X * 10., Vec3::ZERO));
        assert!(!run(&mut recovery, 13.1, 16., Vec3::X * 7., Vec3::ZERO));
        // Within the follow range a bot is not trying to move.
        recovery.reset(0);
        assert!(!run(&mut recovery, 20., 30., Vec3::X * 5., Vec3::ZERO));
    }
    #[test]
    fn all_bot_timers_survive_processing_lower_indices() {
        let mut recovery = Recovery::default();
        for now in [0., 1., 2.1] {
            for index in 0..3 {
                assert_eq!(
                    recovery.due(index, now, Vec3::X * 40., Vec3::ZERO),
                    now > 2.
                );
            }
        }
    }
    #[test]
    fn landing_requires_ground_clearance_and_separate_teammate_space() {
        assert!(landing(&Collision::default(), Vec3::ZERO, 0., 0, &[]).is_none());
        let floor = Collision::test_solid_floor();
        let a = landing(&floor, Vec3::ZERO, 0., 0, &[]).unwrap();
        let b = landing(&floor, Vec3::ZERO, 0., 1, &[(a, true), (Vec3::ZERO, true)]).unwrap();
        assert!(a.distance(b) >= 0.85);
        assert!(floor.clear_hull(a, HEIGHT) && floor.grounded(a, HEIGHT));
    }
}
