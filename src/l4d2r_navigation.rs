//! l4d2r A* routes over the installed game's NAV areas; coordinates are converted at the boundary.
use crate::{binary, l4d2r_navgraph::NavGraph};
use bevy::prelude::*;
use l4d2_formats::nav::parse_nav;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};
/// Deepest NAV drop (Source units) an AI special routes over. Retail bots
/// refuse path steps below their locomotion death-drop height; without a
/// cap a Flee point past a roof edge sent specials down into the
/// trigger_hurt under c1m1's hotel.
const SPECIAL_MAX_DROP: f32 = 400.;
#[derive(Resource, Default)]
pub struct Navigation {
    graph: Option<Mutex<NavGraph>>,
    routes: HashMap<Actor, Route>,
    infected_budget: usize,
    /// The current query is an AI special's (see `SPECIAL_MAX_DROP`).
    special_query: bool,
    steering: HashMap<usize, (Vec3, Vec3, Vec3, f32)>,
    blockers: HashMap<usize, ((Vec3, Vec3), u8)>,
    map_time: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn survivor_routes_skip_infected_only_ladders() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut tagged = 0;
        for map in ["c1m2_streets", "c2m1_highway", "c5m2_park", "c8m2_subway", "c11m3_garage"] {
            let path = root.join(format!("left4dead2/maps/{map}.bsp"));
            let Ok(logic) = crate::l4d2r_cinematic::MapLogic::load(root, &path, true) else { continue };
            let mut nav = Navigation::load(&path);
            let ladders: Vec<_> = logic.world.ents.iter().enumerate()
                .filter(|(_, e)| e.me.classname == "func_simpleladder")
                .filter_map(|(id, e)| Some((logic.bounds(id)?, e.me.kv_i32("team").unwrap_or(0))))
                .filter(|(_, t)| *t != 0)
                .collect();
            nav.set_ladder_teams(&ladders);
            let graph = nav.graph.as_ref().unwrap().lock().unwrap();
            let infected_only: Vec<usize> = (0..graph.mesh.ladders.len())
                .filter(|&li| {
                    graph.query_team.set(1);
                    let survivors = graph.ladder_usable(li);
                    graph.query_team.set(2);
                    let infected = graph.ladder_usable(li);
                    infected && !survivors
                })
                .collect();
            println!("{map}: {} team ladders, {} NAV ladders, {} infected-only", ladders.len(), graph.mesh.ladders.len(), infected_only.len());
            tagged += infected_only.len();
            // A survivor route from an infected ladder's bottom never climbs it.
            for &li in &infected_only {
                let l = &graph.mesh.ladders[li];
                let (Some(from), Some(top)) = (
                    graph.mesh.areas.iter().position(|a| a.id == l.bottom_area),
                    graph.mesh.areas.iter().position(|a| a.id == l.top_forward || a.id == l.top_behind || a.id == l.top_left || a.id == l.top_right),
                ) else { continue };
                graph.query_team.set(1);
                if let Some(edges) = graph.route_edges(from, top) {
                    assert!(edges.iter().all(|(_, via)| *via != Some(li)), "{map}: survivor route used infected ladder {li}");
                }
                graph.query_team.set(2);
                assert!(graph.route_edges(from, top).is_some());
            }
        }
        assert!(tagged > 0, "no infected-only NAV ladders found");
    }
    use super::*;
    #[test]
    fn rescue_vehicle_api_unblocks_native_areas_without_bypassing_map_blockers() {
        let mut mesh=parse_nav(&std::fs::read(crate::test_lab::path().with_extension("nav")).unwrap()).unwrap();
        let to=mesh.areas[0].connections.iter().flatten().next().copied().unwrap();
        let to=mesh.areas.iter().position(|a|a.id==to).unwrap();
        for (id,area) in mesh.areas.iter_mut().enumerate() { if id!=0 { area.spawn_attributes|=0x8000; } }
        let mut nav=Navigation{graph:Some(Mutex::new(NavGraph::new(Arc::new(mesh)))),..default()};
        assert!(nav.graph.as_ref().unwrap().lock().unwrap().route(0,to).is_none());
        nav.rescue_vehicle_access(true);
        assert!(nav.graph.as_ref().unwrap().lock().unwrap().route(0,to).is_some());
        let point={let g=nav.graph.as_ref().unwrap().lock().unwrap();Vec3::from_array(binary::source_position(g.area_center(to).to_array()))};
        nav.map_blocker(7,(point-Vec3::splat(0.01),point+Vec3::splat(0.01)),true);
        assert!(nav.graph.as_ref().unwrap().lock().unwrap().route(0,to).is_none());
        nav.map_blocker(7,(point,point),false);
        assert!(nav.graph.as_ref().unwrap().lock().unwrap().route(0,to).is_some());
        nav.rescue_vehicle_access(false);
        assert!(nav.graph.as_ref().unwrap().lock().unwrap().route(0,to).is_none());
    }
    #[test]
    fn team_blockers_overlap_and_unblock_independently() {
        let mut nav = Navigation::load(&crate::test_lab::path());
        let (from, to, bounds) = {
            let graph = nav.graph.as_ref().unwrap().lock().unwrap();
            let from = 0;
            let id = *graph.mesh.areas[from]
                .connections
                .iter()
                .flatten()
                .next()
                .unwrap();
            let to = graph.mesh.areas.iter().position(|a| a.id == id).unwrap();
            let p = Vec3::from_array(binary::source_position(graph.area_center(to).to_array()));
            (from, to, (p - Vec3::splat(0.01), p + Vec3::splat(0.01)))
        };
        for (team, mask) in [(2, 1), (3, 2)] {
            nav.map_blocker_team(99, bounds, true, team);
            let graph = nav.graph.as_ref().unwrap().lock().unwrap();
            for query in [1, 2] {
                graph.query_team.set(query);
                assert_eq!(graph.route(from, to).is_none(), query == mask);
            }
        }
        nav.map_blocker_team(98, bounds, true, 2);
        {
            let graph = nav.graph.as_ref().unwrap().lock().unwrap();
            for query in [1, 2] {
                graph.query_team.set(query);
                assert!(graph.route(from, to).is_none());
            }
        }
        nav.map_blocker_team(99, bounds, false, 3);
        let graph = nav.graph.as_ref().unwrap().lock().unwrap();
        graph.query_team.set(1);
        assert!(graph.route(from, to).is_none());
        graph.query_team.set(2);
        assert!(graph.route(from, to).is_some());
    }
    #[test]
    fn unchanged_blockers_preserve_routes_and_recovery_only_resets_its_bot() {
        let mut nav = Navigation::default();
        let bounds = (Vec3::ZERO, Vec3::ONE);
        nav.map_blocker(3, bounds, true);
        for actor in [Actor::Survivor(0), Actor::Survivor(1), Actor::Infected(2)] {
            nav.routes.insert(
                actor,
                Route {
                    target: Vec3::ZERO,
                    points: vec![],
                    next: 0,
                    refresh: 1.,
                    ready: true,
                    avoid: vec![],
                    avoid_time: 0.,
                    ladders: vec![],
                },
            );
        }
        nav.steering.insert(0, (Vec3::ZERO, Vec3::X, Vec3::X, 1.));
        nav.map_blocker(3, bounds, true);
        nav.map_blocker(4, bounds, false);
        assert_eq!(nav.routes.len(), 3);
        assert_eq!(nav.steering.len(), 1);
        nav.reset_survivor(0);
        assert_eq!(nav.routes.len(), 2);
        assert!(nav.routes.contains_key(&Actor::Infected(2)));
        assert!(nav.steering.is_empty());
        nav.map_blocker(3, (Vec3::ZERO, Vec3::splat(2.)), true);
        assert!(nav.routes.is_empty());
    }
    #[test]
    fn infected_route_budget_ids_blockers_and_cleanup_are_bounded() {
        let mut nav = Navigation::load(&crate::test_lab::path());
        let start = Vec3::from_array(binary::source_position([1700., 600., 4.]));
        let goal = Vec3::from_array(binary::source_position([1920., 600., 4.]));
        let ids = [u64::MAX - 3, u64::MAX - 2, u64::MAX - 1];
        let alive = HashSet::from(ids);
        nav.begin_infected_tick(&alive);
        for id in ids.iter().take(2) {
            assert!(nav.toward_infected(*id, start, goal, 0.016).length() > 0.1);
        }
        // Past the budget a common heads straight for its target until its
        // own search runs.
        assert_eq!(nav.toward_infected(ids[2], start, goal, 0.016), goal - start);
        assert_eq!(nav.infected_budget, 0);
        assert!(nav.routes.values().all(|r| r.refresh < 2.));
        nav.begin_infected_tick(&alive);
        assert!(nav.toward_infected(ids[2], start, goal, 0.016).length() > 0.1);
        nav.toward(0, start, goal, 0.016);
        nav.map_blocker(1, (Vec3::splat(-1000.), Vec3::splat(1000.)), true);
        nav.begin_infected_tick(&alive);
        assert_eq!(nav.toward_infected(ids[0], start, goal, 0.016), Vec3::ZERO);
        nav.map_blocker(1, (Vec3::ZERO, Vec3::ZERO), false);
        nav.begin_infected_tick(&alive);
        assert!(nav.toward_infected(ids[0], start, goal, 0.016).length() > 0.1);
        nav.toward(0, start, goal, 0.016);
        nav.begin_infected_tick(&HashSet::new());
        assert_eq!(nav.routes.len(), 1);
        assert!(nav.routes.contains_key(&Actor::Survivor(0)));
    }
}
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum Actor {
    Survivor(usize),
    Infected(u64),
}
struct Route {
    target: Vec3,
    points: Vec<Vec3>,
    next: usize,
    refresh: f32,
    ready: bool,
    /// Connections this actor got stuck on, avoided by its own searches
    /// until the timer runs out.
    avoid: Vec<(usize, usize)>,
    avoid_time: f32,
    /// Per waypoint: a NAV ladder starting there (its far end and the
    /// climbable face normal, world space).
    ladders: Vec<Option<(Vec3, Vec3)>>,
}
fn source(p: Vec3) -> l4d2r_math::Vec3 {
    l4d2r_math::Vec3::new(p.x, -p.z, p.y) / 0.0254
}
impl Navigation {
    pub fn load(world: &Path) -> Self {
        match std::fs::read(world.with_extension("nav"))
            .ok()
            .and_then(|b| parse_nav(&b).ok())
        {
            Some(mesh) => {
                println!("l4d2r navigation: {} native areas", mesh.areas.len());
                Self {
                    graph: Some(Mutex::new(NavGraph::new(Arc::new(mesh)))),
                    routes: HashMap::new(),
                    infected_budget: 0,
                    special_query: false,
                    steering: HashMap::new(),
                    blockers: HashMap::new(),
                    map_time: 0.,
                }
            }
            None => {
                eprintln!("No readable native NAV mesh: {}", world.display());
                Self::default()
            }
        }
    }
    pub fn rescue_vehicle_access(&mut self, unblocked: bool) {
        if let Some(graph)=&self.graph {
            let graph=graph.lock().unwrap();
            if graph.rescue_vehicle_unblocked.replace(unblocked)!=unblocked {
                self.routes.clear(); self.steering.clear();
            }
        }
    }
    pub fn sync_map_time(&mut self, now: f32) {
        if now < self.map_time {
            self.blockers.clear();
            self.routes.clear();
            self.steering.clear();
            if let Some(graph) = &self.graph {
                graph.lock().unwrap().clear_area_blocks();
            }
        }
        self.map_time = now;
    }
    /// Team-only ladders (func_simpleladder `team`: 1 survivors, 2 infected),
    /// as (Bevy-space bounds, team). Survivor bots do not route up infected
    /// ladders, and infected do not route up survivor ladders.
    pub fn set_ladder_teams(&mut self, ladders: &[((Vec3, Vec3), i32)]) {
        if let Some(graph) = &self.graph {
            let source_ladders: Vec<_> = ladders
                .iter()
                .map(|&((lo, hi), team)| {
                    let (a, b) = (source(lo), source(hi));
                    (a.min(b), b.max(a), team)
                })
                .collect();
            graph.lock().unwrap().set_ladder_teams(&source_ladders, 24.);
        }
        self.routes.clear();
        self.steering.clear();
    }
    pub fn map_blocker(&mut self, id: usize, bounds: (Vec3, Vec3), active: bool) {
        self.map_blocker_team(id, bounds, active, 0);
    }
    pub fn map_blocker_team(&mut self, id: usize, bounds: (Vec3, Vec3), active: bool, team: i32) {
        let mask = match team {
            2 => 1,
            3 => 2,
            _ => 3,
        };
        if (active && self.blockers.get(&id) == Some(&(bounds, mask)))
            || (!active && !self.blockers.contains_key(&id))
        {
            return;
        }
        if active {
            self.blockers.insert(id, (bounds, mask));
        } else {
            self.blockers.remove(&id);
        }
        if let Some(graph) = &self.graph {
            let graph = graph.lock().unwrap();
            graph.clear_area_blocks();
            for &((lo, hi), mask) in self.blockers.values() {
                let a = source(lo);
                let b = source(hi);
                graph.set_blocked_in_team(a.min(b), a.max(b), true, mask);
            }
        }
        self.routes.clear();
        self.steering.clear();
    }
    /// Whether the NAV area under a Bevy-space point is a checkpoint
    /// (NAV_SPAWN_CHECKPOINT 0x800); None off the mesh.
    pub fn checkpoint_at(&self, position: Vec3) -> Option<bool> {
        let graph = self.graph.as_ref()?.lock().ok()?;
        let area = graph.find_area(source(position + Vec3::Y * 0.4))?;
        Some(graph.mesh.areas[area].spawn_attributes & 0x800 != 0)
    }
    /// NAV area index under a Bevy-space point.
    pub fn area_index(&self, position: Vec3) -> Option<usize> {
        let graph = self.graph.as_ref()?.lock().ok()?;
        graph.find_area(source(position + Vec3::Y * 0.4))
    }
    /// Like `checkpoint_at`, but an off-mesh point (an item on a table by a
    /// wall) uses the nearest NAV area.
    pub fn checkpoint_near(&self, position: Vec3) -> Option<bool> {
        let graph = self.graph.as_ref()?.lock().ok()?;
        let area = graph.nearest_area(source(position + Vec3::Y * 0.4))?;
        Some(graph.mesh.areas[area].spawn_attributes & 0x800 != 0)
    }
    /// Whether a survivor at `position` has a NAV path to an object at
    /// `target` (retail SurvivorUseObject reachability, server 1039a450 /
    /// 1039a260: the object's area is the nearest within 200 units).
    /// None without a NAV mesh.
    pub fn survivor_reachable(&self, position: Vec3, target: Vec3) -> Option<bool> {
        let graph = self.graph.as_ref()?.lock().ok()?;
        graph.query_team.set(1);
        let Some(from) = graph.nearest_area(source(position + Vec3::Y * 0.4)) else {
            return Some(false);
        };
        let Some(to) = graph.nearest_area_within(source(target + Vec3::Y * 0.4), 200.) else {
            return Some(false);
        };
        Some(from == to || graph.route(from, to).is_some())
    }
    pub fn toward(&mut self, id: usize, position: Vec3, target: Vec3, dt: f32) -> Vec3 {
        self.toward_actor(Actor::Survivor(id), position, target, dt)
    }
    pub fn reset_survivor(&mut self, id: usize) {
        self.routes.remove(&Actor::Survivor(id));
        self.steering.remove(&id);
    }
    pub fn steer(
        &mut self,
        id: usize,
        collision: &crate::collision::Collision,
        position: Vec3,
        toward: Vec3,
        height: f32,
        now: f32,
    ) -> Vec3 {
        let desired = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
        if let Some((previous, wish, direction, until)) = self.steering.get(&id) {
            if now < *until
                && position.distance_squared(*previous) < 0.09
                && desired.dot(*wish) > 0.98
            {
                return *direction;
            }
        }
        let direction = collision.steer(position, toward, height);
        self.steering
            .insert(id, (position, desired, direction, now + 0.2));
        direction
    }
    pub fn begin_infected_tick(&mut self, alive: &HashSet<u64>) {
        self.infected_budget = 2;
        self.routes.retain(|actor, _| match actor {
            Actor::Survivor(_) => true,
            Actor::Infected(id) => alive.contains(id),
        });
    }
    /// An infected made no progress for a while: forbid, for its own
    /// searches only, the connection its route starts with and search again,
    /// instead of grinding along whatever it cannot get past.
    pub fn reroute_infected(&mut self, id: u64, position: Vec3, target: Vec3) {
        let (Some(graph), Some(route)) = (&self.graph, self.routes.get_mut(&Actor::Infected(id))) else {
            return;
        };
        let graph = graph.lock().unwrap();
        graph.query_team.set(2);
        if let Some(first) = graph.first_edge_avoiding(source(position), source(target), &route.avoid) {
            if route.avoid.len() >= 4 {
                route.avoid.remove(0);
            }
            route.avoid.push(first);
        }
        route.avoid_time = 8.;
        route.refresh = 0.;
    }
    /// The ladder step to take now: the actor has reached the foot (or head)
    /// of a NAV ladder on its route. Consumes the waypoint.
    fn take_ladder(&mut self, actor: Actor, position: Vec3, target: Vec3) -> Option<crate::ai_ladder::LadderStep> {
        let route = self.routes.get_mut(&actor)?;
        let (top, normal) = (*route.ladders.get(route.next)?)?;
        let from = route.points[route.next];
        let flat = ((from - position) * Vec3::new(1., 0., 1.)).length();
        if flat > 0.9 || (from.y - position.y).abs() > 1.2 {
            return None;
        }
        route.next += 1;
        let exit = route.points.get(route.next).copied().unwrap_or(target);
        Some(crate::ai_ladder::LadderStep { from, to: top, exit, normal })
    }
    pub fn ladder_infected(&mut self, id: u64, position: Vec3, target: Vec3) -> Option<crate::ai_ladder::LadderStep> {
        self.take_ladder(Actor::Infected(id), position, target)
    }
    pub fn ladder_survivor(&mut self, id: usize, position: Vec3, target: Vec3) -> Option<crate::ai_ladder::LadderStep> {
        self.take_ladder(Actor::Survivor(id), position, target)
    }
    pub fn toward_infected(&mut self, id: u64, position: Vec3, target: Vec3, dt: f32) -> Vec3 {
        self.toward_actor(Actor::Infected(id), position, target, dt)
    }
    /// AI special infected route outside the commons' two-searches-per-tick
    /// budget: at most a few specials live (z_minion_limit), and a horde
    /// would otherwise starve them of paths.
    pub fn toward_special(&mut self, id: u64, position: Vec3, target: Vec3, dt: f32) -> Vec3 {
        let saved = self.infected_budget;
        self.infected_budget = 1;
        self.special_query = true;
        let toward = self.toward_actor(Actor::Infected(id), position, target, dt);
        self.special_query = false;
        self.infected_budget = saved;
        toward
    }
    #[cfg(test)]
    pub(crate) fn debug_infected(&self, id: u64) -> String {
        self.routes.get(&Actor::Infected(id)).map_or_else(
            || "none".into(),
            |r| {
                format!(
                    "target={:?} next={} points={:?}",
                    r.target, r.next, r.points
                )
            },
        )
    }
    fn toward_actor(&mut self, actor: Actor, position: Vec3, target: Vec3, dt: f32) -> Vec3 {
        let Some(graph) = &self.graph else {
            return target - position;
        };
        let route = self.routes.entry(actor).or_insert(Route {
            target: Vec3::splat(f32::INFINITY),
            points: vec![],
            next: 0,
            refresh: 0.,
            ready: false,
            avoid: vec![],
            avoid_time: 0.,
            ladders: vec![],
        });
        route.refresh -= dt;
        route.avoid_time -= dt;
        if route.avoid_time <= 0. {
            route.avoid.clear();
        }
        let infected = matches!(actor, Actor::Infected(_));
        if (route.refresh <= 0. || route.target.distance(target) > 1.5)
            && (!infected || self.infected_budget > 0)
        {
            if infected {
                self.infected_budget -= 1;
            }
            let graph = graph.lock().unwrap();
            graph.query_team.set(if infected { 2 } else { 1 });
            graph.max_drop.set(if self.special_query { SPECIAL_MAX_DROP } else { f32::INFINITY });
            let world = |p: l4d2r_math::Vec3| Vec3::from_array(binary::source_position(p.to_array()));
            let points = graph
                .path_points_avoiding(source(position), source(target), &route.avoid)
                .or_else(|| {
                    // The stuck edge was the only way: keep trying it.
                    route.avoid.clear();
                    graph.path_points(source(position), source(target))
                })
                .unwrap_or_default();
            graph.max_drop.set(f32::INFINITY);
            route.points = points.iter().map(|p| world(p.pos)).collect();
            route.ladders = points
                .iter()
                .map(|p| {
                    p.ladder_top.map(|top| {
                        let n = p.ladder_normal;
                        (world(top), Vec3::new(n.x, 0., -n.y))
                    })
                })
                .collect();
            route.next = 0;
            route.target = target;
            let bucket = match actor {
                Actor::Survivor(id) => id as u64,
                Actor::Infected(id) => id,
            };
            route.refresh = 1. + (bucket % 8) as f32 * 0.13;
            route.ready = true;
        }
        // A common still queued behind the two-searches-per-tick budget heads
        // straight for its target: a retail horde breaks into its run on the
        // same tick (c1m1_hotel recording), not as its paths come in.
        if !route.ready && infected {
            return target - position;
        }
        if !route.ready || route.points.is_empty() {
            return Vec3::ZERO;
        }
        // Portal waypoints sit at the average height of the two NAV areas, so
        // on stairs, slopes and drops they can be well above or below the
        // feet: arrival is horizontal. A waypoint already passed along the
        // route is skipped instead of being walked back to.
        if !infected {
            // Survivor bots keep their original 3D arrival radius. A ladder
            // waypoint is left for `take_ladder`.
            while route.next < route.points.len()
                && route.ladders.get(route.next).is_none_or(|l| l.is_none())
                && (route.points[route.next] - position).length() < 0.55
            {
                route.next += 1;
            }
            return route.points.get(route.next).copied().unwrap_or(target) - position;
        }
        while route.next < route.points.len() {
            if route.ladders.get(route.next).is_some_and(|l| l.is_some()) {
                break; // left for `take_ladder`
            }
            let here = route.points[route.next];
            let flat = (here - position) * Vec3::new(1., 0., 1.);
            let arrived = flat.length() < 0.55 && (here.y - position.y).abs() < 2.5;
            let passed = route.points.get(route.next + 1).is_some_and(|after| {
                let along = (*after - here) * Vec3::new(1., 0., 1.);
                // Only nearby waypoints on this level: on switchback stairs the
                // flight above or below is "past" a waypoint too, and skipping
                // to it sent commons over the rail toward it.
                along.length_squared() > 1e-6 && (position - here).dot(along) > 0.
                    && flat.length() < 1.5
                    && (here.y - position.y).abs() < 1.
            });
            if !(arrived || passed) {
                break;
            }
            route.next += 1;
        }
        route.points.get(route.next).copied().unwrap_or(target) - position
    }
}
/// Survivor area clearing queries (retail CNavArea potentially-visible sets).
impl crate::bot_look::Areas for Navigation {
    fn area_at(&self, position: Vec3) -> Option<usize> {
        self.area_index(position)
    }
    fn count(&self) -> usize {
        self.graph
            .as_ref()
            .and_then(|g| g.lock().ok().map(|g| g.mesh.areas.len()))
            .unwrap_or(0)
    }
    fn info(&self, area: usize) -> Option<crate::bot_look::AreaInfo> {
        let graph = self.graph.as_ref()?.lock().ok()?;
        let a = graph.mesh.areas.get(area)?;
        let center = [(a.nw[0] + a.se[0]) * 0.5, (a.nw[1] + a.se[1]) * 0.5, (a.nw[2] + a.se[2]) * 0.5];
        Some(crate::bot_look::AreaInfo {
            center: Vec3::from_array(binary::source_position(center)),
            size: (a.se[0] - a.nw[0]) * (a.se[1] - a.nw[1]),
            attributes: a.spawn_attributes,
        })
    }
    fn visible_areas(&self, from: usize, mask: u8) -> Vec<usize> {
        let Some(graph) = self.graph.as_ref().and_then(|g| g.lock().ok()) else {
            return vec![];
        };
        let mesh = &graph.mesh;
        let Some(area) = mesh.areas.get(from) else {
            return vec![];
        };
        // Own set first, then the area it inherits visibility from (by id).
        let inherited = (area.inherit_visibility_from != 0)
            .then(|| mesh.areas.iter().position(|a| a.id == area.inherit_visibility_from))
            .flatten();
        let mut out: Vec<usize> = vec![];
        for source_area in std::iter::once(from).chain(inherited) {
            for v in &mesh.areas[source_area].visible_areas {
                let to = v.index as usize;
                if v.attributes & mask != 0
                    && !out.contains(&to)
                    && !obscured(&mesh.areas[from], mesh.areas.get(to))
                {
                    out.push(to);
                }
            }
        }
        out
    }
    fn visible(&self, from: usize, to: usize, mask: u8) -> bool {
        from == to || self.visible_areas(from, mask).contains(&to)
    }
    fn battlestations(&self, position: Vec3, range: Option<f32>) -> Vec<usize> {
        let Some(start) = self.area_index(position) else {
            return vec![];
        };
        let Some(graph) = self.graph.as_ref().and_then(|g| g.lock().ok()) else {
            return vec![];
        };
        let mesh = &graph.mesh;
        if mesh.areas[start].spawn_attributes & 0x140 == 0 {
            return vec![];
        }
        let index: std::collections::HashMap<u32, usize> =
            mesh.areas.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let origin = source(position);
        let within = |a: &l4d2_formats::nav::NavArea| {
            range.is_none_or(|r| {
                let c = [(a.nw[0] + a.se[0]) * 0.5, (a.nw[1] + a.se[1]) * 0.5, (a.nw[2] + a.se[2]) * 0.5];
                let d2 = (c[0] - origin.x).powi(2) + (c[1] - origin.y).powi(2) + (c[2] - origin.z).powi(2);
                d2 <= (r / 0.0254).powi(2)
            })
        };
        let mut seen = vec![false; mesh.areas.len()];
        let mut open = vec![start];
        seen[start] = true;
        let mut out = vec![];
        while let Some(i) = open.pop() {
            let area = &mesh.areas[i];
            if area.spawn_attributes & 0x20 != 0 {
                out.push(i);
            }
            for direction in &area.connections {
                for id in direction {
                    let Some(&n) = index.get(id) else { continue };
                    let next = &mesh.areas[n];
                    if !seen[n] && next.spawn_attributes & 0x140 != 0 && within(next) {
                        seen[n] = true;
                        open.push(n);
                    }
                }
            }
        }
        out
    }
}
/// An OBSCURED (0x1000) area is not visible from 400 units or more away.
fn obscured(from: &l4d2_formats::nav::NavArea, to: Option<&l4d2_formats::nav::NavArea>) -> bool {
    let Some(to) = to else {
        return true;
    };
    if to.spawn_attributes & 0x1000 == 0 {
        return false;
    }
    let center = |a: &l4d2_formats::nav::NavArea| {
        [(a.nw[0] + a.se[0]) * 0.5, (a.nw[1] + a.se[1]) * 0.5, (a.nw[2] + a.se[2]) * 0.5]
    };
    let (a, b) = (center(from), center(to));
    let d2 = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
    d2 >= 400. * 400.
}
