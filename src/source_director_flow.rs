//! NAV-connected flow distances for script queries. Native incursion costs/AAS
//! are not reproduced: this uses directed NAV/ladder links and centre distances.
use crate::l4d2r_navgraph::NavGraph;
use bevy::prelude::Vec3;
use l4d2_formats::nav::NavMesh;
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    sync::{Arc, Mutex},
};
pub struct Flow {
    graph: Mutex<NavGraph>,
    distances: Vec<f32>,
    /// Shortest-path tree from the start area (escape route parents).
    prev: Vec<usize>,
    /// Directed NAV/ladder links.
    links: Vec<Vec<usize>>,
    /// Flow distance to the goal (exit) area, over reversed links.
    to_goal: Vec<f32>,
}
#[derive(PartialEq)]
struct Step(f32, usize);
impl Eq for Step {}
impl Ord for Step {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .total_cmp(&self.0)
            .then_with(|| self.1.cmp(&other.1))
    }
}
impl PartialOrd for Step {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
fn source(p: Vec3) -> l4d2r_math::Vec3 {
    l4d2r_math::Vec3::new(p.x, -p.z, p.y) / 0.0254
}
impl Flow {
    pub fn new(mesh: NavMesh) -> Self {
        let len = mesh.areas.len();
        Self {
            graph: Mutex::new(NavGraph::new(Arc::new(mesh))),
            distances: vec![f32::INFINITY; len],
            prev: vec![usize::MAX; len],
            links: vec![],
            to_goal: vec![f32::INFINITY; len],
        }
    }
    pub fn start(&mut self, position: Vec3) {
        self.distances.fill(f32::INFINITY);
        let graph = self.graph.lock().unwrap();
        let Some(start) = graph.nearest_area(source(position)) else {
            return;
        };
        let ids = graph
            .mesh
            .areas
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect::<HashMap<_, _>>();
        let mut links = vec![vec![]; self.distances.len()];
        for (i, a) in graph.mesh.areas.iter().enumerate() {
            for id in a.connections.iter().flatten() {
                if let Some(&j) = ids.get(id) {
                    links[i].push(j);
                }
            }
        }
        for ladder in &graph.mesh.ladders {
            if let Some(&bottom) = ids.get(&ladder.bottom_area) {
                for top in [
                    ladder.top_forward,
                    ladder.top_left,
                    ladder.top_right,
                    ladder.top_behind,
                ] {
                    if let Some(&top) = ids.get(&top) {
                        links[bottom].push(top);
                        links[top].push(bottom);
                    }
                }
            }
        }
        self.prev.fill(usize::MAX);
        self.distances[start] = 0.;
        let mut queue = BinaryHeap::from([Step(0., start)]);
        while let Some(Step(distance, i)) = queue.pop() {
            if distance > self.distances[i] {
                continue;
            }
            for &j in &links[i] {
                let cost = distance + graph.area_center(i).distance(graph.area_center(j));
                if cost < self.distances[j] {
                    self.distances[j] = cost;
                    self.prev[j] = i;
                    queue.push(Step(cost, j));
                }
            }
        }
        // Distances to the goal over reversed links (next area toward it).
        let mut reverse = vec![vec![]; links.len()];
        for (i, out) in links.iter().enumerate() {
            for &j in out {
                reverse[j].push(i);
            }
        }
        self.to_goal.fill(f32::INFINITY);
        drop(graph);
        if let Some(goal) = self.exit_area() {
            let graph = self.graph.lock().unwrap();
            self.to_goal[goal] = 0.;
            let mut queue = BinaryHeap::from([Step(0., goal)]);
            while let Some(Step(distance, i)) = queue.pop() {
                if distance > self.to_goal[i] {
                    continue;
                }
                for &j in &reverse[i] {
                    let cost = distance + graph.area_center(i).distance(graph.area_center(j));
                    if cost < self.to_goal[j] {
                        self.to_goal[j] = cost;
                        queue.push(Step(cost, j));
                    }
                }
            }
        }
        self.links = links;
    }
    /// The goal area route_length measures to: the farthest reachable
    /// CHECKPOINT area, else the farthest reachable area.
    fn exit_area(&self) -> Option<usize> {
        let graph = self.graph.lock().unwrap();
        let reachable = |i: &usize| self.distances[*i].is_finite();
        let farthest = |it: &mut dyn Iterator<Item = usize>| {
            it.max_by(|a, b| self.distances[*a].total_cmp(&self.distances[*b]))
        };
        let all = farthest(&mut (0..self.distances.len()).filter(reachable))?;
        let exit = farthest(
            &mut (0..self.distances.len())
                .filter(reachable)
                .filter(|&i| graph.mesh.areas[i].spawn_attributes & 0x800 != 0),
        )
        .filter(|&i| self.distances[i] > self.distances[all] * 0.25);
        Some(exit.unwrap_or(all))
    }
    /// Areas of the escape route, start first (retail walks the parent
    /// chain from the goal in OnFlowDistancesInitialized).
    pub fn escape_route(&self) -> Vec<usize> {
        let Some(mut i) = self.exit_area() else { return vec![] };
        let mut route = vec![i];
        while self.prev[i] != usize::MAX && route.len() <= self.prev.len() {
            i = self.prev[i];
            route.push(i);
        }
        route.reverse();
        route
    }
    /// Source-unit centre of NAV area `i`.
    pub fn area_center(&self, i: usize) -> l4d2r_math::Vec3 {
        self.graph.lock().unwrap().area_center(i)
    }
    /// World position of NAV area `i`'s centre.
    pub fn area_world(&self, i: usize) -> Vec3 {
        let c = self.area_center(i);
        Vec3::from_array(crate::binary::source_position([c.x, c.y, c.z]))
    }
    pub fn area_attributes(&self, i: usize) -> u32 {
        self.graph.lock().unwrap().mesh.areas.get(i).map_or(0, |a| a.spawn_attributes)
    }
    /// Retail threat-area candidate test: not NOTHREAT (0x80000) and at
    /// least 24 units in both directions.
    pub fn threat_candidate(&self, i: usize) -> bool {
        let graph = self.graph.lock().unwrap();
        let Some(a) = graph.mesh.areas.get(i) else { return false };
        a.spawn_attributes & 0x80000 == 0 && a.se[0] - a.nw[0] >= 24. && a.se[1] - a.nw[1] >= 24.
    }
    /// NAV area under `position`.
    pub fn area_of(&self, position: Vec3) -> Option<usize> {
        self.graph.lock().unwrap().find_area(source(position))
    }
    /// Areas within `limit` units of travel from `start` (CollectSurroundingAreas).
    pub fn surrounding(&self, start: usize, limit: f32) -> Vec<usize> {
        let graph = self.graph.lock().unwrap();
        let mut best = HashMap::from([(start, 0f32)]);
        let mut queue = BinaryHeap::from([Step(0., start)]);
        let mut found = vec![];
        while let Some(Step(distance, i)) = queue.pop() {
            if best.get(&i).is_some_and(|d| distance > *d) {
                continue;
            }
            found.push(i);
            for &j in self.links.get(i).into_iter().flatten() {
                let cost = distance + graph.area_center(i).distance(graph.area_center(j));
                if cost <= limit && best.get(&j).is_none_or(|d| cost < *d) {
                    best.insert(j, cost);
                    queue.push(Step(cost, j));
                }
            }
        }
        found
    }
    /// The neighbour of `i` one step closer to the goal area.
    pub fn toward_goal(&self, i: usize) -> Option<usize> {
        let here = *self.to_goal.get(i)?;
        self.links.get(i)?.iter().copied()
            .filter(|&j| self.to_goal[j] < here)
            .min_by(|a, b| self.to_goal[*a].total_cmp(&self.to_goal[*b]))
    }
    pub fn is_goal(&self, i: usize) -> bool {
        self.to_goal.get(i) == Some(&0.)
    }
    /// The escape route length that retail OnFlowDistancesInitialized
    /// (1035f370) sizes the wanderers by: the flow distance to the farthest
    /// CHECKPOINT area (the exit saferoom), or to the farthest reachable area
    /// on maps without one (finales).
    pub fn route_length(&self) -> Option<f32> {
        let graph = self.graph.lock().unwrap();
        let reachable = |i: &usize| self.distances[*i].is_finite();
        let farthest = |it: &mut dyn Iterator<Item = usize>| {
            it.map(|i| self.distances[i]).max_by(f32::total_cmp)
        };
        let all = farthest(&mut (0..self.distances.len()).filter(reachable))?;
        let exit = farthest(
            &mut (0..self.distances.len())
                .filter(reachable)
                .filter(|&i| graph.mesh.areas[i].spawn_attributes & 0x800 != 0),
        )
        .filter(|d| *d > all * 0.25);
        Some(exit.unwrap_or(all))
    }
    /// Flow at an object's NAV area: the area under it, else the nearest
    /// within `max` Source units (an item on a table).
    pub fn at_near(&self, position: Vec3, max: f32) -> Option<f32> {
        let graph = self.graph.lock().unwrap();
        let i = graph.nearest_area_within(source(position), max)?;
        let d = self.distances[i];
        d.is_finite().then_some(d)
    }
    pub fn at(&self, position: Vec3) -> Option<f32> {
        let graph = self.graph.lock().unwrap();
        let i = graph.find_area(source(position))?;
        let d = self.distances[i];
        d.is_finite().then_some(d)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_lab_flow_follows_connected_areas_and_is_stable_while_standing_still() {
        let bytes = std::fs::read(crate::test_lab::path().with_extension("nav")).unwrap();
        let mesh = l4d2_formats::nav::parse_nav(&bytes).unwrap();
        let origin = Vec3::from_array(crate::binary::source_position(mesh.areas[0].center()));
        let mut flow = Flow::new(mesh);
        flow.start(origin);
        assert_eq!(flow.at(origin), Some(0.));
        let graph = flow.graph.lock().unwrap();
        assert!(flow.distances.iter().any(|v| v.is_finite() && *v > 500.));
        let unreachable = flow.distances.iter().filter(|v| !v.is_finite()).count();
        assert!(unreachable < graph.mesh.areas.len());
        drop(graph);
        assert_eq!(flow.at(origin), Some(0.));
    }
}
