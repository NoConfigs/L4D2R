//! Authored simple-ladder volumes. Movement uses the normal of the climbable
//! face, rather than treating every nearby wall as a ladder.
use crate::collision::{Collision, UNIT};
use bevy::prelude::*;
use l4d2r_game_core::entities::EntityWorld;

#[derive(Resource, Default)]
pub(crate) struct Ladders(Vec<usize>, std::sync::Arc<()>);
#[derive(Default)]
pub(crate) struct State {
    attached: Option<usize>,
    jumped: bool,
    map: Option<std::sync::Arc<()>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::HEIGHT;
    fn lab() -> (EntityWorld, Collision, Ladders) {
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(
            std::fs::read("maps/rewrite_lab.bsp").unwrap()).unwrap());
        let world = EntityWorld::new(bsp.clone(), l4d2r_game_core::mapents::collect(&bsp), 1);
        let collision = Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(
            l4d2r_game_core::trace::Tracer::new(bsp))));
        let ladders = Ladders::load(&world);
        (world, collision, ladders)
    }
    #[test]
    fn authored_teams_idle_and_jump_off_without_an_artificial_cooldown() {
        let (world, collision, ladders) = lab();
        for (team, y) in [(0, 282.), (2, 482.), (1, 682.)] {
            for infected in [false, true] {
                let mut state = State::default();
                let mut feet = Vec3::new(3579.95, 4., -y) * UNIT;
                let mut velocity = Vec3::ZERO;
                let wish = Vec3::X * (100. * UNIT);
                let allowed = team_allowed(team, infected);
                assert_eq!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, wish, Vec3::X, false, infected, 0., 1./30.), allowed);
                if !allowed { continue; }
                assert!((velocity.y / UNIT - 100.).abs() < 0.001);
                let at = feet;
                assert!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, Vec3::ZERO, Vec3::X, false, infected, 0.033, 1./30.));
                assert_eq!(feet, at);
                assert!(!state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, wish, Vec3::X, true, infected, 0.066, 1./30.));
                assert!(state.jumped());
                assert!((velocity.x / UNIT + 270.).abs() < 0.001);
                assert_eq!(feet, at, "jump impulse must be integrated by ordinary movement");
                // A new mount is contact-limited, not a made-up 0.2s timer.
                assert!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, wish, Vec3::X, false, infected, 0.067, 1./30.));
            }
        }
    }
    #[test]
    fn retail_narrow_ladder_pitch_horizon_and_diagonal_buttons() {
        let speed = 100. * UNIT;
        for pitch in [-45_f32, -30., -10., 0., 10., 30., 45.] {
            let forward = Vec3::new(-pitch.to_radians().cos(), -pitch.to_radians().sin(), 0.);
            let expected = if pitch >= 30. { -speed } else { speed };
            let v = climb_velocity(forward * speed, Vec3::X, forward, 24.);
            assert!((v.y - expected).abs() < 0.0001, "pitch {pitch}: {v:?}");
            let diagonal = climb_velocity((forward + forward.cross(Vec3::Y).normalize()) * speed,
                Vec3::X, forward, 24.);
            assert!((diagonal.y - expected).abs() < 0.0001);
            assert!(diagonal.xz().length() < 0.0001);
        }
        let v = climb_velocity(-Vec3::X * speed + Vec3::Z * speed, Vec3::X, -Vec3::X, 64.);
        assert!((v.y / UNIT - 100.).abs() < 0.001);
        assert!((v.z / UNIT - 100.).abs() < 0.001);
    }
    #[test]
    fn retail_100_unit_climb_is_tick_rate_independent_and_does_not_teleport_at_top() {
        let (world, collision, ladders) = lab();
        for hz in [30, 60, 120] {
            let mut state = State::default();
            let mut feet = Vec3::new(3579.95, 24., -282.) * UNIT;
            let mut velocity = Vec3::ZERO;
            for tick in 0..hz {
                assert!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, Vec3::X * 100. * UNIT, Vec3::X, false, false,
                    tick as f32 / hz as f32, 1. / hz as f32));
            }
            assert!((feet.y / UNIT - 124.).abs() < 0.005);
            while feet.y < 160. * UNIT {
                assert!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                    HEIGHT, Vec3::X * 100. * UNIT, Vec3::X, false, false, 2., 1./hz as f32));
            }
            assert!(!state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                HEIGHT, Vec3::X * 100. * UNIT, Vec3::X, false, false, 3., 1./hz as f32));
            assert!((feet.x / UNIT - 3579.95).abs() < 0.005);
            assert!((velocity.y / UNIT - 100.).abs() < 0.001);
        }
    }
    #[test]
    fn streets_reproduces_retail_climb_ticks_and_top_exit_inertia() {
        let path = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/left4dead2/maps/c1m2_streets.bsp");
        if !path.is_file() { return; }
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let world = EntityWorld::new(bsp.clone(), l4d2r_game_core::mapents::collect(&bsp), 1);
        let collision = Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(
            l4d2r_game_core::trace::Tracer::new(bsp))));
        let ladders = Ladders::load(&world);
        let mut state = State::default();
        let mut feet = Vec3::new(-2313.95, 24., -2432.) * UNIT;
        let mut velocity = Vec3::ZERO;
        let dt = 1./30.;
        for tick in 1..=22 {
            assert!(state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
                HEIGHT, -Vec3::X * 100. * UNIT, -Vec3::X, false, false, tick as f32*dt, dt));
            assert!((feet.y / UNIT - (24. + tick as f32 * 100./30.)).abs() < 0.001);
        }
        assert!(!state.step(&ladders, &world, &collision, &mut feet, &mut velocity,
            HEIGHT, -Vec3::X * 100. * UNIT, -Vec3::X, false, false, 23.*dt, dt));
        velocity = crate::player_movement::horizontal(velocity, -Vec3::X * 220. * UNIT, 220.*UNIT, false, dt);
        velocity.y -= crate::player_movement::GRAVITY * dt * 0.5;
        (feet, velocity) = collision.walk_velocity(feet, velocity, dt, HEIGHT, false);
        velocity.y -= crate::player_movement::GRAVITY * dt * 0.5;
        // Retail RL|level at t=.766666 in ladders-retail-1019-first.log.
        assert!((feet.x / UNIT + 2314.95).abs() < 0.003);
        assert!((feet.y / UNIT - 100.22222).abs() < 0.003);
        assert!((velocity.y / UNIT - 73.33333).abs() < 0.001);
    }
    #[test]
    fn running_into_streets_ladders_mounts_for_both_teams() {
        // At run speed a player covers several units a tick and ends up
        // inside the non-solid ladder volume, which stands proud of the wall
        // it is fixed to. That overlap must still grab (it used to be
        // rejected as start-solid, so no campaign ladder could be mounted
        // except by creeping up to it).
        let path = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2/left4dead2/maps/c1m2_streets.bsp");
        if !path.is_file() { return; }
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let world = EntityWorld::new(bsp.clone(), l4d2r_game_core::mapents::collect(&bsp), 1);
        let collision = Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(
            l4d2r_game_core::trace::Tracer::new(bsp))));
        let ladders = Ladders::load(&world);
        // Survivor-usable Hammer 54515 (normal +X) and infected-only 99829.
        for (hammer, infected, start) in [(54515, false, [-2273.95, 2432., 0.03125]), (99829, true, [-4738., -2944., 448.03125])] {
            let id = *ladders.0.iter().find(|&&id| world.ents[id].me.kv_i32("hammerid") == Some(hammer)).unwrap();
            let normal = ladders.surface(&world, id, infected).unwrap().normal;
            let mut feet = Vec3::from_array(crate::binary::source_position(start));
            let start_y = feet.y;
            let mut velocity = -normal * 220. * UNIT;
            let mut state = State::default();
            let dt = 1. / 30.;
            let mut mounted = false;
            for tick in 0..45 {
                let wish = -normal * 220. * UNIT;
                if state.step(&ladders, &world, &collision, &mut feet, &mut velocity, HEIGHT, wish, -normal, false, infected, tick as f32 * dt, dt) {
                    mounted = true;
                } else {
                    (feet, velocity) = collision.walk_velocity(feet, wish, dt, HEIGHT, true);
                }
            }
            assert!(mounted, "hammer {hammer}: never grabbed the ladder");
            assert!(feet.y - start_y > 80. * UNIT, "hammer {hammer}: climbed only {:.1} units", (feet.y - start_y) / UNIT);
        }
    }
    #[test]
    fn airborne_mounts_are_not_subject_to_grounded_view_restrictions() {
        let (world, collision, ladders) = lab();
        for (z, allowed) in [(0.0625, false), (24., true)] {
            let mut feet = Vec3::new(3579.95, z, -282.) * UNIT;
            let mut velocity = Vec3::ZERO;
            let mounted = State::default().step(&ladders, &world, &collision,
                &mut feet, &mut velocity, HEIGHT, Vec3::X * 100. * UNIT,
                -Vec3::X, false, false, 0., 1./30.);
            assert_eq!(mounted, allowed);
        }
        assert!(climb_face_allowed(-Vec3::X, Vec3::Y));
        assert!(climb_face_allowed(Vec3::new(-0.8, 0.6, 0.), -Vec3::X));
        assert!(!climb_face_allowed(-Vec3::X, Vec3::X));
        assert!(!climb_face_allowed(-Vec3::X, -Vec3::Y));
    }
    #[test]
    fn rotated_ladder_uses_the_contact_plane_and_a_wall_in_front_blocks_grabbing() {
        let (mut world, collision, ladders) = lab();
        let id = ladders.0[0];
        world.ents[id].me.angles.y = 45.;
        let matrix = world.world_matrix(id);
        let extent = 16. * 2_f32.sqrt();
        let local = l4d2r_math::Vec3::new(3596. - extent - 0.05, 282., 24.);
        let p = matrix.transform_point3(local);
        let d = matrix.transform_vector3(l4d2r_math::Vec3::X);
        let mut feet = Vec3::new(p.x, p.z, -p.y) * UNIT;
        let forward = Vec3::new(d.x, d.z, -d.y);
        let mut velocity = Vec3::ZERO;
        assert!(State::default().step(&ladders, &world, &collision,
            &mut feet, &mut velocity, HEIGHT, forward * 100. * UNIT,
            forward, false, false, 0., 1./30.));
        assert!((velocity.y / UNIT - 100.).abs() < 0.001);

        let (world, collision, ladders) = lab();
        let start = Vec3::new(3579.6, 24., -282.) * UNIT;
        let mut feet = start;
        let mut velocity = Vec3::ZERO;
        assert!(State::default().step(&ladders, &world, &collision, &mut feet, &mut velocity,
            HEIGHT, Vec3::X * 100. * UNIT, Vec3::X, false, false, 0., 1./30.));
        use l4d2r_game_core::trace::{Tracer, Collider, ColliderShape, CONTENTS_SOLID};
        let mut tracer = Tracer::new(world.bsp.clone());
        tracer.set_owned_colliders(vec![(usize::MAX, Collider { active: true,
            shape: ColliderShape::OrientedBox {
                mins: l4d2r_math::Vec3::new(3595.7, 270., 0.),
                maxs: l4d2r_math::Vec3::new(3595.8, 294., 200.),
                matrix: l4d2r_math::Mat4::IDENTITY, contents: CONTENTS_SOLID,
            },
        })]);
        let blocked = Collision::test_native(std::sync::Arc::new(std::sync::Mutex::new(tracer)));
        feet = start;
        assert!(!State::default().step(&ladders, &world, &blocked, &mut feet, &mut velocity,
            HEIGHT, Vec3::X * 100. * UNIT, Vec3::X, false, false, 0., 1./30.));
    }
}
#[derive(Clone, Copy)]
struct Surface {
    id: usize,
    model: usize,
    matrix: l4d2r_math::Mat4,
    normal: Vec3,
    width: f32,
}
fn climb_face_allowed(authored: Vec3, hit: Vec3) -> bool {
    // Retail validates horizontal alignment, allowing the top-facing cap.
    authored.y >= 1. || hit.y >= 1. || authored.with_y(0.).normalize_or_zero()
        .dot(hit.with_y(0.).normalize_or_zero()) >= 0.9
}
pub(crate) fn team_allowed(team: i32, infected: bool) -> bool {
    team == 0 || team == if infected { 2 } else { 1 }
}
impl Ladders {
    pub(crate) fn verify_scene(&self, world: &EntityWorld) {
        let expected = Self::load(world);
        assert_eq!(self.0, expected.0, "Ladder registry belongs to a previous map");
        println!("Ladder reload check: {} authored ladders registered", self.0.len());
    }
    pub(crate) fn load(world: &EntityWorld) -> Self {
        Self(world.ents.iter().enumerate().filter_map(|(id, e)|
            (e.me.classname == "func_simpleladder" && e.me.brush.is_some()).then_some(id)
        ).collect(), std::sync::Arc::new(()))
    }
    fn surface(&self, world: &EntityWorld, id: usize, infected: bool) -> Option<Surface> {
        let e = world.ents.get(id)?;
        if e.me.classname != "func_simpleladder" || !e.alive
            || !team_allowed(e.me.kv_i32("team").unwrap_or(0), infected) { return None; }
        let model = e.me.brush?;
        let bounds = world.bsp.models.get(model)?;
        let matrix = world.world_matrix(id);
        let n = matrix.transform_vector3(l4d2r_math::Vec3::new(
            e.me.kv_f32("normal.x").unwrap_or(0.),
            e.me.kv_f32("normal.y").unwrap_or(0.),
            e.me.kv_f32("normal.z").unwrap_or(0.),
        ));
        let normal = Vec3::new(n.x, n.z, -n.y).normalize_or_zero();
        if normal == Vec3::ZERO { return None; }
        let width = if e.me.kv_f32("normal.x").unwrap_or(0.) == 0. {
            bounds.maxs.x - bounds.mins.x
        } else { bounds.maxs.y - bounds.mins.y };
        Some(Surface { id, model, matrix, normal, width })
    }
}
// Retail server.dll 100fb970: button magnitudes are not normalized together.
// Narrow ladders use ladder_autocenter=1, ladder_horizon=-0.3. Wide ladders
// and predominantly sideways wishes retain the full face projection.
fn climb_velocity(wish: Vec3, normal: Vec3, forward: Vec3, width: f32) -> Vec3 {
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let fwd = wish.dot(forward);
    let side = wish.dot(right);
    let facing = forward.with_y(0.).normalize_or_zero().dot(-normal);
    let up = normal.cross(Vec3::Y.cross(normal).normalize_or_zero());
    if width < 48. && ((facing.abs() >= 0.5 && fwd != 0. && side.abs() <= fwd.abs() + 0.0001)
        || forward.y.abs() == 1.) {
        let (direction, horizon) = if fwd < 0. { (-forward, 0.3) } else { (forward, -0.3) };
        return up * fwd.abs() * if direction.dot(up) <= horizon { -1. } else { 1. };
    }
    let into = wish.dot(normal);
    wish - normal * into - up * into
}
impl State {
    pub(crate) fn reset(&mut self) { self.attached = None; self.jumped = false; }
    pub(crate) fn jumped(&self) -> bool { self.jumped }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step(&mut self, ladders: &Ladders, world: &EntityWorld, collision: &Collision,
        feet: &mut Vec3, velocity: &mut Vec3, height: f32, wish: Vec3,
        forward: Vec3, jump: bool, infected: bool, _now: f32, dt: f32) -> bool {
        // Local system state survives scene replacement, but an attachment
        // must never carry over to a different map (or a reload of this one).
        if !self.map.as_ref().is_some_and(|map| std::sync::Arc::ptr_eq(map, &ladders.1)) {
            self.reset();
            self.map = Some(ladders.1.clone());
        }
        self.jumped = false;
        let attached = self.attached.and_then(|id| ladders.surface(world, id, infected));
        let direction = if let Some(surface) = attached { -surface.normal }
            else { wish.normalize_or_zero() };
        if direction == Vec3::ZERO { self.reset(); return false; }
        let reach = if attached.is_some() { 10. } else { 0.5 } * UNIT;
        let end = *feet + direction * reach;
        let grounded_mount = attached.is_none() && collision.grounded(*feet, height);
        let center = Vec3::Y * (height * 0.5);
        let blocking = collision.trace(*feet + center, end + center,
            Vec3::new(crate::collision::RADIUS, height * 0.5, crate::collision::RADIUS));
        let candidate = ladders.0.iter().filter_map(|&id| {
            let surface = ladders.surface(world, id, infected)?;
            let mut hit = collision.ladder_trace(surface.model, surface.matrix, *feet, end, height)?;
            let inside = hit.start_solid;
            if hit.start_solid {
                // Ladder brushes are not solid, so a player walking in at
                // run speed (3-7 units a tick) passes the short probe and
                // ends up inside the volume; usually it stands proud of the
                // wall the player then stops against. Source's ladder trace
                // still reports that overlap. Accept it only while pushing
                // into the authored climbable face, so overlapping from the
                // side or back of the brush does not grab.
                if direction.dot(-surface.normal) < 0.5 { return None; }
                hit.fraction = 0.;
                hit.normal = surface.normal;
            }
            // Retail filters the actual hit face against m_climbableNormal.
            // Bounds overlap alone accepts back faces, corners and walls.
            if hit.fraction >= 1. || !climb_face_allowed(surface.normal, hit.normal)
                // A wall in front stops the grab; a climbable face within
                // 0.15 units behind the wall face still counts (c1m2's
                // slanted infected ladders sit 0.09 units behind it).
                || blocking.start_solid
                || (blocking.fraction - hit.fraction) * reach < -0.15 * UNIT { return None; }
            if grounded_mount && (forward.with_y(0.).normalize_or_zero()
                .dot(-hit.normal.with_y(0.).normalize_or_zero()) < 0.5 || forward.y.abs() == 1.) {
                return None;
            }
            Some((surface, hit, inside))
        }).min_by(|a, b| a.1.fraction.total_cmp(&b.1.fraction));
        let Some((surface, hit, inside)) = candidate else { self.reset(); return false; };
        if inside && self.attached.is_none() {
            // Grabbed from inside the volume: retail grabs at the climbable
            // face, so step back out to it. Climbing pressed against the
            // wall behind caught overhead lips the face clears (c1m3).
            let back = *feet + surface.normal * (24. * UNIT);
            if let Some(face) = collision.ladder_trace(surface.model, surface.matrix, back, *feet, height) {
                if !face.start_solid && face.fraction < 1. {
                    let out = back + (*feet - back) * face.fraction + surface.normal * (0.03 * UNIT);
                    if collision.clear_hull(out, height) {
                        *feet = out;
                    }
                }
            }
        }
        self.attached = Some(surface.id);
        if jump {
            *velocity = hit.normal * (270. * UNIT);
            self.reset(); self.jumped = true;
            // FullWalkMove runs in the same tick: air acceleration/gravity
            // must operate on this impulse before integrating the position.
            return false;
        }
        *velocity = climb_velocity(wish, hit.normal, forward, surface.width);
        if wish.dot(hit.normal) > 0. && collision.grounded(*feet, height) {
            *velocity += hit.normal * (200. * UNIT);
        }
        (*feet, *velocity) = collision.walk_velocity(*feet, *velocity, dt, height, false);
        // Descending onto a floor releases the ladder and resumes walking.
        if velocity.y <= 0. && collision.grounded(*feet, height) && wish.dot(surface.normal) >= 0. {
            self.reset();
        }
        true
    }
}

#[cfg(test)]
mod survey {
    use super::*;
    use crate::collision::{HEIGHT, RADIUS};
    /// Every func_simpleladder on the listed maps: stand at the foot of its
    /// climbable face (centre, then sideways offsets), run into it and report
    /// how far up it gets.
    #[test]
    #[ignore = "manual ladder survey on installed campaign maps"]
    fn ladder_survey() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let maps = std::env::var("L4D2R_LADDER_MAPS").unwrap_or("c1m1_hotel c1m2_streets c1m3_mall c1m4_atrium".into());
        for map in maps.split_whitespace() {
            let path = root.join(format!("left4dead2/maps/{map}.bsp"));
            let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
            let mut collision = Collision::load(&path).unwrap();
            collision.enable_l4d2r(root, &logic).unwrap();
            let world = &logic.world;
            let ladders = Ladders::load(world);
            let (mut ok, mut bad) = (0, 0);
            for &id in &ladders.0 {
                let team = world.ents[id].me.kv_i32("team").unwrap_or(0);
                let infected = team == 2;
                let c = collision.for_actor(infected, false);
                let (Some(surface), Some((lo, hi))) = (ladders.surface(world, id, infected), logic.bounds(id)) else {
                    println!("LADDER {map} id={id} no surface");
                    bad += 1;
                    continue;
                };
                let n = surface.normal;
                let side = n.cross(Vec3::Y).normalize_or_zero();
                let center = (lo + hi) * 0.5;
                let half_along = ((hi - lo) * 0.5).dot(n.abs());
                let height = hi.y - lo.y;
                let climb = |offset: f32| -> Option<f32> {
                    let base = Vec3::new(center.x, lo.y + 0.03, center.z) + side * offset * UNIT;
                    let mut feet = c.snap_ground(base + n * (half_along + RADIUS + 2. * UNIT) + Vec3::Y * 0.3, HEIGHT);
                    if !c.clear_hull(feet, HEIGHT) {
                        return None;
                    }
                    let start = feet.y;
                    let (mut velocity, mut state, mut top) = (Vec3::ZERO, State::default(), feet.y);
                    let dt = 1. / 30.;
                    for tick in 0..200 {
                        let wish = -n * 220. * UNIT; // run speed overshoots the short probe
                        if !state.step(&ladders, world, &c, &mut feet, &mut velocity, HEIGHT, wish, -n, false, infected, tick as f32 * dt, dt) {
                            (feet, velocity) = c.walk_velocity(feet, crate::player_movement::horizontal(velocity, wish, 220. * UNIT, true, dt) - Vec3::Y * crate::player_movement::GRAVITY * dt, dt, HEIGHT, c.grounded(feet, HEIGHT));
                        }
                        top = top.max(feet.y);
                    }
                    Some(top - start)
                };
                let width = surface.width * UNIT;
                let offsets = [0., -0.3, 0.3, -0.45, 0.45].map(|f| f * width / UNIT);
                let results: Vec<Option<f32>> = offsets.iter().map(|&o| climb(o)).collect();
                let best = results.iter().flatten().copied().fold(f32::NAN, f32::max);
                if best.is_nan() || best < height * 0.6 {
                    bad += 1;
                    let s = [center.x / UNIT, -center.z / UNIT, lo.y / UNIT];
                    println!("LADDER {map} id={id} hammer={:?} team={team} FAIL at=[{:.0},{:.0},{:.0}] ladder_h={:.0}u best_gain={:.0}u starts_clear={}",
                        world.ents[id].me.kv_i32("hammerid"), s[0], s[1], s[2], height / UNIT, best / UNIT, results.iter().filter(|r| r.is_some()).count());
                } else {
                    ok += 1;
                }
            }
            println!("LADDERS {map} total={} ok={ok} fail={bad}", ladders.0.len());
        }
    }
}
