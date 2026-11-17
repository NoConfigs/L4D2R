// Vendored from the user-supplied snapshot; see L4D2R_INTEGRATION.md.
#![allow(dead_code)]
//! Queries over the parsed `.nav` mesh: area lookup under a point and A* paths between areas.
//! UNVERIFIED: path cost is plain Euclidean distance between area centres (the game's `ShortestPathCost` adds
//! jump/crouch/danger penalties); waypoints are the shared-edge midpoints. Ladder links (`CNavArea::m_ladder[UP/DOWN]`:
//! bottom area <-> the four top areas) are routed as edges and reported as `PathPoint::ladder_top`.

use l4d2_formats::nav::{NavArea, NavMesh};
use l4d2r_math::Vec3;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

const CELL: f32 = 256.0;

pub struct NavGraph {
    pub mesh: std::sync::Arc<NavMesh>,
    index_of: HashMap<u32, usize>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    ladder_index: HashMap<u32, usize>,
    /// Areas the pathfinder must avoid (areas an agent got stuck in).
    blocked: std::cell::RefCell<HashMap<usize, u8>>,
    pub query_team: std::cell::Cell<u8>,
    /// Deepest drop (Source units) a query may take; AI specials cap it so a
    /// retreat cannot route them off a roof (commons keep dropping freely).
    pub max_drop: std::cell::Cell<f32>,
    /// NAV ladder index -> the query teams (1 survivors, 2 infected) that may
    /// climb it, from the team of the func_simpleladder it belongs to.
    ladder_teams: std::cell::RefCell<HashMap<usize, u8>>,
    pub rescue_vehicle_unblocked: std::cell::Cell<bool>,
    /// Connections an agent could not physically cross (fences, props): (from, to) area indices.
    blocked_edges: std::cell::RefCell<std::collections::HashSet<(usize, usize)>>,
    /// Route through `func_nav_blocker` areas anyway (agents that open doors, e.g. the test autopilot).
    pub ignore_blockers: std::cell::Cell<bool>,
    /// Areas blocked by an agent getting stuck (kept even when ignoring map blockers).
    stuck_blocked: std::cell::RefCell<std::collections::HashSet<usize>>,
}

/// A waypoint; `ladder_top` is set on the *bottom* point of a ladder edge (climb up to it) .
#[derive(Clone, Copy, Debug)]
pub struct PathPoint {
    pub pos: Vec3,
    pub ladder_top: Option<Vec3>,
    /// Source-space horizontal normal of the ladder's climbable face
    /// (`NavLadder::dir`: 0 north/-y, 1 east/+x, 2 south/+y, 3 west/-x).
    pub ladder_normal: Vec3,
}

#[derive(PartialEq)]
struct Node {
    f: f32,
    i: usize,
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

fn center(a: &NavArea) -> Vec3 {
    Vec3::from(a.center())
}

impl NavGraph {
    pub fn new(mesh: std::sync::Arc<NavMesh>) -> NavGraph {
        let index_of = mesh
            .areas
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, a) in mesh.areas.iter().enumerate() {
            let (lo, hi) = (a.min_xy(), a.max_xy());
            for x in (lo[0] / CELL).floor() as i32..=(hi[0] / CELL).floor() as i32 {
                for y in (lo[1] / CELL).floor() as i32..=(hi[1] / CELL).floor() as i32 {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        let ladder_index = mesh
            .ladders
            .iter()
            .enumerate()
            .map(|(i, l)| (l.id, i))
            .collect();
        NavGraph {
            mesh,
            index_of,
            grid,
            ladder_index,
            blocked: Default::default(),
            rescue_vehicle_unblocked: Default::default(),
            query_team: std::cell::Cell::new(1),
            max_drop: std::cell::Cell::new(f32::INFINITY),
            ladder_teams: Default::default(),
            blocked_edges: Default::default(),
            ignore_blockers: Default::default(),
            stuck_blocked: Default::default(),
        }
    }

    /// Area whose rectangle contains `p` in XY and whose surface is closest below (or slightly above) `p`.
    pub fn find_area(&self, p: Vec3) -> Option<usize> {
        let cell = ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32);
        let mut best: Option<(f32, usize)> = None;
        for &i in self.grid.get(&cell)? {
            let a = &self.mesh.areas[i];
            if !a.contains_xy(p.x, p.y) {
                continue;
            }
            let dz = p.z - a.z_at(p.x, p.y);
            if dz < -64.0 {
                continue; // area is well above us
            }
            let score = if dz >= 0.0 { dz } else { -dz * 4.0 };
            if best.map(|b| score < b.0).unwrap_or(true) {
                best = Some((score, i));
            }
        }
        best.map(|b| b.1)
    }

    /// Retail GetNearestNavArea with a maximum distance: the area under
    /// `p`, else the nearest area whose centre is within `max` units.
    pub fn nearest_area_within(&self, p: Vec3, max: f32) -> Option<usize> {
        self.find_area(p).or_else(|| {
            self.nearest_area(p)
                .filter(|&a| center(&self.mesh.areas[a]).distance(p) <= max)
        })
    }
    /// Nearest area by centre distance (fallback when `p` is off-mesh).
    pub fn nearest_area(&self, p: Vec3) -> Option<usize> {
        self.find_area(p).or_else(|| {
            self.mesh
                .areas
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    center(a.1)
                        .distance_squared(p)
                        .partial_cmp(&center(b.1).distance_squared(p))
                        .unwrap()
                })
                .map(|a| a.0)
        })
    }

    pub fn area_center(&self, i: usize) -> Vec3 {
        center(&self.mesh.areas[i])
    }

    /// Midpoint of the edge shared by two connected areas (falls back to the segment between centres).
    fn portal(&self, a: usize, b: usize) -> Vec3 {
        let (aa, bb) = (&self.mesh.areas[a], &self.mesh.areas[b]);
        let (alo, ahi, blo, bhi) = (aa.min_xy(), aa.max_xy(), bb.min_xy(), bb.max_xy());
        let x0 = alo[0].max(blo[0]);
        let x1 = ahi[0].min(bhi[0]);
        let y0 = alo[1].max(blo[1]);
        let y1 = ahi[1].min(bhi[1]);
        let (x, y) = if x0 <= x1 && y0 <= y1 {
            ((x0 + x1) * 0.5, (y0 + y1) * 0.5)
        } else {
            let c = (center(aa) + center(bb)) * 0.5;
            (c.x, c.y)
        };
        Vec3::new(x, y, (aa.z_at(x, y) + bb.z_at(x, y)) * 0.5)
    }

    /// Ladder edges out of area `i`: (destination area, ladder index). Up ladders lead to the top areas, down ladders to the bottom area.
    /// Restrict the NAV ladders over each team-only func_simpleladder
    /// (`team` 1 survivors, 2 infected; Source bounds). A NAV ladder belongs to
    /// a brush when its climb segment lies within the brush, widened by
    /// `margin` horizontally (NAV ladders sit just in front of the brush face).
    pub fn set_ladder_teams(&self, ladders: &[(Vec3, Vec3, i32)], margin: f32) {
        let mut teams = self.ladder_teams.borrow_mut();
        teams.clear();
        for (li, l) in self.mesh.ladders.iter().enumerate() {
            let (top, bottom) = (Vec3::from(l.top), Vec3::from(l.bottom));
            let mid = (top + bottom) * 0.5;
            for &(lo, hi, team) in ladders {
                let mask = match team {
                    1 => 1,
                    2 => 2,
                    _ => continue,
                };
                let inside = |p: Vec3| {
                    p.x >= lo.x - margin
                        && p.x <= hi.x + margin
                        && p.y >= lo.y - margin
                        && p.y <= hi.y + margin
                };
                let overlap = top.z.max(bottom.z) >= lo.z - margin && top.z.min(bottom.z) <= hi.z + margin;
                if inside(mid) && inside(top) && inside(bottom) && overlap {
                    teams.insert(li, mask);
                }
            }
        }
    }
    /// Whether the current query team may climb NAV ladder `li`.
    pub fn ladder_usable(&self, li: usize) -> bool {
        self.ladder_teams.borrow().get(&li).is_none_or(|mask| mask & self.query_team.get() != 0)
    }
    fn ladder_edges(&self, i: usize) -> Vec<(usize, usize)> {
        let a = &self.mesh.areas[i];
        let mut out = Vec::new();
        for lid in &a.ladders[0] {
            let Some(&li) = self.ladder_index.get(lid) else {
                continue;
            };
            let l = &self.mesh.ladders[li];
            for id in [l.top_forward, l.top_left, l.top_right, l.top_behind] {
                if let Some(&j) = self.index_of.get(&id) {
                    out.push((j, li));
                }
            }
        }
        for lid in &a.ladders[1] {
            let Some(&li) = self.ladder_index.get(lid) else {
                continue;
            };
            if let Some(&j) = self.index_of.get(&self.mesh.ladders[li].bottom_area) {
                out.push((j, li));
            }
        }
        out
    }

    /// A* over area connections and ladders; returns (area, ladder used to arrive) per step, inclusive, or `None` if unreachable.
    pub fn route_edges(&self, from: usize, to: usize) -> Option<Vec<(usize, Option<usize>)>> {
        if from == to {
            return Some(vec![(from, None)]);
        }
        let goal = center(&self.mesh.areas[to]);
        let mut open = BinaryHeap::new();
        let mut g: HashMap<usize, f32> = HashMap::new();
        let mut parent: HashMap<usize, (usize, Option<usize>)> = HashMap::new();
        g.insert(from, 0.0);
        open.push(Node {
            f: center(&self.mesh.areas[from]).distance(goal),
            i: from,
        });
        while let Some(Node { i, .. }) = open.pop() {
            if i == to {
                let mut chain = vec![(i, None)];
                let mut c = i;
                while let Some(&(p, l)) = parent.get(&c) {
                    chain.last_mut().unwrap().1 = l;
                    chain.push((p, None));
                    c = p;
                }
                chain.reverse();
                return Some(chain);
            }
            let gi = g[&i];
            let ci = center(&self.mesh.areas[i]);
            let mut edges: Vec<(usize, Option<usize>, f32)> = Vec::new();
            for dir in &self.mesh.areas[i].connections {
                for id in dir {
                    if let Some(&j) = self.index_of.get(id) {
                        let cj = center(&self.mesh.areas[j]);
                        // Prefer walkable routes: a climb above the step height is only taken when nothing else exists (UNVERIFIED cost).
                        // Survivors avoid lethal drops; infected (query team 2)
                        // take no fall damage in the rewrite and drop freely.
                        let climb = if cj.z - ci.z > 34.0 {
                            600.0
                        } else if ci.z - cj.z > 160.0 && self.query_team.get() != 2 {
                            4000.0
                        } else {
                            0.0
                        }; // lethal drops: last resort
                        if ci.z - cj.z > self.max_drop.get() {
                            continue;
                        }
                        edges.push((j, None, ci.distance(cj) + climb));
                    }
                }
            }
            for (j, li) in self.ladder_edges(i).into_iter().filter(|&(_, li)| self.ladder_usable(li)) {
                edges.push((j, Some(li), self.mesh.ladders[li].length + 30.0));
            }
            let blocked = self.blocked.borrow();
            for (j, via, cost) in edges {
                if (!self.rescue_vehicle_unblocked.get() && self.mesh.areas[j].spawn_attributes & 0x8000 != 0)
                    || (!self.ignore_blockers.get()
                    && blocked
                        .get(&j)
                        .is_some_and(|mask| mask & self.query_team.get() != 0))
                    || (self.stuck_blocked.borrow().contains(&j) && j != to)
                    || self.blocked_edges.borrow().contains(&(i, j))
                {
                    continue;
                }
                let cj = center(&self.mesh.areas[j]);
                let ng = gi + cost;
                if g.get(&j).map(|&old| ng < old).unwrap_or(true) {
                    g.insert(j, ng);
                    parent.insert(j, (i, via));
                    open.push(Node {
                        f: ng + cj.distance(goal),
                        i: j,
                    });
                }
            }
        }
        None
    }

    /// A* over area connections; returns the area index chain (inclusive) or `None` if unreachable.
    pub fn route(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        self.route_edges(from, to)
            .map(|c| c.into_iter().map(|(a, _)| a).collect())
    }

    /// Block or unblock every area whose centre lies inside the box (`func_nav_blocker`).
    pub fn clear_area_blocks(&self) {
        self.blocked.borrow_mut().clear();
    }

    pub fn set_blocked_in(&self, lo: Vec3, hi: Vec3, block: bool) {
        self.set_blocked_in_team(lo, hi, block, 3);
    }
    pub fn set_blocked_in_team(&self, lo: Vec3, hi: Vec3, block: bool, team_mask: u8) {
        let mut set = self.blocked.borrow_mut();
        for (i, a) in self.mesh.areas.iter().enumerate() {
            let c = center(a);
            if c.cmpge(lo).all() && c.cmple(hi).all() {
                if block {
                    *set.entry(i).or_default() |= team_mask;
                } else {
                    if let Some(mask) = set.get_mut(&i) {
                        *mask &= !team_mask;
                    }
                }
            }
        }
    }

    /// Forbid the first connection of the current route from `start` toward `goal` (the one an agent is stuck on).
    pub fn block_first_edge(&self, start: Vec3, goal: Vec3) {
        let (Some(a), Some(b)) = (self.nearest_area(start), self.nearest_area(goal)) else {
            return;
        };
        if let Some(chain) = self.route_edges(a, b) {
            if chain.len() >= 2 {
                let mut e = self.blocked_edges.borrow_mut();
                e.insert((chain[0].0, chain[1].0));
                e.insert((chain[1].0, chain[0].0));
            }
        }
    }

    /// Forbid the connection from the area under `from` to the area under `to` (both directions).
    pub fn block_edge_between(&self, from: Vec3, to: Vec3) {
        if let (Some(a), Some(b)) = (self.nearest_area(from), self.nearest_area(to)) {
            let mut e = self.blocked_edges.borrow_mut();
            e.insert((a, b));
            e.insert((b, a));
        }
    }

    /// Mark the area at `p` as impassable for later path queries (an agent could not get through it).
    pub fn block_area_at(&self, p: Vec3) {
        if let Some(i) = self.nearest_area(p) {
            self.stuck_blocked.borrow_mut().insert(i);
        }
    }

    /// Waypoints including ladder climbs: for a ladder edge the point at the ladder bottom (with `ladder_top`), then the
    /// arrival area's centre.
    /// The `k` areas closest to `p` (point-to-rectangle distance in XY, plus a height penalty).
    pub fn nearest_areas(&self, p: Vec3, k: usize) -> Vec<usize> {
        let mut v: Vec<(f32, usize)> = self
            .mesh
            .areas
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (lo, hi) = (a.min_xy(), a.max_xy());
                let dx = (lo[0] - p.x).max(0.0).max(p.x - hi[0]);
                let dy = (lo[1] - p.y).max(0.0).max(p.y - hi[1]);
                let dz = (a.z_at(p.x.clamp(lo[0], hi[0]), p.y.clamp(lo[1], hi[1])) - p.z).abs();
                ((dx * dx + dy * dy).sqrt() + dz * 0.5, i)
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().take(k).map(|x| x.1).collect()
    }

    pub fn path_points(&self, start: Vec3, goal: Vec3) -> Option<Vec<PathPoint>> {
        let b = self.nearest_area(goal)?;
        // Off-mesh starts (agent shoved aside): try the closest few areas until one connects to the goal.
        // On the mesh, only the containing area is used (as `path`), so
        // blocked routes stay blocked.
        let starts = match self.find_area(start) {
            Some(a) => vec![a],
            None => self.nearest_areas(start, 12),
        };
        let (a, chain) = starts
            .into_iter()
            .find_map(|a| self.route_edges(a, b).map(|c| (a, c)))?;
        let _ = a;
        let mut pts = Vec::new();
        for k in 1..chain.len() {
            let (prev, (cur, via)) = (chain[k - 1].0, chain[k]);
            match via {
                Some(li) => {
                    let l = &self.mesh.ladders[li];
                    let (bot, top) = (Vec3::from(l.bottom), Vec3::from(l.top));
                    // Up ladder if we arrive at an area near the top.
                    let up = center(&self.mesh.areas[cur]).z > center(&self.mesh.areas[prev]).z;
                    let (from_p, to_p) = if up { (bot, top) } else { (top, bot) };
                    let ladder_normal = match l.dir {
                        0 => Vec3::new(0., -1., 0.),
                        1 => Vec3::new(1., 0., 0.),
                        2 => Vec3::new(0., 1., 0.),
                        _ => Vec3::new(-1., 0., 0.),
                    };
                    pts.push(PathPoint {
                        pos: from_p,
                        ladder_top: Some(to_p),
                        ladder_normal,
                    });
                    pts.push(PathPoint {
                        pos: center(&self.mesh.areas[cur]),
                        ladder_top: None,
                        ladder_normal: Vec3::ZERO,
                    });
                }
                None => pts.push(PathPoint {
                    pos: self.portal(prev, cur),
                    ladder_top: None,
                    ladder_normal: Vec3::ZERO,
                }),
            }
        }
        pts.push(PathPoint {
            pos: goal,
            ladder_top: None,
            ladder_normal: Vec3::ZERO,
        });
        Some(pts)
    }

    /// Run `query` with some connections forbidden for it only (one agent's
    /// stuck edges; other agents keep using them).
    fn avoiding<T>(&self, avoid: &[(usize, usize)], query: impl FnOnce() -> T) -> T {
        let added: Vec<(usize, usize)> = {
            let mut e = self.blocked_edges.borrow_mut();
            avoid
                .iter()
                .flat_map(|&(a, b)| [(a, b), (b, a)])
                .filter(|edge| e.insert(*edge))
                .collect()
        };
        let result = query();
        let mut e = self.blocked_edges.borrow_mut();
        for edge in added {
            e.remove(&edge);
        }
        result
    }

    /// The first area connection of the route from `start` toward `goal`.
    pub fn first_edge_avoiding(
        &self,
        start: Vec3,
        goal: Vec3,
        avoid: &[(usize, usize)],
    ) -> Option<(usize, usize)> {
        let (a, b) = (self.nearest_area(start)?, self.nearest_area(goal)?);
        let chain = self.avoiding(avoid, || self.route_edges(a, b))?;
        (chain.len() >= 2).then(|| (chain[0].0, chain[1].0))
    }

    pub fn path_avoiding(&self, start: Vec3, goal: Vec3, avoid: &[(usize, usize)]) -> Option<Vec<Vec3>> {
        self.avoiding(avoid, || self.path(start, goal))
    }

    pub fn path_points_avoiding(
        &self,
        start: Vec3,
        goal: Vec3,
        avoid: &[(usize, usize)],
    ) -> Option<Vec<PathPoint>> {
        self.avoiding(avoid, || self.path_points(start, goal))
    }

    /// Waypoints from `start` to `goal`: portal midpoints along the area route, then the goal.
    pub fn path(&self, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        let (a, b) = (self.nearest_area(start)?, self.nearest_area(goal)?);
        let chain = self.route(a, b)?;
        let mut pts: Vec<Vec3> = chain.windows(2).map(|w| self.portal(w[0], w[1])).collect();
        pts.push(goal);
        Some(pts)
    }
}
