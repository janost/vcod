//! The bots' navigation graph: a grid flood-filled out of the map's spawn
//! points, each edge proven by running pmove along it, and A* over it.
//! Design and build times: `docs/research/bot-navigation.md`.

use crate::game::trigger::{BrushHull, box_contacts_hulls};
use crate::server::FRAME_MS;
use glam::Vec3;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use vcod_common::collision::{CollisionWorld, MASK_PLAYERSOLID, Prim};
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::pmove::PlayerState;
use vcod_common::pmove::cmd::{ANGLE2SHORT, EventRing, chop, player_step};

/// Grid pitch bounds in world units. The finest is about a capsule width
/// plus the slack a doorway leaves; a large map coarsens toward the other
/// to keep its node count near [`TARGET_CELLS`] (bot-navigation.md,
/// "Spacing"). At 64 mp_hurtgen's Retrieval bunker, the objective's room,
/// came out as islands its stairs never joined.
const SPACING_MIN: f32 = 32.0;
const SPACING_MAX: f32 = 48.0;
/// The cells the spawn points' bounding box is cut into.
const TARGET_CELLS: f32 = 20_000.0;
/// How far past the spawn points' bounding box the flood goes. Stock maps
/// put spawns at the edges of play; past this lies the scenery behind the
/// boundary (mp_hurtgen's forest is ~2000 units of it).
const MARGIN: f32 = 512.0;
/// Two nodes in one grid column are one node when their floors are closer
/// than this; a full storey is ~128.
const Z_MERGE: f32 = 40.0;
/// A walk counts as arrived this close to its target, horizontally. One 50
/// ms tick at run speed covers 9.5 units, so the walk cannot step over it.
/// A walk bound for a node's floor also ends this close to its height
/// (bot-navigation.md, "The flood": mp_ship's hull beam).
const ARRIVE: f32 = 8.0;
/// A walk that drops further than this is refused: past
/// `bg_fallDamageMinHeight` (256) the drop hurts.
const MAX_DROP: f32 = 200.0;
/// An edge whose ends differ by less than this in height is level, and a
/// walk one way stands in for the walk back.
const LEVEL: f32 = 1.0;
/// The flood stops here; a stock map fills well under it.
const MAX_NODES: usize = 60_000;
/// The longest a walk runs with a ladder in it: 15 s, ~800 units of climb
/// at the 53 units/s a full-rate climb makes. mp_ship's hold ladder is 560.
const LADDER_TICKS: usize = 300;
/// Degrees up a climbing body looks; past ~9 deg `ladder_move`'s upscale
/// saturates. Shared with the bots, which climb the same way.
pub const LADDER_PITCH: f32 = 45.0;
/// A* gives up past this many expansions and reports no path.
const MAX_EXPANSIONS: usize = 40_000;

const CARDINALS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
const DIAGONALS: [(i32, i32); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];

pub struct NavGraph {
    /// Feet origins where pmove came to rest.
    pub nodes: Vec<Vec3>,
    /// Directed: `edges[a]` holds every `b` a run from `a` reaches. A drop
    /// off a ledge has no edge back.
    pub edges: Vec<Vec<u32>>,
    /// The edges a walk proved only with a jump; a bot following one jumps
    /// the way the walk did ([`jump_cue`]).
    pub jumps: HashSet<(u32, u32)>,
    /// The jump edges proved from a standstill at their foot, jumping at the
    /// lip ([`NavGraph::link_leaps`]): a bot comes to rest at the foot first.
    pub leaps: HashSet<(u32, u32)>,
    /// The grid pitch the graph was built on.
    pub spacing: f32,
    /// Nodes by grid column, for lookups only; nothing iterates it.
    columns: HashMap<(i32, i32), Vec<u32>>,
    /// Each node's strongly connected component ([`Self::components`]), and
    /// each component's nodes in index order: a roam picks its destination
    /// among the nodes its start reaches both ways.
    comp: Vec<u32>,
    members: Vec<Vec<u32>>,
    /// Per component, a bitset of the components it reaches (itself
    /// included): an unreachable goal fails before any search. A stock map
    /// has 1 to 13 components.
    reach: Vec<Vec<u64>>,
    /// The world's ladders ([`ladders`]), for the build.
    ladder_boxes: Vec<(Vec3, Vec3)>,
}

/// The spawn points' horizontal bounding box.
fn seed_bounds(seeds: &[[f32; 3]]) -> (glam::Vec2, glam::Vec2) {
    seeds.iter().fold(
        (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)),
        |(lo, hi), s| {
            let p = glam::Vec2::new(s[0], s[1]);
            (lo.min(p), hi.max(p))
        },
    )
}

impl NavGraph {
    /// Floods the grid from `seeds` (feet origins, typically every spawn
    /// point of the map) and keeps an edge only where a standing run, one
    /// usercmd per 50 ms through the shared cmd step, arrives. No node
    /// stands where a body would touch one of `hazards`.
    pub fn build(world: &CollisionWorld, seeds: &[[f32; 3]], hazards: &[BrushHull]) -> NavGraph {
        // The pitch cuts the spawns' box into about TARGET_CELLS, rounded up
        // to a multiple of 16; the flood stays within MARGIN of the box.
        let (lo, hi) = seed_bounds(seeds);
        let area = (hi - lo).max(glam::Vec2::ZERO).element_product();
        let spacing = ((area / TARGET_CELLS).sqrt() / 16.0).ceil() * 16.0;
        let (lo, hi) = (
            lo - glam::Vec2::splat(MARGIN),
            hi + glam::Vec2::splat(MARGIN),
        );
        let inside = |p: Vec3| {
            let p = p.truncate();
            p.cmpge(lo).all() && p.cmple(hi).all()
        };
        let mut g = NavGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            jumps: HashSet::new(),
            leaps: HashSet::new(),
            spacing: spacing.clamp(SPACING_MIN, SPACING_MAX),
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            ladder_boxes: ladders(world),
        };
        let mut queue = Vec::new();
        for seed in seeds {
            let Some(p) = settle(world, Vec3::from(*seed), 0.0) else {
                continue;
            };
            // On the lattice when the seed can walk to its own column's
            // centre, so every later walk targets a node's real spot.
            let c = g.column(p);
            let p = walk(world, &g.ladder_boxes, p, g.center(c)).map_or(p, |(p, _)| p);
            if g.find(c, p.z).is_none() {
                queue.push(g.add(c, p));
            }
        }
        g.flood(world, hazards, &inside, queue);
        let ends = g.link_ladders(world, hazards);
        g.flood(world, hazards, &inside, ends);
        g.link_back(world);
        g.link_leaps(world);
        g.index_components();
        g
    }

    /// Floods the grid out of `frontier`: the spawn points' nodes, then the
    /// ladders' ends, whose floors a walk between neighbouring columns may
    /// never reach. New nodes stay `inside` and out of `hazards`.
    fn flood(
        &mut self,
        world: &CollisionWorld,
        hazards: &[BrushHull],
        inside: &dyn Fn(Vec3) -> bool,
        mut frontier: Vec<u32>,
    ) {
        // Breadth-first by layers. A layer's walks depend only on where its
        // nodes stand, so they run on every core, and the merge takes them in
        // job order, which keeps the graph identical to a serial flood
        // whatever the thread count. A layer's diagonals run two layers late,
        // once the square's other three corners have walked their sides.
        let mut late: VecDeque<Vec<u32>> = VecDeque::new();
        loop {
            let diagonal = if late.len() >= 2 || frontier.is_empty() {
                late.pop_front().unwrap_or_default()
            } else {
                Vec::new()
            };
            if frontier.is_empty() && diagonal.is_empty() {
                break;
            }
            let jobs: Vec<(u32, &[(i32, i32); 4])> = frontier
                .iter()
                .map(|&n| (n, &CARDINALS))
                .chain(diagonal.iter().map(|&n| (n, &DIAGONALS)))
                .collect();
            let ends = par_map(&jobs, |&(n, dirs)| {
                let (cx, cy) = self.column(self.nodes[n as usize]);
                dirs.map(|(dx, dy)| self.step(world, n, (cx + dx, cy + dy)))
            });
            let mut next = Vec::new();
            for (&(n, dirs), ends) in jobs.iter().zip(ends) {
                let (cx, cy) = self.column(self.nodes[n as usize]);
                for (&(dx, dy), to) in dirs.iter().zip(ends) {
                    let Some((to, jumped)) = to else {
                        continue;
                    };
                    let c = (cx + dx, cy + dy);
                    let m = match self.find(c, to.z) {
                        Some(m) => m,
                        None if self.nodes.len() < MAX_NODES
                            && inside(to)
                            && !hazard(hazards, to) =>
                        {
                            let m = self.add(c, to);
                            next.push(m);
                            m
                        }
                        None => continue,
                    };
                    self.link_as(n, m, jumped);
                }
            }
            if !frontier.is_empty() {
                late.push_back(frontier);
            }
            frontier = next;
        }
    }

    /// The spawn points in the strongly connected component holding the most
    /// of them: the census bot-navigation.md's tables report. Each spawn is
    /// looked up from where a body dropped at it lands, where a bot spawned
    /// there stands; from the spawn's own origin, often 100 units up, the
    /// nearest node can be one on a crate beside it.
    pub fn spawns_in_one_component(&self, world: &CollisionWorld, spawns: &[[f32; 3]]) -> usize {
        let mut count = BTreeMap::new();
        for s in spawns {
            let feet = settle(world, Vec3::from(*s), 0.0).map_or(*s, Into::into);
            if let Some(n) = self.nearest(feet) {
                *count.entry(self.comp[n as usize]).or_insert(0) += 1;
            }
        }
        count.into_values().max().unwrap_or(0)
    }

    fn index_components(&mut self) {
        self.comp = self.components();
        let count = self.comp.iter().max().map_or(0, |&c| c as usize + 1);
        self.members = vec![Vec::new(); count];
        for (n, &c) in self.comp.iter().enumerate() {
            self.members[c as usize].push(n as u32);
        }
        // The condensation's edges, then a walk of them from each component.
        let mut out = vec![HashSet::new(); count];
        for (a, next) in self.edges.iter().enumerate() {
            let ca = self.comp[a];
            for &b in next {
                let cb = self.comp[b as usize];
                if cb != ca {
                    out[ca as usize].insert(cb);
                }
            }
        }
        let words = count.div_ceil(64);
        self.reach = (0..count)
            .map(|root| {
                let mut bits = vec![0u64; words];
                let mut stack = vec![root as u32];
                bits[root / 64] |= 1 << (root % 64);
                while let Some(c) = stack.pop() {
                    for &d in &out[c as usize] {
                        let (w, b) = (d as usize / 64, d % 64);
                        if bits[w] & (1 << b) == 0 {
                            bits[w] |= 1 << b;
                            stack.push(d);
                        }
                    }
                }
                bits
            })
            .collect();
    }

    /// Whether some path leads from node `a` to node `b`.
    pub fn reaches(&self, a: u32, b: u32) -> bool {
        let (Some(&ca), Some(&cb)) = (self.comp.get(a as usize), self.comp.get(b as usize)) else {
            return false;
        };
        self.reach[ca as usize][cb as usize / 64] & (1 << (cb % 64)) != 0
    }

    /// The node `from` reaches that lies nearest `to`, `to` itself when it
    /// is reached; `None` when that is `from` and `from` is not `to`.
    fn nearest_reached(&self, from: u32, to: u32) -> Option<u32> {
        if self.reaches(from, to) {
            return Some(to);
        }
        let goal = self.nodes[to as usize];
        let ca = self.comp[from as usize] as usize;
        let best = self
            .members
            .iter()
            .enumerate()
            .filter(|(c, _)| self.reach[ca][c / 64] & (1 << (c % 64)) != 0)
            .flat_map(|(_, m)| m)
            .min_by(|a, b| {
                let d = |n: u32| self.nodes[n as usize].distance_squared(goal);
                d(**a).total_cmp(&d(**b)).then(a.cmp(b))
            })?;
        (*best != from).then_some(*best)
    }

    /// The flood walks to a neighbouring column only, so a ledge across a
    /// gap is never tried: mp_depot's Retrieval documents lie on a crate top
    /// 32 units over a step, a 32-unit gap between them. Every node up to two
    /// columns off and between a step and a jump higher, that three edges
    /// don't already reach, is run at from a standstill with a jump at the
    /// lip ([`Gait::Leap`]).
    fn link_leaps(&mut self, world: &CollisionWorld) {
        use vcod_common::pmove::{JUMP_HEIGHT, STEPSIZE};
        let mut jobs = Vec::new();
        for (a, &p) in self.nodes.iter().enumerate() {
            let (cx, cy) = self.column(p);
            let mut near: Option<HashSet<u32>> = None;
            for dx in -2..=2i32 {
                for dy in -2..=2i32 {
                    if dx.abs().max(dy.abs()) < 2 {
                        continue;
                    }
                    for &m in self.columns.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                        let rise = self.nodes[m as usize].z - p.z;
                        if rise > STEPSIZE
                            && rise <= JUMP_HEIGHT
                            && !near
                                .get_or_insert_with(|| self.within(a as u32, 3))
                                .contains(&m)
                        {
                            jobs.push((a as u32, m));
                        }
                    }
                }
            }
        }
        let walked = par_map(&jobs, |&(a, b)| {
            let to = self.nodes[b as usize];
            let from = self.nodes[a as usize];
            matches!(
                walk_as(world, from, to.truncate(), Some(to.z), Gait::Leap),
                Walked::Arrived(..)
            )
        });
        for (&(a, b), ok) in jobs.iter().zip(walked) {
            if ok && !self.edges[a as usize].contains(&b) {
                self.link_as(a, b, true);
                self.leaps.insert((a, b));
            }
        }
    }

    /// The nodes `hops` edges or fewer from `a`.
    fn within(&self, a: u32, hops: usize) -> HashSet<u32> {
        let mut seen = HashSet::from([a]);
        let mut layer = vec![a];
        for _ in 0..hops {
            let mut next = Vec::new();
            for n in layer {
                for &m in &self.edges[n as usize] {
                    if seen.insert(m) {
                        next.push(m);
                    }
                }
            }
            layer = next;
        }
        seen
    }

    /// The flood walks toward a column's centre, so a node that stands off
    /// its centre (a spawn point against a wall, which never reached it) is
    /// walked out of but never into: its neighbours aim at the centre and
    /// meet the wall. Every one-way edge between nodes on one floor is
    /// walked back node to node.
    fn link_back(&mut self, world: &CollisionWorld) {
        let mut jobs = Vec::new();
        for (a, out) in self.edges.iter().enumerate() {
            let p = self.nodes[a];
            for &b in out {
                let q = self.nodes[b as usize];
                if (q.z - p.z).abs() < Z_MERGE && !self.edges[b as usize].contains(&(a as u32)) {
                    jobs.push((b, a as u32));
                }
            }
        }
        self.walk_jobs(world, &jobs, true);
    }

    /// The flood walks only to a neighbouring column, which rarely lines up
    /// with a ladder. So each ladder face gets two nodes of its own: a foot
    /// in front of the face and a head where a climb up it comes to rest,
    /// linked where the bots' own steering proves the way, and each linked
    /// to the graph around it (bot-navigation.md, "Ladders").
    fn link_ladders(&mut self, world: &CollisionWorld, hazards: &[BrushHull]) -> Vec<u32> {
        let faces: Vec<(Vec3, Vec3, Vec3)> = self
            .ladder_boxes
            .iter()
            .copied()
            .flat_map(|(lo, hi)| {
                let ext = hi - lo;
                let axis = if ext.x < ext.y { Vec3::X } else { Vec3::Y };
                [(lo, hi, axis), (lo, hi, -axis)]
            })
            .collect();
        let rungs = par_map(&faces, |&(lo, hi, n)| rung(world, lo, hi, n));
        let mut ends = Vec::new();
        let safe = |r: &(Vec3, Vec3, bool, bool)| !hazard(hazards, r.0) && !hazard(hazards, r.1);
        for (foot, head, up, down) in rungs.into_iter().flatten().filter(safe) {
            let f = self.add(self.column(foot), foot);
            let h = self.add(self.column(head), head);
            if up {
                self.link(f, h);
            }
            if down {
                self.link(h, f);
            }
            ends.extend([f, h]);
        }
        // Each end to the nodes on its floor around it, each way walked.
        let mut jobs = Vec::new();
        for &e in &ends {
            let p = self.nodes[e as usize];
            let (cx, cy) = self.column(p);
            for dx in -2..=2 {
                for dy in -2..=2 {
                    for &m in self.columns.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                        let q = self.nodes[m as usize];
                        if m != e && (q.z - p.z).abs() < Z_MERGE {
                            jobs.extend([(e, m), (m, e)]);
                        }
                    }
                }
            }
        }
        self.walk_jobs(world, &jobs, false);
        ends
    }

    /// Walks each `(a, b)` node to node on `b`'s floor and links the ones
    /// that arrive. With `sidestep`, a walk that does not is tried again
    /// at [`SIDESTEP`] either side of `b`, across the line: a run pinned on
    /// a door frame's corner can slide past it a few units over.
    fn walk_jobs(&mut self, world: &CollisionWorld, jobs: &[(u32, u32)], sidestep: bool) {
        let walked = par_map(jobs, |&(a, b)| {
            let to = self.nodes[b as usize];
            let from = self.nodes[a as usize];
            let across = (to - from).truncate().perp().normalize_or_zero() * SIDESTEP;
            let tries: &[f32] = if sidestep { &[0.0, 1.0, -1.0] } else { &[0.0] };
            tries.iter().find_map(|&k| {
                let target = to.truncate() + across * k;
                match walk_as(world, from, target, Some(to.z), Gait::Forward) {
                    Walked::Arrived(_, jumped) => Some(jumped),
                    _ => None,
                }
            })
        });
        for (&(a, b), jumped) in jobs.iter().zip(walked) {
            if let Some(jumped) = jumped {
                self.link_as(a, b, jumped);
            }
        }
    }

    /// Where a run from node `n` toward column `c`'s centre comes to rest,
    /// skipping the walk when the graph already proves the answer: a level
    /// cardinal edge already walked the other way (half a flood's walks), or
    /// a level diagonal across a square whose four sides run both ways.
    fn step(&self, world: &CollisionWorld, n: u32, c: (i32, i32)) -> Option<(Vec3, bool)> {
        let from = self.nodes[n as usize];
        let (cx, cy) = self.column(from);
        let level = |m: u32| (self.nodes[m as usize].z - from.z).abs() < LEVEL;
        // Walked without a jump: a jump one way says nothing of the other.
        let has =
            |a: u32, b: u32| self.edges[a as usize].contains(&b) && !self.jumps.contains(&(a, b));
        if let Some(d) = self.find(c, from.z).filter(|&d| level(d)) {
            let proven = if c.0 == cx || c.1 == cy {
                has(d, n)
            } else {
                match (self.find((c.0, cy), from.z), self.find((cx, c.1), from.z)) {
                    (Some(x), Some(y)) => {
                        level(x)
                            && level(y)
                            && [(n, x), (x, d), (n, y), (y, d)]
                                .iter()
                                .all(|&(a, b)| has(a, b) && has(b, a))
                    }
                    _ => false,
                }
            };
            if proven {
                return Some((self.nodes[d as usize], false));
            }
        }
        walk(world, &self.ladder_boxes, from, self.center(c))
    }

    fn column(&self, p: Vec3) -> (i32, i32) {
        (
            (p.x / self.spacing).round() as i32,
            (p.y / self.spacing).round() as i32,
        )
    }

    fn center(&self, c: (i32, i32)) -> glam::Vec2 {
        glam::Vec2::new(c.0 as f32, c.1 as f32) * self.spacing
    }

    fn link(&mut self, a: u32, b: u32) {
        self.link_as(a, b, false);
    }

    /// Links `a -> b`, a jump edge when `jumped`. A walk that proves an edge
    /// without a jump clears its jump.
    fn link_as(&mut self, a: u32, b: u32, jumped: bool) {
        if a == b {
            return;
        }
        if !self.edges[a as usize].contains(&b) {
            self.edges[a as usize].push(b);
            if jumped {
                self.jumps.insert((a, b));
            }
        } else if !jumped {
            self.jumps.remove(&(a, b));
        }
    }

    fn add(&mut self, c: (i32, i32), p: Vec3) -> u32 {
        let n = self.nodes.len() as u32;
        self.nodes.push(p);
        self.edges.push(Vec::new());
        self.columns.entry(c).or_default().push(n);
        n
    }

    fn find(&self, c: (i32, i32), z: f32) -> Option<u32> {
        self.columns
            .get(&c)?
            .iter()
            .copied()
            .find(|&n| (self.nodes[n as usize].z - z).abs() < Z_MERGE)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.iter().map(Vec::len).sum()
    }

    /// The node nearest `p` among the 5x5 columns around it, a floor's height
    /// difference costing four times a horizontal unit so a node one storey
    /// up loses to one beside the feet. `None` off the graph.
    pub fn nearest(&self, p: [f32; 3]) -> Option<u32> {
        let p = Vec3::from(p);
        let (cx, cy) = self.column(p);
        let mut best: Option<(f32, u32)> = None;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let Some(col) = self.columns.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &n in col {
                    let d = self.nodes[n as usize] - p;
                    let score = d.truncate().length() + 4.0 * d.z.abs();
                    // Ties go to the lower index, so the pick never depends
                    // on the order the columns are visited in.
                    if best.is_none_or(|(s, b)| score < s || (score == s && n < b)) {
                        best = Some((score, n));
                    }
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// A* from `from` to `to` over the directed edges, Euclidean cost and
    /// heuristic. The node list runs from `from` to `to` inclusive; `None`
    /// when `to` is unreachable or the search runs past its budget.
    pub fn path(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        self.path_avoiding(from, to, &[])
    }

    /// [`Self::path`] without the directed edges in `avoid`.
    pub fn path_avoiding(&self, from: u32, to: u32, avoid: &[(u32, u32)]) -> Option<Vec<u32>> {
        if !self.reaches(from, to) {
            return None;
        }
        match Search::new(self, from, to, avoid.to_vec()).run(self, &mut { u32::MAX }) {
            Plan::Found(p) => Some(p),
            _ => None,
        }
    }

    /// [`Self::path`], or when `to` is out of reach, the path to the
    /// reachable node nearest it: a goal the graph does not reach (an
    /// objective down a bunker's stairs, on a pitch too coarse for them)
    /// is still closed in on along the graph, not wandered at.
    pub fn path_toward(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        if from as usize >= self.nodes.len() || to as usize >= self.nodes.len() {
            return None;
        }
        self.path(from, self.nearest_reached(from, to)?)
    }

    /// Each node's strongly connected component, numbered from 0 (Kosaraju,
    /// iterative). Two nodes share a number when each reaches the other.
    pub fn components(&self) -> Vec<u32> {
        let n = self.nodes.len();
        // Pass 1: finish order of a DFS over the forward edges.
        let mut order = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        for root in 0..n {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut stack = vec![(root as u32, 0usize)];
            while let Some((v, i)) = stack.last_mut() {
                if let Some(&w) = self.edges[*v as usize].get(*i) {
                    *i += 1;
                    if !seen[w as usize] {
                        seen[w as usize] = true;
                        stack.push((w, 0));
                    }
                } else {
                    order.push(*v);
                    stack.pop();
                }
            }
        }
        // Pass 2: flood the reversed edges in reverse finish order.
        let mut back = vec![Vec::new(); n];
        for (a, out) in self.edges.iter().enumerate() {
            for &b in out {
                back[b as usize].push(a as u32);
            }
        }
        let mut comp = vec![u32::MAX; n];
        let mut next = 0;
        for &root in order.iter().rev() {
            if comp[root as usize] != u32::MAX {
                continue;
            }
            comp[root as usize] = next;
            let mut stack = vec![root];
            while let Some(v) = stack.pop() {
                for &w in &back[v as usize] {
                    if comp[w as usize] == u32::MAX {
                        comp[w as usize] = next;
                        stack.push(w);
                    }
                }
            }
            next += 1;
        }
        comp
    }

    /// Test-facing: a graph from explicit nodes and directed edges, on the
    /// finest pitch.
    pub fn from_parts(nodes: Vec<Vec3>, edges: Vec<Vec<u32>>) -> NavGraph {
        let mut g = NavGraph {
            nodes: Vec::new(),
            edges,
            jumps: HashSet::new(),
            leaps: HashSet::new(),
            spacing: SPACING_MIN,
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            ladder_boxes: Vec::new(),
        };
        for p in nodes {
            let c = g.column(p);
            g.columns.entry(c).or_default().push(g.nodes.len() as u32);
            g.nodes.push(p);
        }
        g.index_components();
        g
    }
}

/// The graphs built so far this process, by map. A map cycle comes back
/// to a map without a second build, and the test gates that start a server
/// per test share one.
type CacheKey = (String, usize, usize, usize, usize, Vec<[u32; 3]>);
static CACHE: std::sync::Mutex<Vec<(CacheKey, Arc<NavGraph>)>> = std::sync::Mutex::new(Vec::new());

/// The key carries the collision's counts and the spawn points too, so a
/// test world built without the static props is a different graph from the
/// real one.
fn cache_key(map: &str, world: &crate::world::World) -> CacheKey {
    let c = &world.collision;
    (
        map.to_ascii_lowercase(),
        c.brushes.len(),
        c.tris.len(),
        c.model_tris.len(),
        world.hazards.len(),
        world
            .spawn_points
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect(),
    )
}

/// The cached graph under `key`, else `build`'s, cached. The lock is held
/// across the build, so two servers asking at once build it once.
fn cached(key: CacheKey, build: impl FnOnce() -> NavGraph) -> Arc<NavGraph> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, g)) = cache.iter().find(|(k, _)| *k == key) {
        return g.clone();
    }
    let t = std::time::Instant::now();
    let g = Arc::new(build());
    log::info!(
        "nav graph for {}: {} nodes, {} edges, spacing {}, built in {} ms",
        key.0,
        g.len(),
        g.edge_count(),
        g.spacing,
        t.elapsed().as_millis()
    );
    cache.push((key, g.clone()));
    g
}

/// `map`'s graph, built on first use on the calling thread.
pub fn graph_for(map: &str, world: &crate::world::World) -> Arc<NavGraph> {
    cached(cache_key(map, world), || {
        NavGraph::build(&world.collision, &world.spawn_points, &world.hazards)
    })
}

/// A graph on its way: built on a thread of its own over a snapshot of the
/// collision, so the server keeps ticking through a build of a second or
/// more (bot-navigation.md, "Build times").
pub struct NavJob {
    ready: Option<Arc<NavGraph>>,
    building: Option<std::thread::JoinHandle<Arc<NavGraph>>>,
}

impl NavJob {
    /// The cached graph when there is one, else a build started over a copy
    /// of `world`'s collision as it stands now: the links and poses scripts
    /// change later never reach the build.
    pub fn start(map: &str, world: &crate::world::World) -> NavJob {
        let key = cache_key(map, world);
        // A lock held elsewhere is a build in progress; the thread waits on
        // it, never the tick.
        if let Ok(cache) = CACHE.try_lock()
            && let Some((_, g)) = cache.iter().find(|(k, _)| *k == key)
        {
            return NavJob {
                ready: Some(g.clone()),
                building: None,
            };
        }
        let collision = world.collision.clone();
        let seeds = world.spawn_points.clone();
        let hazards = world.hazards.clone();
        let build = move || {
            // One core stays the tick's, so a build doesn't jitter the
            // schedule it runs beside.
            SPARE_CORE.set(true);
            cached(key, || NavGraph::build(&collision, &seeds, &hazards))
        };
        NavJob {
            ready: None,
            building: Some(std::thread::spawn(build)),
        }
    }

    /// The graph once it is built; `wait` blocks until it is.
    pub fn poll(&mut self, wait: bool) -> Option<Arc<NavGraph>> {
        if let Some(h) = self.building.take_if(|h| wait || h.is_finished()) {
            self.ready = Some(h.join().expect("the nav build panicked"));
        }
        self.ready.clone()
    }
}

/// Horizontal distance at which a waypoint counts as reached.
const REACH: f32 = 24.0;
/// A waypoint further than this many grid steps away means the bot has left
/// its path (knocked back, fell off a ledge): plan again.
const OFF_PATH_STEPS: f32 = 4.0;
/// Ticks without closing on the waypoint before the follower gives up on
/// it, and the ticks it then leaves the bot to its own unstick.
const STUCK_TICKS: u32 = 40;
const REST_TICKS: u32 = 20;
/// The most of the tick's plan budget one follower's search takes, so a
/// long search leaves some for the bots after it.
const PLAN_SLICE: u32 = 2000;
/// How long the edge a follower got stuck on stays out of its plans. What
/// blocks a proven edge is mostly a body: a bot guarding the bomb at the
/// foot of mp_rocket's stairs held another there for 11 s.
const AVOID_TICKS: u32 = 200;
/// How far below where its path ended a body walking at a point goal has
/// to drop before the follower plans for the point again: more than a stair
/// or a step off a kerb.
const ARRIVED_DROP: f32 = 48.0;
/// How near a leap's foot, flat, a bot comes to rest before it leaps.
pub const LEAP_FOOT: f32 = 12.0;
/// A jump edge's top counts as reached only within this of its height:
/// under a stair step, over the slack a slope leaves.
const JUMP_PASS: f32 = 16.0;
/// A node the next edge drops more than a step from counts as reached only
/// within this of its height (bot-navigation.md, section 3).
const DROP_PASS: f32 = 8.0;
/// A roam goal is picked at least this far away when it can be.
const ROAM_MIN: f32 = 1000.0;
/// A seen enemy has to move this far off the planned destination before the
/// path is planned again.
const RETARGET: f32 = 128.0;

/// One bot's walk along the graph: the destination, the planned path and
/// the bookkeeping that notices when it has gone wrong. The server keeps one
/// per bot and asks it for the waypoint every tick.
#[derive(Default)]
pub struct Follower {
    dest: Option<u32>,
    path: Vec<u32>,
    /// Index into `path` of the current waypoint.
    next: usize,
    /// Closest the bot has come to the current waypoint, and the ticks since
    /// it last closed in.
    best: f32,
    idle: u32,
    rest: u32,
    /// A point goal's path is walked out; the bot heads at the point itself.
    arrived: bool,
    /// Where the body stood when the path was walked out. Walking at the
    /// point off the graph can drop it a floor (mp_depot's Retrieval
    /// documents sit by an upper floor's edge), and from there the point is
    /// planned for again.
    arrived_at: Vec3,
    /// Calls to [`Self::waypoint`], one per tick: the clock `avoid` runs on.
    clock: u32,
    /// Edges it got stuck on, left out of its plans until the tick given.
    avoid: Vec<(u32, u32, u32)>,
    /// The plan under way, and whether it leaves out `avoid` (a plain one
    /// follows if it fails).
    search: Option<Search>,
    round: bool,
    /// The threat an [`Goal::Away`](crate::bots::Goal::Away) destination
    /// was picked against.
    threat: Option<Vec3>,
}

impl Follower {
    /// The waypoint toward `goal` for a bot standing at `at`; `still` says
    /// it stands at rest with a jump ready ([`ready_to_leap`]). `world`
    /// re-proves a leap from where the bot rests (none in unit tests).
    /// `budget` is the tick's shared A* budget in expanded nodes; a bot whose
    /// plan it does not finish gets no waypoint this tick and resumes the
    /// search next tick, so neither one long plan nor many bots planning at
    /// once (the tick the graph lands) stall a tick.
    /// `rand` picks roam destinations.
    #[allow(clippy::too_many_arguments)]
    pub fn waypoint(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        at: [f32; 3],
        still: bool,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<[f32; 3]> {
        use crate::bots::Goal;
        self.clock += 1;
        let clock = self.clock;
        self.avoid.retain(|e| e.2 > clock);
        if self.rest > 0 {
            self.rest -= 1;
            return None;
        }
        let here = Vec3::from(at);
        match goal {
            Goal::Hold => {
                self.reset();
                return None;
            }
            Goal::Roam => {}
            Goal::Away(t) => {
                let t = Vec3::from(t);
                if self.threat.is_none_or(|p| p.distance(t) > RETARGET) {
                    self.reset();
                    self.threat = Some(t);
                }
            }
            Goal::To(p) => {
                let moved = self
                    .dest
                    .is_none_or(|d| g.nodes[d as usize].distance(Vec3::from(p)) > RETARGET);
                if moved {
                    self.reset();
                    self.dest = g.nearest(p);
                } else if self.arrived {
                    let fell = here.z < self.arrived_at.z - ARRIVED_DROP;
                    if !fell {
                        return Some(p);
                    }
                    self.reset();
                    self.dest = g.nearest(p);
                }
            }
        }
        if self.path.is_empty() {
            self.path = self.plan(g, goal, here, budget, rand)?;
            self.next = 0;
            self.best = f32::INFINITY;
            self.idle = 0;
        }
        let flat = |p: Vec3| (p - here).truncate().length();
        // Past a waypoint once inside its reach, or once nearer the next one
        // than the waypoint itself is: that keeps a bot from doubling back
        // to touch a node it cut the corner on. Either only on the
        // waypoint's floor: a ladder's head is right above its foot.
        while let Some(&w) = self.path.get(self.next) {
            // The top of a jump is passed only standing on it: from its
            // foot the node is in reach flat and the jump not yet made.
            // So is a node a drop leaves from: from under it the drop is
            // not where the graph proved it.
            let rise = if self.jumping(g) {
                JUMP_PASS
            } else if self.dropping(g) {
                DROP_PASS
            } else {
                48.0
            };
            // A leap's foot is passed only at rest on it: the leap was
            // proved from a standstill there.
            let foot = self.holding(g);
            let w = g.nodes[w as usize];
            let level = (w.z - here.z).abs() < rise;
            let past = !foot
                && self.path.get(self.next + 1).is_some_and(|&n| {
                    let n = g.nodes[n as usize];
                    flat(n) < (n - w).truncate().length()
                });
            let reached = if foot {
                flat(w) < LEAP_FOOT && still
            } else {
                flat(w) < REACH
            };
            if level && reached && foot && !self.leap_holds(world, here, g) {
                // The leap fails from where this body came to rest: round it.
                let edge = (self.path[self.next], self.path[self.next + 1]);
                self.avoid.push((edge.0, edge.1, self.clock + AVOID_TICKS));
                self.reset();
                return None;
            }
            if level && (reached || past) {
                self.next += 1;
                self.best = f32::INFINITY;
                self.idle = 0;
            } else {
                break;
            }
        }
        let Some(&w) = self.path.get(self.next) else {
            // Arrived. A roam picks somewhere new; a point is walked at.
            self.path.clear();
            return match goal {
                Goal::To(p) => {
                    self.arrived = true;
                    self.arrived_at = here;
                    Some(p)
                }
                _ => {
                    self.dest = None;
                    None
                }
            };
        };
        let w = g.nodes[w as usize];
        if flat(w) > OFF_PATH_STEPS * g.spacing {
            self.path.clear();
            return None;
        }
        // Progress counts height too: a climb closes on nothing flat.
        let d = flat(w) + (w.z - here.z).abs();
        if d < self.best - 1.0 {
            self.best = d;
            self.idle = 0;
        } else {
            self.idle += 1;
            if self.idle > STUCK_TICKS {
                // Stuck on the way from the last node: plan round that edge
                // next tick. Before the first node, leave the bot to its own
                // unstick for a spell.
                if let Some(from) = self.next.checked_sub(1).map(|i| self.path[i]) {
                    let to = self.path[self.next];
                    self.avoid.push((from, to, self.clock + AVOID_TICKS));
                } else {
                    self.rest = REST_TICKS;
                }
                self.reset();
                return None;
            }
        }
        Some(w.into())
    }

    /// The path toward the destination, once the search for it is done.
    /// A search starts only while `budget` has some left and runs at most
    /// [`PLAN_SLICE`] of it a tick; a destination the start does not reach
    /// fails before any search, and a point goal then heads for the reached
    /// node nearest it instead.
    fn plan(
        &mut self,
        g: &NavGraph,
        goal: crate::bots::Goal,
        here: Vec3,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<Vec<u32>> {
        if *budget == 0 {
            return None;
        }
        if self.search.is_none() {
            let start = g.nearest(here.into())?;
            if self.dest.is_none() {
                self.dest = match goal {
                    crate::bots::Goal::Roam => g.roam_from(start, here, rand),
                    crate::bots::Goal::Away(t) => g.flee_from(start, here, t.into(), rand),
                    _ => None,
                };
            }
            let to = match goal {
                crate::bots::Goal::To(_) => self.dest.and_then(|d| g.nearest_reached(start, d)),
                _ => self.dest.filter(|&d| g.reaches(start, d)),
            };
            let Some(to) = to else {
                // Unreachable from here: a roam picks again next time.
                self.reset();
                self.rest = REST_TICKS;
                return None;
            };
            // Round the edges it got stuck on when there is a way round,
            // else through them again: whatever blocked one may have moved.
            let avoid: Vec<(u32, u32)> = self.avoid.iter().map(|e| (e.0, e.1)).collect();
            self.round = !avoid.is_empty();
            self.search = Some(Search::new(g, start, to, avoid));
        }
        let search = self.search.as_mut()?;
        let mut slice = (*budget).min(PLAN_SLICE);
        let before = slice;
        let plan = search.run(g, &mut slice);
        // A plan that finds its path at once still costs one.
        *budget -= (before - slice).max(1).min(*budget);
        match plan {
            Plan::Pending => None,
            Plan::Found(path) => {
                self.search = None;
                Some(path)
            }
            Plan::Failed if self.round => {
                let s = self.search.take()?;
                self.round = false;
                self.search = Some(Search::new(g, s.from, s.to, Vec::new()));
                None
            }
            Plan::Failed => {
                self.reset();
                self.rest = REST_TICKS;
                None
            }
        }
    }

    /// Whether the leap from the current waypoint, a leap's foot, arrives run
    /// from `at`, where the bot came to rest: the graph proved it from the
    /// node, and a body a few units nearer the lip leaves the ground before
    /// it has the pace to clear the gap. Without a world, taken as proved.
    fn leap_holds(&self, world: Option<&CollisionWorld>, at: Vec3, g: &NavGraph) -> bool {
        let (Some(world), Some(&top)) = (world, self.path.get(self.next + 1)) else {
            return true;
        };
        let top = g.nodes[top as usize];
        matches!(
            walk_as(world, at, top.truncate(), Some(top.z), Gait::Leap),
            Walked::Arrived(..)
        )
    }

    /// Whether the current waypoint is a leap's foot: the bot comes to rest
    /// on it before it heads on.
    pub fn holding(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => g.leaps.contains(&(a, b)),
            _ => false,
        }
    }

    /// Whether the way to the current waypoint is a leap.
    pub fn leaping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.leaps.contains(&(a, b)),
            _ => false,
        }
    }

    /// Whether the edge on from the current waypoint drops more than a step.
    fn dropping(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z < g.nodes[a as usize].z - vcod_common::pmove::STEPSIZE
            }
            _ => false,
        }
    }

    /// Whether the way to the current waypoint is a jump edge.
    pub fn jumping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.jumps.contains(&(a, b)),
            _ => false,
        }
    }

    fn reset(&mut self) {
        self.dest = None;
        self.search = None;
        self.path.clear();
        self.arrived = false;
    }
}

impl NavGraph {
    /// A roam destination: of a handful of random nodes in the component of
    /// node `start`, which stands at `from`, the first at least [`ROAM_MIN`]
    /// away, else the farthest. A pick the start cannot reach would cost a
    /// search of everything it can.
    fn roam_from(&self, start: u32, from: Vec3, rand: &mut dyn FnMut() -> i32) -> Option<u32> {
        let pool = self.members.get(*self.comp.get(start as usize)? as usize)?;
        let mut best: Option<(f32, u32)> = None;
        for _ in 0..8 {
            let n = pool[rand() as usize % pool.len()];
            let d = self.nodes[n as usize].distance(from);
            if d >= ROAM_MIN {
                return Some(n);
            }
            if best.is_none_or(|(b, _)| d > b) {
                best = Some((d, n));
            }
        }
        best.map(|(_, n)| n)
    }
}

/// Where a [`Search`] stands after a call to [`Search::run`].
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// The node list from `from` to `to` inclusive.
    Found(Vec<u32>),
    /// No way, or past [`MAX_EXPANSIONS`].
    Failed,
    /// The budget ran out first; call again.
    Pending,
}

/// An A* under way, over the directed edges with Euclidean cost and
/// heuristic. [`Search::run`] expands at most its budget per call, so a long
/// plan spreads over ticks rather than stalling one.
pub struct Search {
    from: u32,
    to: u32,
    avoid: Vec<(u32, u32)>,
    cost: Vec<f32>,
    came: Vec<u32>,
    open: BinaryHeap<Open>,
    expanded: usize,
}

impl Search {
    pub fn new(g: &NavGraph, from: u32, to: u32, avoid: Vec<(u32, u32)>) -> Search {
        let n = g.nodes.len();
        let mut s = Search {
            from,
            to,
            avoid,
            cost: vec![f32::INFINITY; n],
            came: vec![u32::MAX; n],
            open: BinaryHeap::new(),
            expanded: 0,
        };
        if (from as usize) < n && (to as usize) < n {
            s.cost[from as usize] = 0.0;
            s.open.push(Open {
                f: g.nodes[from as usize].distance(g.nodes[to as usize]),
                node: from,
            });
        }
        s
    }

    /// Expands nodes until the path is found or ruled out, each one taken
    /// from `budget`; [`Plan::Pending`] once `budget` is 0.
    pub fn run(&mut self, g: &NavGraph, budget: &mut u32) -> Plan {
        let goal = g.nodes.get(self.to as usize).copied().unwrap_or(Vec3::ZERO);
        let h = |i: u32| g.nodes[i as usize].distance(goal);
        while let Some(&Open { f, node }) = self.open.peek() {
            if node == self.to {
                let mut out = vec![node];
                let mut at = node;
                while at != self.from {
                    at = self.came[at as usize];
                    out.push(at);
                }
                out.reverse();
                return Plan::Found(out);
            }
            // A stale heap entry: the node was reached cheaper since.
            if f > self.cost[node as usize] + h(node) + 1e-3 {
                self.open.pop();
                continue;
            }
            if *budget == 0 {
                return Plan::Pending;
            }
            self.open.pop();
            *budget -= 1;
            self.expanded += 1;
            if self.expanded > MAX_EXPANSIONS {
                return Plan::Failed;
            }
            let here = g.nodes[node as usize];
            for &next in &g.edges[node as usize] {
                if self.avoid.contains(&(node, next)) {
                    continue;
                }
                let c = self.cost[node as usize] + here.distance(g.nodes[next as usize]);
                if c < self.cost[next as usize] {
                    self.cost[next as usize] = c;
                    self.came[next as usize] = node;
                    self.open.push(Open {
                        f: c + h(next),
                        node: next,
                    });
                }
            }
        }
        Plan::Failed
    }
}

impl NavGraph {
    /// A destination away from `threat`: of a handful of random nodes in
    /// the component of node `start`, which stands at `from`, the one
    /// farthest from the threat among those nearer the bot than the threat
    /// (a way there that runs past the threat is no escape), else the
    /// farthest of all.
    fn flee_from(
        &self,
        start: u32,
        from: Vec3,
        threat: Vec3,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<u32> {
        let pool = self.members.get(*self.comp.get(start as usize)? as usize)?;
        let mut best: Option<(bool, f32, u32)> = None;
        for _ in 0..12 {
            let n = pool[rand() as usize % pool.len()];
            let p = self.nodes[n as usize];
            let key = (p.distance(from) < p.distance(threat), p.distance(threat));
            if best.is_none_or(|(s, d, _)| key > (s, d)) {
                best = Some((key.0, key.1, n));
            }
        }
        best.map(|(_, _, n)| n)
    }
}

/// A* open-list entry; a min-heap on `f`, ties to the lower node index.
#[derive(PartialEq)]
struct Open {
    f: f32,
    node: u32,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

thread_local! {
    /// Set on a [`NavJob`]'s thread: its [`par_map`]s leave one core free.
    static SPARE_CORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `f` over `items` on every core (all but one under [`SPARE_CORE`]),
/// results in `items` order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = if SPARE_CORE.get() {
        cores.saturating_sub(1).max(1)
    } else {
        cores
    };
    if threads == 1 || items.len() < 16 {
        return items.iter().map(f).collect();
    }
    // Handed out one at a time: a walk into open ground is a few ticks and
    // one along a wall runs out its budget, so even chunks would leave most
    // cores idle behind the slowest.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().expect("a nav walk panicked") {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every item mapped"))
        .collect()
}

/// The world's ladders as axial boxes: the `SURF_LADDER` brushes of the
/// world model, with touching ones merged, since a tall ladder is often
/// several brushes stacked.
fn ladders(world: &CollisionWorld) -> Vec<(Vec3, Vec3)> {
    let mut boxes: Vec<(Vec3, Vec3)> = world
        .brushes
        .iter()
        .filter(|b| b.model == 0 && b.surface_flags & vcod_common::collision::SURF_LADDER != 0)
        .filter_map(|b| {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &(n, d) in &b.planes {
                for i in 0..3 {
                    if n[i] == 1.0 {
                        hi[i] = d;
                    } else if n[i] == -1.0 {
                        lo[i] = -d;
                    }
                }
            }
            lo.cmple(hi).all().then_some((lo, hi))
        })
        .collect();
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                if (a.0 - 2.0).cmple(b.1).all() && (b.0 - 2.0).cmple(a.1).all() {
                    boxes[i] = (a.0.min(b.0), a.1.max(b.1));
                    boxes.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    boxes
}

/// A ladder face's foot and head, `(foot, head, up, down)`: the foot is a
/// body settled in front of the face with normal `n`, the head where a climb
/// from there facing the face comes to rest a few steps past the top. `up`
/// and `down` say whether the bots' steering between the two arrives: a run
/// at the head looking up, a back down onto the face (`bots::steer`).
fn rung(world: &CollisionWorld, lo: Vec3, hi: Vec3, n: Vec3) -> Option<(Vec3, Vec3, bool, bool)> {
    let mid = (lo + hi) * 0.5;
    let half = (hi - lo).dot(n.abs()) * 0.5;
    let front = mid.truncate() + n.truncate() * (half + 16.0);
    let side = glam::Vec2::new(-n.y, n.x);
    let probe = PlayerState::spawn(lo, 0.0);
    // A ladder brush often runs on below the floor it stands on, and a wall
    // can crowd one edge of it, so the drop starts at the first height in
    // front of it a body fits, the middle first, then up to 16 units aside.
    let start = (1..=FOOT_TRIES)
        .map(|i| lo.z + 16.0 * i as f32)
        .take_while(|&z| z < hi.z)
        .flat_map(|z| [0.0, -8.0, 8.0, -16.0, 16.0].map(|s| (front + side * s).extend(z)))
        .find(|&p| !world.box_trace(p, p, probe.mins(), probe.maxs()).startsolid)?;
    // Dropped facing away, or an airborne grab would hang it on the face.
    let foot = settle(world, start, (n.y).atan2(n.x).to_degrees())?;
    if foot.z < lo.z - 48.0 || under_ground(world, foot) {
        return None;
    }
    let yaw = (-n.y).atan2(-n.x).to_degrees();
    let mut ps = PlayerState::spawn(foot, yaw);
    let mut sim = Sim::default();
    let mut climbed = false;
    let mut top: Option<Vec3> = None;
    for _ in 0..LADDER_TICKS {
        let pitch = if ps.on_ladder { -LADDER_PITCH } else { 0.0 };
        // A few steps in past the lip, then the keys come up and it stands.
        let forward = match top {
            Some(t) if (ps.origin - t).truncate().length() >= LADDER_STEP_IN => 0,
            _ => 127,
        };
        let cmd = UserCmd {
            forward,
            angles: [(pitch * ANGLE2SHORT) as i32, (yaw * ANGLE2SHORT) as i32, 0],
            ..NULL_USERCMD
        };
        sim.tick(world, &mut ps, &cmd);
        climbed |= ps.on_ladder;
        if top.is_none() && climbed && ps.on_ground && ps.origin.z > foot.z + 48.0 {
            top = Some(ps.origin);
        }
        // Stopped past the top, or pinned at the lip against whatever
        // stands beyond it.
        if top.is_some() && ps.on_ground && ps.velocity.length() < 1.0 {
            break;
        }
    }
    top?;
    if !ps.on_ground || under_ground(world, ps.origin) {
        return None;
    }
    let head = ps.origin;
    let arrives = |from: Vec3, to: Vec3, gait| {
        matches!(
            walk_as(world, from, to.truncate(), Some(to.z), gait),
            Walked::Arrived(_, false)
        )
    };
    let up = arrives(foot, head, Gait::Forward);
    let down = arrives(head, foot, Gait::Creep);
    (up || down).then_some((foot, head, up, down))
}

/// Heights, 16 units apart from the ladder's bottom, a foot's drop is
/// tried from.
const FOOT_TRIES: usize = 6;
/// How far a climb walks on past the top of a ladder before it stops.
const LADDER_STEP_IN: f32 = 24.0;

/// Whether a standing body with its feet at `p` touches one of `hazards`.
pub(crate) fn hazard(hazards: &[BrushHull], p: Vec3) -> bool {
    use vcod_common::pmove::{HALF_WIDTH, Stance};
    let half = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height() * 0.5);
    !hazards.is_empty() && box_contacts_hulls(p + Vec3::Z * half.z, half, Vec3::ZERO, hazards)
}

/// Idle cmds until a body dropped at `p` stands on something; `None` when it
/// falls out of the world or never lands.
fn settle(world: &CollisionWorld, p: Vec3, yaw: f32) -> Option<Vec3> {
    let mut ps = PlayerState::spawn(p + Vec3::Z, yaw);
    let idle = UserCmd {
        angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
        ..NULL_USERCMD
    };
    let mut sim = Sim::default();
    for _ in 0..40 {
        sim.tick(world, &mut ps, &idle);
        if ps.on_ground {
            return Some(ps.origin);
        }
    }
    None
}

/// A walk's tick budget for `dist` units: twice the run time, plus slack for
/// a stair or a slide along a wall.
fn run_ticks(dist: f32) -> usize {
    (dist / (vcod_common::pmove::SPEED_RUN * 0.05) * 2.0) as usize + 4
}

/// How far to either side of a node a walk back to it is retried
/// (`NavGraph::link_back`). VERIFIED (measured, `mp_rocket`): the walk west
/// from (11858, 4224, 262) pins on a corner at x 11840 aimed at the node at
/// (11803, 4226), and arrives aimed 8 units north of it.
const SIDESTEP: f32 = 8.0;
/// How far across from a ladder box a fall is retried backing down.
const LADDER_NEAR: f32 = 96.0;
/// The ticks a walk's budget stretches by while the body is in the air:
/// a fall of [`MAX_DROP`] takes 14.
const FALL_TICKS: usize = 16;

/// Inside this of its target, horizontally, a body backing onto a ladder
/// below a ledge creeps: an airborne grab reaches 17 units past the box
/// (`check_ladder_move`, the box shrunk 6 and the trace 8), and at a run the
/// body is past that within the tick it leaves the lip.
const CREEP_RADIUS: f32 = 24.0;
/// The creep's move key, about 16 units/s. The body leaves the lip at 15
/// units past the face and is first low enough to grab (under the ladder's
/// top) a tick later, so two ticks of travel have to stay under 2 units.
const CREEP: i8 = 15;
/// [`CREEP`], for the bots' tests.
#[cfg(test)]
pub const CREEP_KEY: i8 = CREEP;
/// The ticks the creep adds to a backing walk's budget.
const CREEP_TICKS: usize = 16;

/// The move key a body backing toward a point `flat` units away sends:
/// full off the ground near it and on a ladder, a creep at the lip.
pub fn back_move(flat: f32, on_ladder: bool) -> i8 {
    if on_ladder || flat > CREEP_RADIUS {
        -127
    } else {
        -CREEP
    }
}

/// The heading a creep holds. The lip is a step or two from the point it
/// backs toward, and re-aiming as the body passes over it would turn it
/// round in the air, away from the face its grab traces toward.
#[derive(Default, Clone, Copy)]
pub struct Creep(Option<f32>);

impl Creep {
    /// The yaw to send with move key `forward`: `want` at full speed, the
    /// first creeping tick's yaw for as long as the creep lasts.
    pub fn hold(&mut self, forward: i8, want: f32) -> f32 {
        if forward == -CREEP {
            *self.0.get_or_insert(want)
        } else {
            self.0 = None;
            want
        }
    }
}

/// How a walk is driven: facing its way, or backing along it facing the
/// other way, which is how a body gets onto a ladder below a ledge (the grab
/// traces along the view, `ladder_move`), at a run or creeping at the lip
/// ([`back_move`]).
#[derive(Clone, Copy, PartialEq)]
enum Gait {
    Forward,
    /// [`Gait::Forward`] that also jumps at a lip ([`lip_ahead`]): across a
    /// gap to a ledge (`NavGraph::link_leaps`).
    Leap,
    Backward,
    Creep,
}

/// What a walk came to.
enum Walked {
    /// Where it came to rest, and whether it took a jump to get there.
    Arrived(Vec3, bool),
    Blocked,
    /// It left the ground and never arrived; a ladder below may still take it.
    Fell,
}

/// [`walk_as`] forward, backward when the forward run fell, and creeping
/// when that fell too. The creep is last because it is slow: a back down
/// off every ledge on a map is most of a build, and only a ladder below
/// needs it, so neither is tried away from one ([`ladder_near`]).
fn walk(
    world: &CollisionWorld,
    ladders: &[(Vec3, Vec3)],
    from: Vec3,
    target: glam::Vec2,
) -> Option<(Vec3, bool)> {
    for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
        if gait == Gait::Creep && !ladder_near(ladders, from) {
            return None;
        }
        match walk_as(world, from, target, None, gait) {
            Walked::Arrived(p, jumped) => return Some((p, jumped)),
            Walked::Blocked => return None,
            Walked::Fell => {}
        }
    }
    None
}

/// Whether a ladder box comes within [`LADDER_NEAR`] of `p` across and
/// spans heights from a drop below it to a storey above: the only place
/// a back down off a ledge can find a grab.
fn ladder_near(ladders: &[(Vec3, Vec3)], p: Vec3) -> bool {
    ladders.iter().any(|&(lo, hi)| {
        let near = Vec3::new(LADDER_NEAR, LADDER_NEAR, 0.0);
        let (lo, hi) = (lo - near, hi + near);
        (lo.x..=hi.x).contains(&p.x)
            && (lo.y..=hi.y).contains(&p.y)
            && hi.z >= p.z - MAX_DROP
            && lo.z <= p.z + Z_MERGE
    })
}

/// Runs a body from `from` toward `target`, re-aiming every tick, and
/// reports where it came to rest when it got within [`ARRIVE`] on the
/// ground (and of the height `floor`, when given) without falling past
/// [`MAX_DROP`], and whether it jumped on the way: once where a forward
/// run stalls, or at the lip for a [`Gait::Leap`].
fn walk_as(
    world: &CollisionWorld,
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    let dist = target.distance(from.truncate());
    let on_floor = |z: f32| floor.is_none_or(|f| (z - f).abs() < ARRIVE);
    if dist < ARRIVE && on_floor(from.z) {
        return Walked::Arrived(from, false);
    }
    // A running start: a following bot never stops at a node, and the
    // spin-up from a standstill was a third of every walk.
    let mut ps = PlayerState::spawn(from, 0.0);
    let dir = (target - from.truncate()) / dist;
    // A leap starts standing: a bot comes to rest at its foot first.
    if gait != Gait::Leap {
        ps.velocity = (dir * vcod_common::pmove::SPEED_RUN).extend(0.0);
    }
    let mut sim = Sim::default();
    let mut budget = run_ticks(dist);
    // A floor-bound walk is a ladder's (`link_ladders`): room to reach it.
    if floor.is_some() {
        budget = budget.max(60);
    }
    if gait == Gait::Creep {
        budget += CREEP_TICKS;
    }
    let mut stalled = 0;
    let mut creep = Creep::default();
    let mut climbed = false;
    let mut fell = false;
    // Where the body last stood or held on; a fall is measured from here.
    let mut support_z = from.z;
    let mut tick = 0;
    let mut airborne = false;
    let cap = budget + FALL_TICKS + run_ticks(dist);
    // A forward run pinned on the ground jumps once (`jump_key`); one that
    // still doesn't arrive reports what it came to before the jump.
    let mut jumped = false;
    let mut jump = false;
    let mut fell_before_jump = false;
    // A body in the air when the budget runs out is let land: a drop down
    // a steep companionway is slower than the run it replaced.
    while tick < budget || (airborne && tick < budget + FALL_TICKS) {
        tick += 1;
        let before = ps.origin;
        let to = target - ps.origin.truncate();
        let (yaw, forward) = match gait {
            Gait::Forward | Gait::Leap => (to.y.atan2(to.x).to_degrees(), 127),
            Gait::Backward => ((-to.y).atan2(-to.x).to_degrees(), -127),
            Gait::Creep => {
                let forward = back_move(to.length(), ps.on_ladder);
                let yaw = (-to.y).atan2(-to.x).to_degrees();
                (creep.hold(forward, yaw), forward)
            }
        };
        // On a ladder the view goes up, where `ladder_move` climbs at full
        // rate (a level view climbs at a third); backing off it climbs down.
        let pitch = if ps.on_ladder { -LADDER_PITCH } else { 0.0 };
        if gait == Gait::Leap && !jumped && ps.on_ground && lip_ahead(world, &ps) {
            jumped = true;
            jump = true;
            fell_before_jump = fell;
            budget = budget.max(tick + JUMP_TICKS + run_ticks(to.length()));
        }
        let cmd = UserCmd {
            forward,
            up: if std::mem::take(&mut jump) { 127 } else { 0 },
            angles: [(pitch * ANGLE2SHORT) as i32, (yaw * ANGLE2SHORT) as i32, 0],
            ..NULL_USERCMD
        };
        sim.tick(world, &mut ps, &cmd);
        // Landed past the budget: the run back to the target from there.
        if airborne && ps.on_ground && tick >= budget {
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max((tick + rest).min(cap));
        }
        airborne = !ps.on_ground && !ps.on_ladder;
        if ps.on_ground || ps.on_ladder {
            support_z = ps.origin.z;
        } else {
            fell = true;
            if ps.origin.z < support_z - MAX_DROP {
                return Walked::Fell;
            }
        }
        if ps.on_ground && target.distance(ps.origin.truncate()) < ARRIVE && on_floor(ps.origin.z) {
            if under_ground(world, ps.origin) {
                return Walked::Blocked;
            }
            return Walked::Arrived(ps.origin, jumped);
        }
        // A climb is slower than a run and straight up, so the budget
        // stretches while it goes on, with the run still left past the top,
        // and progress is the height gained.
        let moved = if ps.on_ladder {
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max(tick + rest).min(LADDER_TICKS + rest);
            (ps.origin.z - before.z).abs() * 4.0
        } else {
            (ps.origin - before).length()
        };
        // Pinned against a wall: two ticks without moving two units (half
        // one, creeping). A run that only slides along a wall, or overshoots
        // off a ledge and turns back, keeps its budget, and so does one
        // hanging in the air at the top of a ladder before it tips over.
        climbed |= ps.on_ladder;
        let least = if forward.abs() < 127 { 0.5 } else { 2.0 };
        let hanging = (climbed || jumped) && !ps.on_ground && !ps.on_ladder;
        if moved < least && !hanging {
            stalled += 1;
            if stalled == 2 {
                if gait == Gait::Forward
                    && !jumped
                    && ps.on_ground
                    && jump_clear(world, &ps, target)
                {
                    jumped = true;
                    jump = true;
                    fell_before_jump = fell;
                    stalled = 0;
                    let rest = run_ticks(target.distance(ps.origin.truncate()));
                    budget = budget.max(tick + JUMP_TICKS + rest);
                    continue;
                }
                break;
            }
        } else {
            stalled = 0;
        }
    }
    if jumped {
        fell = fell_before_jump;
    }
    if fell { Walked::Fell } else { Walked::Blocked }
}

/// Under this many units a tick a body is pinned: a run against a ledge's
/// face, which a jump may clear.
const PINNED: f32 = 2.0;
/// A body this close under a jump edge's top, flat, jumps whatever its pace:
/// past it, a run carries it off the ledge's foot (mp_depot's stair to the
/// documents' step).
const JUMP_UNDER: f32 = 16.0;
/// The ticks a jump stays in the air: 2 * 249.8 / 800 s.
const JUMP_TICKS: usize = 13;

/// Whether a body on a jump edge jumps now, its target `flat` away and
/// `rise` above. A leap jumps at the lip ([`lip_ahead`]), as its walk did;
/// any other jump where the walk stalled, pinned (closing on the target
/// under [`PINNED`] units a tick), or right under a target above it
/// ([`JUMP_UNDER`]). Never once standing on the top.
pub fn jump_cue(leap: bool, lip: bool, closing: f32, flat: f32, rise: f32) -> bool {
    use vcod_common::pmove::STEPSIZE;
    if flat < REACH && rise.abs() < JUMP_PASS {
        false
    } else if leap {
        lip
    } else {
        closing < PINNED || (flat < JUMP_UNDER && rise > STEPSIZE)
    }
}

/// The jump key for a cue: pressed on the ground when the key was up last
/// cmd. `PM_CheckJump` refuses a held key, so a second try needs a release.
pub fn jump_key(on_ground: bool, cue: bool, held: bool) -> i8 {
    if on_ground && cue && !held { 127 } else { 0 }
}

/// A body at rest on the ground with its jump off cooldown: what a leap was
/// proved from.
pub fn ready_to_leap(ps: &PlayerState) -> bool {
    ps.on_ground
        && ps.velocity.truncate().length() * (FRAME_MS as f32 / 1000.0) < PINNED
        && ps.since_jump_ms >= vcod_common::pmove::JUMP_COOLDOWN_MS
}

/// Whether the body `ps` loses the ground in the coming tick at its pace:
/// no floor within a step under where its velocity takes it. A leap jumps
/// there (`Gait::Leap`, and the bots on its edges).
pub fn lip_ahead(world: &CollisionWorld, ps: &PlayerState) -> bool {
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let ahead = ps.origin + (ps.velocity.truncate() * (FRAME_MS as f32 / 1000.0)).extend(0.0);
    let down = ahead - Vec3::Z * vcod_common::pmove::STEPSIZE;
    let t = world.box_trace(ahead, down, mins, maxs);
    !t.startsolid && t.fraction == 1.0
}

/// Whether a jump from where `ps` stands could clear what pins it: room
/// overhead for the jump's height, and from there room ahead toward
/// `target`. A wall to the ceiling fails it, which keeps a jump off the
/// walks that end at one.
fn jump_clear(world: &CollisionWorld, ps: &PlayerState, target: glam::Vec2) -> bool {
    let (mins, maxs) = (ps.mins(), ps.maxs());
    let top = ps.origin + Vec3::Z * vcod_common::pmove::JUMP_HEIGHT;
    if world.box_trace(ps.origin, top, mins, maxs).fraction < 1.0 {
        return false;
    }
    let dir = (target - ps.origin.truncate())
        .normalize_or_zero()
        .extend(0.0);
    let ahead = world.box_trace(top, top + dir * 16.0, mins, maxs);
    !ahead.startsolid && ahead.fraction == 1.0
}

/// Whether `p` lies beneath a sheet of ground. Terrain and patches are
/// surfaces, not solids, so a run that finds a gap at a sheet's edge can
/// walk the map's floor under them (mp_hurtgen's, 48 units below its river
/// bed). From there the ray up either stops on the underside of a triangle
/// whose own face, `(b - a) x (c - a)`, points up, or passes through a
/// one-sided terrain triangle and the same ray back down hits it. Both rays
/// see what a player clips, not the brushes only a shot meets. Nothing else
/// the ray back down meets is ground over the point: a patch (mp_ship's
/// mast ladder stands 540 units under a spar's top facet), or a ceiling
/// just over the head that the ray starts against (a low doorway on
/// mp_pavlov kept 540 nodes behind it off the graph). The rays start a step
/// over the feet, not at the head: mp_carentan's terrain runs 38 units over
/// the base brush west of its boundary wall, and from the head a flood
/// seeded there walked under it into the town.
fn under_ground(world: &CollisionWorld, p: Vec3) -> bool {
    let start = p + Vec3::Z * vcod_common::pmove::STEPSIZE;
    let up = world.point_trace(start, start + Vec3::Z * 8192.0, MASK_PLAYERSOLID, false);
    if let Some(Prim::Tri(t)) = up.hit {
        let [a, b, c] = world.tris[t as usize];
        if (b - a).cross(c - a).normalize_or_zero().z > 0.7 {
            return true;
        }
    }
    let down = world.point_trace(up.endpos - Vec3::Z, start, MASK_PLAYERSOLID, false);
    matches!(down.hit, Some(Prim::Tri(_)))
}

/// The bits of a client a walk carries between cmds.
#[derive(Default)]
struct Sim {
    time: i32,
    delta: [i32; 3],
    ring: EventRing,
}

impl Sim {
    fn tick(&mut self, world: &CollisionWorld, ps: &mut PlayerState, cmd: &UserCmd) {
        let cmd = UserCmd {
            server_time: self.time + FRAME_MS,
            ..*cmd
        };
        let mw = MoveWorld::bare(world);
        for (step, dt) in chop(self.time, &cmd) {
            player_step(ps, &mut self.delta, &mut self.ring, &step, dt, &mw, &[]);
        }
        self.time = cmd.server_time;
    }
}

/// Every spawn point in an ents lump: the gametypes' `mp_*_spawn*` classes
/// and `info_player_start`.
pub fn spawn_points(entities: &str) -> Vec<[f32; 3]> {
    vcod_common::bsp::entity_blocks(entities)
        .iter()
        .filter(|b| {
            b.get("classname").is_some_and(|c| {
                c == "info_player_start" || (c.starts_with("mp_") && c.contains("_spawn"))
            })
        })
        .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 - 1 - 2 in a row with a shortcut 0 -> 3 -> 2 that is longer, and a
    /// one-way drop 2 -> 4.
    fn tiny() -> NavGraph {
        NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(32.0, 64.0, 0.0),
                Vec3::new(96.0, 0.0, -100.0),
            ],
            vec![vec![1, 3], vec![0, 2], vec![1, 4], vec![0, 2], vec![]],
        )
    }

    #[test]
    fn a_star_takes_the_shorter_way() {
        assert_eq!(tiny().path(0, 2), Some(vec![0, 1, 2]));
        assert_eq!(tiny().path(2, 2), Some(vec![2]));
    }

    #[test]
    fn a_one_way_drop_has_no_way_back() {
        let g = tiny();
        assert_eq!(g.path(0, 4), Some(vec![0, 1, 2, 4]));
        assert_eq!(g.path(4, 0), None);
    }

    #[test]
    fn a_drop_splits_the_components() {
        let c = tiny().components();
        assert!(c[0] == c[1] && c[1] == c[2] && c[2] == c[3]);
        assert_ne!(c[4], c[0], "the node below the drop is its own component");
    }

    /// Six nodes in a two-way row along +x, 32 apart.
    fn row() -> NavGraph {
        let nodes = (0..6)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let edges = (0..6u32)
            .map(|i| {
                [i.wrapping_sub(1), i + 1]
                    .into_iter()
                    .filter(|&n| n < 6)
                    .collect()
            })
            .collect();
        NavGraph::from_parts(nodes, edges)
    }

    #[test]
    fn a_follower_hands_out_the_path_then_the_point() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let goal = Goal::To([170.0, 0.0, 40.0]);
        let mut budget = 100;
        let mut rand = || 0;
        let mut at = [0.0, 0.0, 0.0];
        let mut seen = Vec::new();
        let mut planned = None;
        for _ in 0..10 {
            let Some(w) = f.waypoint(&g, None, goal, at, true, &mut budget, &mut rand) else {
                panic!("no waypoint at {at:?}");
            };
            seen.push(w[0]);
            planned.get_or_insert(budget);
            if w == [170.0, 0.0, 40.0] {
                break;
            }
            at = w;
        }
        assert_eq!(seen, [32.0, 64.0, 96.0, 128.0, 160.0, 170.0]);
        assert_eq!(Some(budget), planned, "one plan for the whole walk");
    }

    /// A `side` x `side` two-way grid 32 apart, and one node 1000 units off
    /// it with no edges: an island no plan reaches.
    fn grid_and_island(side: u32) -> NavGraph {
        let mut nodes: Vec<Vec3> = (0..side * side)
            .map(|i| Vec3::new((i % side) as f32 * 32.0, (i / side) as f32 * 32.0, 0.0))
            .collect();
        let mut edges: Vec<Vec<u32>> = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let mut out = Vec::new();
                if x > 0 {
                    out.push(i - 1);
                }
                if x + 1 < side {
                    out.push(i + 1);
                }
                if y > 0 {
                    out.push(i - side);
                }
                if y + 1 < side {
                    out.push(i + side);
                }
                out
            })
            .collect();
        nodes.push(Vec3::new(-1000.0, -1000.0, 0.0));
        edges.push(Vec::new());
        NavGraph::from_parts(nodes, edges)
    }

    /// The island is ruled out by the component table, not by a search of
    /// all 40000 nodes: the plan heads for the grid's corner nearest it at
    /// the cost of the walk there, a handful of expansions.
    #[test]
    fn an_unreachable_goal_fails_without_a_search() {
        let g = grid_and_island(200);
        let island = g.nodes.len() as u32 - 1;
        assert!(!g.reaches(0, island) && !g.reaches(island, 0));
        assert_eq!(g.path(5, island), None);
        let mut f = Follower::default();
        let mut budget = 4000;
        let at = [64.0, 64.0, 0.0];
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::To([-1000.0, -1000.0, 0.0]),
            at,
            true,
            &mut budget,
            &mut || 0,
        );
        assert!(w.is_some(), "heads for the corner");
        assert!(4000 - budget < 10, "spent {}", 4000 - budget);
        assert_eq!(f.path.last(), Some(&0), "ends at the corner");
        // From the island nothing is reached: no search and no waypoint.
        let mut f = Follower::default();
        let mut budget = 4000;
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::To([0.0; 3]),
            [-1000.0, -1000.0, 0.0],
            true,
            &mut budget,
            &mut || 0,
        );
        assert_eq!((w, budget), (None, 4000));
    }

    /// A plan longer than one tick's slice resumes where it stopped: no
    /// tick spends more than [`PLAN_SLICE`], and the path still arrives.
    #[test]
    fn a_long_plan_spreads_over_ticks() {
        // A serpentine: rows joined at alternate ends, so A*'s straight-line
        // guess is wrong nearly everywhere.
        let side = 120u32;
        let nodes = (0..side * side)
            .map(|i| Vec3::new((i % side) as f32 * 32.0, (i / side) as f32 * 32.0, 0.0))
            .collect();
        let edges = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side, i / side);
                let mut out = Vec::new();
                if x > 0 {
                    out.push(i - 1);
                }
                if x + 1 < side {
                    out.push(i + 1);
                }
                let open = if y % 2 == 0 { side - 1 } else { 0 };
                if x == open && y + 1 < side {
                    out.push(i + side);
                }
                let below = if y % 2 == 1 { side - 1 } else { 0 };
                if x == below && y > 0 {
                    out.push(i - side);
                }
                out
            })
            .collect();
        let g = NavGraph::from_parts(nodes, edges);
        let goal = crate::bots::Goal::To([0.0, (side - 1) as f32 * 32.0, 0.0]);
        let mut f = Follower::default();
        let mut ticks = 0;
        let w = loop {
            let mut budget = 4000;
            let w = f.waypoint(&g, None, goal, [0.0; 3], true, &mut budget, &mut || 0);
            assert!(
                4000 - budget <= PLAN_SLICE,
                "tick {ticks} spent {}",
                4000 - budget
            );
            ticks += 1;
            if w.is_some() || ticks > 100 {
                break w;
            }
        };
        assert!(ticks > 1, "one tick held the whole plan");
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert_eq!(f.path.len() as u32, side * side);
    }

    #[test]
    fn a_spent_plan_budget_leaves_the_bot_without_a_waypoint() {
        let g = row();
        let mut f = Follower::default();
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::Roam,
            [0.0; 3],
            true,
            &mut 0,
            &mut || 0,
        );
        assert_eq!(w, None);
    }

    #[test]
    fn a_roam_heads_for_a_far_node() {
        let g = row();
        let mut f = Follower::default();
        let mut picks = [1, 5].into_iter();
        let mut rand = || picks.next().unwrap_or(0);
        let w = f.waypoint(
            &g,
            None,
            crate::bots::Goal::Roam,
            [0.0; 3],
            true,
            &mut 100,
            &mut rand,
        );
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        // Nothing is ROAM_MIN away on a 160-unit row: the farthest pick wins.
        assert_eq!(f.dest, Some(5));
    }

    /// Away from a threat at one end of the row, the farthest pick that is
    /// nearer the bot than the threat wins.
    #[test]
    fn a_flight_heads_away_from_the_threat() {
        let g = row();
        let mut f = Follower::default();
        let mut picks = [0, 4, 1, 5, 3].into_iter().cycle();
        let mut rand = || picks.next().unwrap();
        let goal = crate::bots::Goal::Away([0.0, 0.0, 0.0]);
        let w = f.waypoint(&g, None, goal, [64.0, 0.0, 0.0], true, &mut 100, &mut rand);
        assert_eq!(f.dest, Some(5));
        assert_eq!(w, Some([96.0, 0.0, 0.0]));
        // The threat moves past the far end: the flight turns round.
        let goal = crate::bots::Goal::Away([300.0, 0.0, 0.0]);
        f.waypoint(&g, None, goal, [64.0, 0.0, 0.0], true, &mut 100, &mut rand);
        assert_eq!(f.dest, Some(0));
    }

    /// A bot pinned on its way from node 1 to node 2 (by a body guarding the
    /// stairs, say) plans round that edge, through node 3, once it gives up.
    #[test]
    fn a_follower_stuck_on_an_edge_plans_round_it() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(60.0, 30.0, 0.0),
            ],
            vec![vec![1], vec![0, 2, 3], vec![1, 3], vec![1, 2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([64.0, 0.0, 0.0]);
        let mut budget = 1000;
        let at = [32.0, 0.0, 0.0];
        let mut last = None;
        for _ in 0..=STUCK_TICKS + 1 {
            last = f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0);
        }
        assert_eq!(last, None, "still steering at a waypoint it never nears");
        let w = f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0);
        assert_eq!(
            w,
            Some([60.0, 30.0, 0.0]),
            "planned through the blocked edge"
        );
    }

    /// West of mp_carentan's boundary wall the map's base brush runs on
    /// under the terrain; a flood seeded there reached the town at this
    /// point, with the terrain sheet under 72 units over the feet.
    #[test]
    fn carentans_floor_under_a_low_terrain_sheet_is_under_ground() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_carentan").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let p = Vec3::new(389.0, 575.0, -46.7);
        assert!(under_ground(w, p));
    }

    /// mp_carentan's graph from the real collision: every spawn point sits on
    /// it, and nearly all of them reach each other both ways.
    #[test]
    fn carentans_spawns_reach_each_other() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let entry = fs
            .resolve_map("mp_carentan")
            .expect("mp_carentan in the paks");
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let g = graph_for("mp_carentan", &world);
        let on: Vec<u32> = world
            .spawn_points
            .iter()
            .filter_map(|s| g.nearest(*s))
            .collect();
        assert_eq!(on.len(), world.spawn_points.len(), "a spawn off the graph");
        let comp = g.components();
        let mut count = std::collections::BTreeMap::new();
        for n in &on {
            *count.entry(comp[*n as usize]).or_insert(0) += 1;
        }
        let most = count.values().max().copied().unwrap_or(0);
        assert!(
            most * 100 >= on.len() * 95,
            "only {most} of {} spawns in one component",
            on.len()
        );
    }

    /// A ladder whose head is right above its foot: the follower keeps the
    /// head as its waypoint up the climb, though the climb never closes in
    /// flat, and does not skip it for the next node from a floor below.
    /// A goal the graph does not reach is closed in on as far as it goes.
    #[test]
    fn a_path_toward_an_island_ends_at_the_node_nearest_it() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(200.0, 0.0, 0.0),
            ],
            vec![vec![1], vec![0, 2], vec![1], vec![]],
        );
        assert_eq!(g.path(0, 3), None);
        assert_eq!(g.path_toward(0, 3), Some(vec![0, 1, 2]));
        assert_eq!(g.path_toward(3, 0), None, "nowhere to go from the island");
    }

    /// Walking at a point past the path's end can drop the body a floor;
    /// from there the point is planned for again, not walked at blind.
    #[test]
    fn a_follower_that_falls_off_its_point_plans_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(32.0, 0.0, 100.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
            ],
            vec![vec![1], vec![0, 2], vec![3, 1], vec![2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([40.0, 0.0, 120.0]);
        let mut budget = 1000;
        // Standing on the path's last node: walked out, head at the point.
        let at = [32.0, 0.0, 100.0];
        assert_eq!(
            f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0),
            Some([40.0, 0.0, 120.0])
        );
        assert_eq!(
            f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0),
            Some([40.0, 0.0, 120.0])
        );
        // Fell to the floor below: a new plan back up, its waypoints nodes.
        let below = [34.0, 0.0, 0.0];
        let before = budget;
        let w = f.waypoint(&g, None, goal, below, true, &mut budget, &mut || 0);
        assert!(budget < before, "planned again");
        assert!(
            w.is_some_and(|w| g.nodes.contains(&Vec3::from(w))),
            "still walking at the point: {w:?}"
        );
    }

    /// Floor, a ledge 16 up, a top 32 up, the floor beyond: from the ledge,
    /// beside the top and in reach of it flat, the bot still heads for the
    /// top, where the drop on was proved.
    #[test]
    fn a_node_a_drop_leaves_from_is_passed_only_on_it() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(64.0, 0.0, -64.0),
                Vec3::new(32.0, 0.0, -48.0),
                Vec3::new(0.0, 0.0, -32.0),
                Vec3::new(-48.0, 0.0, -64.0),
            ],
            vec![vec![1], vec![0, 2], vec![1, 3], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([-48.0, 0.0, -64.0]);
        let mut budget = 1000;
        let mut at = |p: [f32; 3]| f.waypoint(&g, None, goal, p, false, &mut budget, &mut || 0);
        assert_eq!(at([64.0, 0.0, -64.0]), Some([32.0, 0.0, -48.0]));
        assert_eq!(at([12.0, 0.0, -48.0]), Some([0.0, 0.0, -32.0]));
        assert_eq!(at([2.0, 0.0, -32.0]), Some([-48.0, 0.0, -64.0]));
    }

    #[test]
    fn a_follower_climbs_to_a_head_above_its_foot() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 200.0),
                Vec3::new(40.0, 0.0, 200.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([40.0, 0.0, 200.0]);
        let mut budget = 100;
        let mut at = [0.0, 0.0, 0.0];
        // 2.5 units a tick, the climb rate looking up, until the head's
        // floor is in reach.
        while at[2] <= 152.0 {
            let w = f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0);
            assert_eq!(w, Some([10.0, 0.0, 200.0]), "at z {}", at[2]);
            at[2] += 2.5;
        }
        at[0] = 10.0;
        let w = f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0);
        assert_eq!(w, Some([40.0, 0.0, 200.0]), "on the head's floor");
    }

    #[test]
    fn a_creep_holds_its_first_heading() {
        let mut c = Creep::default();
        assert_eq!(back_move(100.0, false), -127);
        assert_eq!(c.hold(-127, 10.0), 10.0);
        assert_eq!(back_move(20.0, false), -CREEP);
        assert_eq!(c.hold(-CREEP, 20.0), 20.0);
        assert_eq!(c.hold(-CREEP, 200.0), 20.0, "turned round over the lip");
        assert_eq!(back_move(20.0, true), -127, "full speed down the ladder");
        assert_eq!(c.hold(-127, 30.0), 30.0);
        assert_eq!(c.hold(-CREEP, 40.0), 40.0, "a new creep, a new heading");
    }

    /// mp_ship's ladders, each from its open face: up and down both proved
    /// by the bots' own steering. One plain, one whose head stands right
    /// above its foot in a shaft, one only the creep gets down, and the five
    /// that got no rung before 2026-10-07.
    #[test]
    fn ships_ladders_are_climbed_both_ways() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let Some(entry) = fs.resolve_map("mp_ship") else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let boxes = ladders(&world.collision);
        for (at, n, foot_z, head_z) in [
            ([5872.5, 47.0], Vec3::X, 352.125, 480.125),
            ([3711.5, 56.0], -Vec3::X, 760.125, 992.125),
            ([3792.5, 65.0], Vec3::X, 304.125, 616.125),
            // The hold's 560-unit ladder, the mast's under a spar, a crow's
            // nest: climbs past the old 10 s budget, or under a patch.
            ([4332.0, -77.5], -Vec3::Y, 56.125, 615.125),
            ([3675.0, -86.5], -Vec3::Y, 896.125, 1145.125),
            ([4975.5, 64.0], -Vec3::X, 692.125, 1219.125),
            // A brush running on below the floor, a wall crowding one edge.
            ([6487.0, 324.5], Vec3::Y, 80.125, 216.125),
            ([3600.5, -119.0], Vec3::X, 760.125, 896.125),
        ] {
            let &(lo, hi) = boxes
                .iter()
                .find(|(lo, hi)| (0..2).all(|i| lo[i] <= at[i] && at[i] <= hi[i]))
                .unwrap_or_else(|| panic!("no ladder at {at:?}"));
            let (foot, head, up, down) =
                rung(&world.collision, lo, hi, n).unwrap_or_else(|| panic!("{at:?}: no rung"));
            assert!((foot.z - foot_z).abs() < 1.0, "{at:?}: foot {foot}");
            assert!((head.z - head_z).abs() < 1.0, "{at:?}: head {head}");
            assert!(up && down, "{at:?}: up {up} down {down}");
            assert!(
                rung(&world.collision, lo, hi, -n).is_none(),
                "{at:?}: the back"
            );
        }
    }

    /// mp_hurtgen's minefields ring the axis spawn: a bot that wandered off
    /// it died about every 12 s and ended the Retrieval round.
    #[test]
    fn hurtgens_minefields_are_hazards() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let Some(entry) = fs.resolve_map("mp_hurtgen") else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        assert!(
            hazard(&world.hazards, Vec3::new(6198.0, -302.0, 188.0)),
            "where the bot died"
        );
        assert!(
            !hazard(&world.hazards, Vec3::new(6304.0, 296.0, 96.125)),
            "the spawn beside it"
        );
    }

    /// A jump edge stays a jump until a walk proves it without one, and a
    /// jump one way is no proof of a plain walk the other.
    #[test]
    fn a_plain_walk_clears_a_jump_edge() {
        let mut g = row();
        g.link_as(0, 2, true);
        assert!(g.jumps.contains(&(0, 2)));
        g.link_as(0, 2, true);
        assert!(g.jumps.contains(&(0, 2)), "a second jump keeps it a jump");
        g.link_as(0, 2, false);
        assert!(!g.jumps.contains(&(0, 2)));
        g.link_as(2, 4, false);
        g.link_as(2, 4, true);
        assert!(!g.jumps.contains(&(2, 4)), "a plain edge stays plain");
    }

    #[test]
    fn a_leap_jumps_only_at_the_lip() {
        // flat, rise: the top 40 off and 32 up.
        assert!(!jump_cue(true, false, 0.0, 40.0, 32.0), "pinned is no cue");
        assert!(jump_cue(true, true, 9.5, 40.0, 32.0));
        // A jump onto a ledge: pinned at its face, or right under the top.
        assert!(jump_cue(false, false, 0.5, 30.0, 32.0));
        assert!(
            !jump_cue(false, false, 9.5, 30.0, 32.0),
            "still running at it"
        );
        assert!(jump_cue(false, false, 9.5, 10.0, 32.0));
        assert!(!jump_cue(false, false, 9.5, 10.0, 10.0), "under a step");
        // On the top: no more jumping.
        assert!(!jump_cue(false, false, 0.0, 10.0, 2.0));
        assert!(!jump_cue(true, true, 0.0, 10.0, 2.0));
        assert_eq!(jump_key(true, true, false), 127);
        assert_eq!(jump_key(true, true, true), 0, "a held key is refused");
        assert_eq!(jump_key(false, true, false), 0);
    }

    /// A leap's foot is held until the bot stands still on it, then the
    /// top is the waypoint, flagged a leap.
    #[test]
    fn a_follower_comes_to_rest_at_a_leaps_foot() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(96.0, 0.0, 32.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.jumps.insert((1, 2));
        g.leaps.insert((1, 2));
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([96.0, 0.0, 32.0]);
        let mut budget = 100;
        let foot = Some([32.0, 0.0, 0.0]);
        let at = [30.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(&g, None, goal, at, false, &mut budget, &mut || 0),
            foot
        );
        assert!(f.holding(&g) && !f.jumping(&g));
        // Running past the foot, nearer the top than the foot is: held.
        let at = [40.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(&g, None, goal, at, false, &mut budget, &mut || 0),
            foot
        );
        let at = [33.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(&g, None, goal, at, true, &mut budget, &mut || 0),
            Some([96.0, 0.0, 32.0])
        );
        assert!(f.jumping(&g) && f.leaping(&g) && !f.holding(&g));
    }

    /// A roam picks among the nodes its start reaches both ways: two rows
    /// with no edge between them, and a pick that would land in the other.
    #[test]
    fn a_roam_stays_in_its_component() {
        let nodes = (0..6)
            .map(|i| Vec3::new(i as f32 * 32.0, (i / 3) as f32 * 512.0, 0.0))
            .collect();
        let edges = vec![vec![1], vec![0, 2], vec![1], vec![4], vec![3, 5], vec![4]];
        let g = NavGraph::from_parts(nodes, edges);
        let mut picks = [4, 5].into_iter();
        let mut rand = || picks.next().unwrap_or(0);
        let d = g.roam_from(0, Vec3::ZERO, &mut rand);
        assert!(d.is_some_and(|d| d < 3), "picked {d:?}");
    }

    fn map_world(map: &str) -> Option<crate::world::World> {
        let fs = vcod_common::testing::game_fs()?;
        let entry = fs.resolve_map(map)?;
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        Some(crate::world::World::from_bsp(&bsp, Some(&fs)))
    }

    /// mp_depot's documents lie on a crate top at z 148 across a gap from a
    /// step at 116: a run off the step falls, a standing-start leap from it
    /// lands on the crate.
    #[test]
    fn depots_crate_takes_a_leap() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let step = settle(w, Vec3::new(-1820.0, -609.0, 120.0), 0.0).expect("the step");
        assert!((step.z - 116.125).abs() < 0.5, "step at {step}");
        let top = glam::Vec2::new(-1888.0, -537.0);
        assert!(matches!(
            walk_as(w, step, top, Some(148.125), Gait::Forward),
            Walked::Fell
        ));
        assert!(matches!(
            walk_as(w, step, top, Some(148.125), Gait::Leap),
            Walked::Arrived(p, true) if (p.z - 148.125).abs() < 0.5
        ));
    }

    /// mp_ship: a run down the steep stair to the hull's deck at z 104 is in
    /// the air past its run time (the walk lets it land), and the spawns
    /// in a lifeboat at z 394 get out over its 40-unit gunwale only with a
    /// jump.
    #[test]
    fn ships_hull_and_lifeboat_walks() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let stair = settle(w, Vec3::new(3430.0, 288.0, 170.0), 0.0).expect("the stair");
        let boxes = ladders(w);
        let down = walk(w, &boxes, stair, glam::Vec2::new(3456.0, 288.0));
        assert!(
            down.is_some_and(|(p, jumped)| p.z < 110.0 && !jumped),
            "down the stair: {down:?}"
        );
        let boat = settle(w, Vec3::new(4224.0, -376.0, 440.0), 0.0).expect("the gunwale");
        let out = walk(w, &boxes, boat, glam::Vec2::new(4192.0, -384.0));
        assert!(
            out.is_some_and(|(p, jumped)| p.z < 360.0 && jumped),
            "out of the boat: {out:?}"
        );
    }

    /// mp_ship's hull beam: a body stands on its side's edge 16 under the
    /// top. A walk from there aimed 8 units beside a top node comes to rest
    /// on that edge again, which is not the node's floor.
    #[test]
    fn a_walk_back_onto_ships_hull_beam_arrives_only_on_top() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let edge = Vec3::new(2208.5088, 444.68066, -47.875);
        let top = Vec3::new(2172.9297, 418.1561, -31.875);
        let aside = top.truncate() + (top - edge).truncate().perp().normalize() * SIDESTEP;
        let walked = walk_as(w, edge, aside, Some(top.z), Gait::Forward);
        assert!(!matches!(walked, Walked::Arrived(..)));
        // The way it went: onto the edge at the beam's foot.
        let free = walk_as(w, edge, aside, None, Gait::Forward);
        assert!(matches!(free, Walked::Arrived(p, false) if (p.z - edge.z).abs() < 0.5));
    }

    #[test]
    fn nearest_prefers_the_floor_under_the_feet() {
        let mut g = tiny();
        // A node straight above node 1, one storey up.
        g = NavGraph::from_parts(
            [g.nodes.clone(), vec![Vec3::new(32.0, 0.0, 128.0)]].concat(),
            [g.edges.clone(), vec![vec![]]].concat(),
        );
        assert_eq!(g.nearest([30.0, 2.0, 10.0]), Some(1));
        assert_eq!(g.nearest([30.0, 2.0, 140.0]), Some(5));
        assert_eq!(g.nearest([5000.0, 0.0, 0.0]), None);
    }
}
