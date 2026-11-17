//! AI ladder traversal along native NAV ladders (`CNavLadder`).
//!
//! Routes report a ladder as its entry point, exit point and the side the
//! climbable face looks toward (`NavLadder::dir`). The actor is moved
//! kinematically: to the foot (or head) of the ladder, along it, then off it
//! onto the arrival area. Ladder columns are authored clear for player hulls,
//! but the hull rubs the wall behind, so the climb itself is not collision
//! swept; the start and the final standing spot are hull-checked instead.
use crate::collision::{Collision, HEIGHT, RADIUS, UNIT};
use bevy::prelude::*;
use std::collections::HashMap;

/// A ladder step taken from a route: climb from `from` to `to` (both on the
/// ladder line, feet height), then walk off toward `exit`.
#[derive(Clone, Copy, Debug)]
pub struct LadderStep {
    pub from: Vec3,
    pub to: Vec3,
    pub exit: Vec3,
    /// Horizontal normal of the climbable face, toward the climber.
    pub normal: Vec3,
}

/// Only the vertical part owns the climbing pose. Approach and dismount
/// retain ordinary locomotion; the face is the side actually chosen by begin.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub facing: Vec3,
    pub descending: bool,
}
impl Pose {
    pub fn activity(self) -> &'static str {
        if self.descending { "ACT_CLIMB_DOWN" } else { "ACT_CLIMB_UP" }
    }
    pub fn rotation(self) -> Quat {
        Quat::from_rotation_y((-self.facing.z).atan2(self.facing.x))
    }
}

#[derive(Clone, Debug)]
struct Climb {
    points: [Vec3; 3],
    stage: usize,
    exit: Vec3,
    elapsed: f32,
    /// Ladder face at the far end (ascent: the hull must step past it).
    face: Option<Vec3>,
    pose: Pose,
}

#[derive(Default)]
pub struct LadderClimbs(HashMap<u64, Climb>, Option<std::sync::Weak<l4d2_formats::bsp::Bsp>>);

/// Horizontal clearance of the hull centre from the ladder face.
const STANDOFF: f32 = RADIUS + 2. * UNIT;
/// Approach and dismount speed (m/s).
const WALK: f32 = 150. * UNIT;

impl LadderClimbs {
    /// Squad ids are reused across chapters; an old column must not move
    /// a newly spawned survivor into coordinates from the previous map.
    pub fn bind_map(&mut self, bsp: &std::sync::Arc<l4d2_formats::bsp::Bsp>) {
        if !self.1.as_ref().is_some_and(|map| map.ptr_eq(&std::sync::Arc::downgrade(bsp))) {
            self.0.clear();
            self.1 = Some(std::sync::Arc::downgrade(bsp));
        }
    }
    pub fn pose(&self, id: u64) -> Option<Pose> {
        self.0.get(&id).filter(|c| c.stage == 1).map(|c| c.pose)
    }
    pub fn active(&self, id: u64) -> bool {
        self.0.contains_key(&id)
    }
    pub fn cancel(&mut self, id: u64) {
        self.0.remove(&id);
    }
    pub fn retain(&mut self, alive: impl Fn(u64) -> bool) {
        self.0.retain(|id, _| alive(*id));
    }
    /// Start a ladder step. The column side is the authored face normal, or
    /// the opposite side if that one is blocked at mid height.
    pub fn begin(&mut self, id: u64, c: &Collision, feet: Vec3, step: LadderStep) -> bool {
        let mid = |n: Vec3| (step.from + step.to) * 0.5 + n * STANDOFF;
        let normal = [step.normal, -step.normal]
            .into_iter()
            .find(|&n| n != Vec3::ZERO && c.clear_hull(mid(n) - Vec3::Y * HEIGHT * 0.5, HEIGHT))
            .unwrap_or(step.normal);
        let foot = step.from + normal * STANDOFF;
        let head = step.to + normal * STANDOFF;
        // Approach at the current height for an ascent; a descent starts out
        // over the drop at the head of the ladder.
        let approach = if step.to.y >= step.from.y {
            Vec3::new(foot.x, feet.y.max(foot.y), foot.z)
        } else {
            foot
        };
        self.0.insert(
            id,
            Climb {
                points: [approach, head, step.exit],
                stage: 0,
                exit: step.exit,
                elapsed: 0.,
                face: (step.to.y > step.from.y).then_some(step.to),
                pose: Pose { facing: -normal, descending: step.to.y < step.from.y },
            },
        );
        true
    }
    /// Advance an active climb; `None` when not climbing (or just finished,
    /// with the actor left at a clear standing spot).
    pub fn advance(
        &mut self,
        id: u64,
        c: &Collision,
        feet: Vec3,
        climb_speed: f32,
        dt: f32,
    ) -> Option<Vec3> {
        let climb = self.0.get_mut(&id)?;
        climb.elapsed += dt;
        // A stuck climb must not hold the actor forever.
        if climb.elapsed > 20. {
            self.0.remove(&id);
            return None;
        }
        let mut position = feet;
        let mut budget = dt;
        while budget > 0. {
            match climb.stage {
                0 | 1 => {
                    let target = climb.points[climb.stage];
                    let speed = if climb.stage == 0 { WALK } else { climb_speed };
                    let delta = target - position;
                    let step = speed * budget;
                    if delta.length() <= step {
                        budget -= delta.length() / speed;
                        position = target;
                        climb.stage += 1;
                    } else {
                        position += delta.normalize() * step;
                        budget = 0.;
                    }
                }
                _ => {
                    // Dismount: move level toward the exit until a standing
                    // spot with support is clear.
                    let flat = (climb.exit - position) * Vec3::new(1., 0., 1.);
                    // After an ascent the centre must be over the top, not
                    // hanging off the lip by the hull's edge.
                    let past = climb.face.is_none_or(|face| {
                        ((position - face) * Vec3::new(1., 0., 1.)).length() >= STANDOFF + 0.1
                    });
                    let landed = past && c.clear_hull(position, HEIGHT) && c.grounded(position, HEIGHT);
                    if landed || flat.length() < 0.05 {
                        self.0.remove(&id);
                        let rest = if c.clear_hull(position, HEIGHT) {
                            c.snap_ground(position + Vec3::Y * 0.05, HEIGHT)
                        } else {
                            position
                        };
                        return Some(rest);
                    }
                    let step = WALK * budget;
                    position += flat.normalize() * step.min(flat.length());
                    budget = 0.;
                    climb.points[2] = position;
                }
            }
        }
        Some(position)
    }
}

/// Opt-in laboratory fixture that still uses squad_ai's real movement,
/// transform and animation paths. Normal sessions never reposition a bot.
pub(crate) fn capture_bot(climbs: &mut LadderClimbs, c: &Collision,
    member: &mut crate::game_state::Companion, index: usize) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if index != 0 || member.state.elapsed != 0.
        || !*ENABLED.get_or_init(|| std::env::args().any(|a| a == "--capture-ai-ladder")) { return; }
    let source = |x: f32, y: f32, z: f32| Vec3::new(x, z, -y) * UNIT;
    member.position = source(3540., 282., 4.);
    member.spawn = member.position;
    climbs.begin(0, c, member.position, LadderStep {
        from: source(3596., 282., 4.), to: source(3596., 282., 160.),
        exit: source(3700., 282., 160.), normal: Vec3::NEG_X,
    });
    println!("AI ladder fixture: bot ascent initialized");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ladder_pose_owns_only_vertical_travel_and_faces_the_chosen_side() {
        let mut c = Collision::test_solid_floor();
        c.test_add_box(Vec3::new(1., 0., -3.), Vec3::new(4., 3., 3.));
        for descending in [false, true] {
            let mut climbs = LadderClimbs::default();
            let (from, to, exit, mut p) = if descending {
                (Vec3::new(1., 3., 0.), Vec3::new(1., 0., 0.),
                    Vec3::new(-1., 0., 0.), Vec3::new(1.6, 3.004, 0.))
            } else {
                (Vec3::new(1., 0., 0.), Vec3::new(1., 3., 0.),
                    Vec3::new(2.5, 3., 0.), Vec3::new(0.2, 0.004, 0.))
            };
            climbs.begin(1, &c, p, LadderStep { from, to, exit, normal: Vec3::NEG_X });
            assert!(climbs.pose(1).is_none(), "approach must walk");
            let mut vertical_ticks = 0;
            for _ in 0..300 {
                let Some(next) = climbs.advance(1, &c, p, 100. * UNIT, 1. / 30.) else { break };
                p = next;
                if let Some(pose) = climbs.pose(1) {
                    vertical_ticks += 1;
                    assert_eq!(pose.descending, descending);
                    assert_eq!(pose.activity(), if descending { "ACT_CLIMB_DOWN" } else { "ACT_CLIMB_UP" });
                    assert!((pose.rotation() * Vec3::X).dot(Vec3::X) > 0.999);
                }
            }
            assert!(vertical_ticks > 20);
            assert!(!climbs.active(1));
            assert!(climbs.pose(1).is_none(), "dismount must restore locomotion");
            climbs.begin(1, &c, p, LadderStep { from, to, exit, normal: Vec3::NEG_X });
            climbs.cancel(1);
            assert!(climbs.pose(1).is_none());
        }
    }
    #[test]
    fn climbs_a_wall_ladder_and_steps_onto_the_roof() {
        // A 3 m wall from x = 1 with a roof on top; ladder face at x = 1.
        let mut c = Collision::test_solid_floor();
        c.test_add_box(Vec3::new(1., 0., -3.), Vec3::new(4., 3., 3.));
        let mut climbs = LadderClimbs::default();
        let feet = Vec3::new(0.2, 0.004, 0.);
        assert!(climbs.begin(1, &c, feet, LadderStep {
            from: Vec3::new(1., 0., 0.),
            to: Vec3::new(1., 3., 0.),
            exit: Vec3::new(2.5, 3., 0.),
            normal: Vec3::NEG_X,
        }));
        let mut p = feet;
        let mut ticks = 0;
        while let Some(next) = climbs.advance(1, &c, p, 200. * UNIT, 1. / 30.) {
            p = next;
            ticks += 1;
            assert!(ticks < 300, "climb never finished at {p:?}");
            if !climbs.active(1) {
                break;
            }
        }
        assert!((p.y - 3.).abs() < 0.05 && p.x > 1. + RADIUS, "ended at {p:?}");
        assert!(c.clear_hull(p, HEIGHT) && c.grounded(p, HEIGHT));
    }
    #[test]
    fn descends_a_wall_ladder_from_the_roof() {
        let mut c = Collision::test_solid_floor();
        c.test_add_box(Vec3::new(1., 0., -3.), Vec3::new(4., 3., 3.));
        let mut climbs = LadderClimbs::default();
        let feet = Vec3::new(1.6, 3.004, 0.);
        climbs.begin(1, &c, feet, LadderStep {
            from: Vec3::new(1., 3., 0.),
            to: Vec3::new(1., 0., 0.),
            exit: Vec3::new(-1., 0., 0.),
            normal: Vec3::NEG_X,
        });
        let mut p = feet;
        for _ in 0..300 {
            match climbs.advance(1, &c, p, 200. * UNIT, 1. / 30.) {
                Some(next) => p = next,
                None => break,
            }
            if !climbs.active(1) {
                break;
            }
        }
        assert!(!climbs.active(1));
        assert!(p.y < 0.05 && p.x < 1. - RADIUS, "ended at {p:?}");
    }
}
#[cfg(test)]
mod streets {
    use super::*;
    /// NAV ladders on c1m2: start in the bottom area, goal in a top area.
    /// Bot (survivor route) and infected routes must climb and arrive.
    #[test]
    fn streets_nav_ladders_are_climbed_by_bots_and_infected() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let map = std::env::var("L4D2R_LADDER_MAP").unwrap_or("c1m2_streets".into());
        let path = root.join(format!("left4dead2/maps/{map}.bsp"));
        if !path.is_file() {
            return;
        }
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let mesh = l4d2_formats::nav::parse_nav(&std::fs::read(path.with_extension("nav")).unwrap()).unwrap();
        let index: HashMap<u32, usize> = mesh.areas.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let world = |p: [f32; 3]| Vec3::from_array(crate::binary::source_position(p));
        let limit: usize = std::env::var("L4D2R_LADDER_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
        let (mut tried, mut ok, mut climbed_total) = (0, 0, 0);
        for ladder in mesh.ladders.iter() {
            let (Some(&bottom), Some(&top)) = (index.get(&ladder.bottom_area), index.get(&ladder.top_forward)) else { continue };
            if tried >= limit { break; }
            for survivor in [true, false] {
                let c = if survivor { collision.for_actor(false, false) } else { collision.for_common() };
                let start = world(mesh.areas[bottom].center()) + Vec3::Y * 0.02;
                let goal = world(mesh.areas[top].center());
                if !c.clear_hull(start, HEIGHT) { continue; }
                tried += 1;
                let mut nav = crate::l4d2r_navigation::Navigation::load(&path);
                let mut climbs = LadderClimbs::default();
                let (mut p, mut vertical, mut climbed, mut arrived) = (start, 0., false, false);
                let dt = 1. / 30.;
                for _ in 0..(30 * 30) {
                    if (goal - p).xz().length() < 1.2 && (goal.y - p.y).abs() < 1. { arrived = true; break; }
                    if let Some(next) = climbs.advance(1, &c, p, 150. * UNIT, dt) { p = next; vertical = 0.; climbed = true; continue; }
                    let toward = if survivor {
                        nav.toward(1, p, goal, dt)
                    } else {
                        nav.begin_infected_tick(&std::collections::HashSet::from([1]));
                        nav.toward_infected(1, p, goal, dt)
                    };
                    let step = if survivor { nav.ladder_survivor(1, p, goal) } else { nav.ladder_infected(1, p, goal) };
                    if let Some(step) = step { climbs.begin(1, &c, p, step); continue; }
                    let wish = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero() * 4.;
                    (p, vertical) = crate::infected_movement::walk(&c, p, wish, vertical, dt);
                }
                climbed_total += climbed as usize;
                if arrived { ok += 1; } else {
                    let s = |v: Vec3| [v.x / UNIT, -v.z / UNIT, v.y / UNIT];
                    println!("LADDERFAIL id={} survivor={survivor} climbed={climbed} start={:?} at={:?} goal={:?}", ladder.id, s(start).map(|x| x.round()), s(p).map(|x| x.round()), s(goal).map(|x| x.round()));
                }
            }
        }
        println!("NAVLADDERS map={map} tried={tried} arrived={ok} climbed={climbed_total}");
        assert!(tried > 0);
        assert!(ok * 10 >= tried * 7, "only {ok}/{tried} ladder routes arrived");
    }
}
