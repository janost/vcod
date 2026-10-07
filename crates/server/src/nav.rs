//! The bots' navigation graph: a grid flood-filled out of the map's spawn
//! points, each edge proven by running pmove along it, and A* over it.
//! Design and build times: `docs/research/bot-navigation.md`.

use crate::game::trigger::{BrushHull, box_contacts_hulls};
use glam::Vec3;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
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
const ARRIVE: f32 = 8.0;
/// A walk that drops further than this is refused: past
/// `bg_fallDamageMinHeight` (256) the drop hurts.
const MAX_DROP: f32 = 200.0;
/// An edge whose ends differ by less than this in height is level, and a
/// walk one way stands in for the walk back.
const LEVEL: f32 = 1.0;
/// The flood stops here; a stock map fills well under it.
const MAX_NODES: usize = 60_000;
/// One bot tick, the cmd length every walk is stepped in.
const TICK_MS: i32 = 50;
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
    /// The grid pitch the graph was built on.
    pub spacing: f32,
    /// Nodes by grid column, for lookups only; nothing iterates it.
    columns: HashMap<(i32, i32), Vec<u32>>,
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
            spacing: spacing.clamp(SPACING_MIN, SPACING_MAX),
            columns: HashMap::new(),
        };
        let mut queue = Vec::new();
        for seed in seeds {
            let Some(p) = settle(world, Vec3::from(*seed), 0.0) else {
                continue;
            };
            // On the lattice when the seed can walk to its own column's
            // centre, so every later walk targets a node's real spot.
            let c = g.column(p);
            let p = walk(world, p, g.center(c)).unwrap_or(p);
            if g.find(c, p.z).is_none() {
                queue.push(g.add(c, p));
            }
        }
        // Breadth-first by layers. A layer's walks depend only on where its
        // nodes stand, so they run on every core, and the merge takes them in
        // job order, which keeps the graph identical to a serial flood
        // whatever the thread count. A layer's diagonals run two layers late,
        // once the square's other three corners have walked their sides.
        let mut frontier = queue;
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
                let (cx, cy) = g.column(g.nodes[n as usize]);
                dirs.map(|(dx, dy)| g.step(world, n, (cx + dx, cy + dy)))
            });
            let mut next = Vec::new();
            for (&(n, dirs), ends) in jobs.iter().zip(ends) {
                let (cx, cy) = g.column(g.nodes[n as usize]);
                for (&(dx, dy), to) in dirs.iter().zip(ends) {
                    let Some(to) = to else {
                        continue;
                    };
                    let c = (cx + dx, cy + dy);
                    let m = match g.find(c, to.z) {
                        Some(m) => m,
                        None if g.nodes.len() < MAX_NODES && inside(to) && !hazard(hazards, to) => {
                            let m = g.add(c, to);
                            next.push(m);
                            m
                        }
                        None => continue,
                    };
                    g.link(n, m);
                }
            }
            if !frontier.is_empty() {
                late.push_back(frontier);
            }
            frontier = next;
        }
        g.link_ladders(world, hazards);
        g.link_back(world);
        g
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
        let ok = par_map(&jobs, |&(a, b)| {
            let to = self.nodes[b as usize];
            let walked = walk_as(
                world,
                self.nodes[a as usize],
                to.truncate(),
                Some(to.z),
                Gait::Forward,
            );
            matches!(walked, Walked::Arrived(_))
        });
        for (&(a, b), ok) in jobs.iter().zip(ok) {
            if ok {
                self.link(a, b);
            }
        }
    }

    /// The flood walks only to a neighbouring column, which rarely lines up
    /// with a ladder. So each ladder face gets two nodes of its own: a foot
    /// in front of the face and a head where a climb up it comes to rest,
    /// linked where the bots' own steering proves the way, and each linked
    /// to the graph around it (bot-navigation.md, "Ladders").
    fn link_ladders(&mut self, world: &CollisionWorld, hazards: &[BrushHull]) {
        let faces: Vec<(Vec3, Vec3, Vec3)> = ladders(world)
            .into_iter()
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
        let ok = par_map(&jobs, |&(a, b)| {
            let to = self.nodes[b as usize];
            let walked = walk_as(
                world,
                self.nodes[a as usize],
                to.truncate(),
                Some(to.z),
                Gait::Forward,
            );
            matches!(walked, Walked::Arrived(_))
        });
        for (&(a, b), ok) in jobs.iter().zip(ok) {
            if ok {
                self.link(a, b);
            }
        }
    }

    /// Where a run from node `n` toward column `c`'s centre comes to rest,
    /// skipping the walk when the graph already proves the answer: a level
    /// cardinal edge already walked the other way (half a flood's walks), or
    /// a level diagonal across a square whose four sides run both ways.
    fn step(&self, world: &CollisionWorld, n: u32, c: (i32, i32)) -> Option<Vec3> {
        let from = self.nodes[n as usize];
        let (cx, cy) = self.column(from);
        let level = |m: u32| (self.nodes[m as usize].z - from.z).abs() < LEVEL;
        let has = |a: u32, b: u32| self.edges[a as usize].contains(&b);
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
                return Some(self.nodes[d as usize]);
            }
        }
        walk(world, from, self.center(c))
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
        if a != b && !self.edges[a as usize].contains(&b) {
            self.edges[a as usize].push(b);
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
        self.astar(from, to, avoid, false)
    }

    /// [`Self::path`], or when `to` is out of reach, the path to the
    /// reachable node nearest it: a goal the graph does not reach (an
    /// objective down a bunker's stairs, on a pitch too coarse for them)
    /// is still closed in on along the graph, not wandered at.
    pub fn path_toward(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        self.astar(from, to, &[], true)
    }

    fn astar(&self, from: u32, to: u32, avoid: &[(u32, u32)], partial: bool) -> Option<Vec<u32>> {
        let n = self.nodes.len();
        if from as usize >= n || to as usize >= n {
            return None;
        }
        let goal = self.nodes[to as usize];
        let h = |i: u32| self.nodes[i as usize].distance(goal);
        let mut cost = vec![f32::INFINITY; n];
        let mut came = vec![u32::MAX; n];
        let mut open = BinaryHeap::new();
        cost[from as usize] = 0.0;
        open.push(Open {
            f: h(from),
            node: from,
        });
        let mut expanded = 0;
        let back = |came: &[u32], to: u32| {
            let mut out = vec![to];
            let mut at = to;
            while at != from {
                at = came[at as usize];
                out.push(at);
            }
            out.reverse();
            out
        };
        // The expanded node nearest the goal, for a partial plan.
        let mut closest = (h(from), from);
        while let Some(Open { f, node }) = open.pop() {
            if node == to {
                return Some(back(&came, to));
            }
            // A stale heap entry: the node was reached cheaper since.
            if f > cost[node as usize] + h(node) + 1e-3 {
                continue;
            }
            expanded += 1;
            if expanded > MAX_EXPANSIONS {
                return None;
            }
            if h(node) < closest.0 {
                closest = (h(node), node);
            }
            let here = self.nodes[node as usize];
            for &next in &self.edges[node as usize] {
                if avoid.contains(&(node, next)) {
                    continue;
                }
                let c = cost[node as usize] + here.distance(self.nodes[next as usize]);
                if c < cost[next as usize] {
                    cost[next as usize] = c;
                    came[next as usize] = node;
                    open.push(Open {
                        f: c + h(next),
                        node: next,
                    });
                }
            }
        }
        (partial && closest.1 != from).then(|| back(&came, closest.1))
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
            spacing: SPACING_MIN,
            columns: HashMap::new(),
        };
        for p in nodes {
            let c = g.column(p);
            g.columns.entry(c).or_default().push(g.nodes.len() as u32);
            g.nodes.push(p);
        }
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
/// How long the edge a follower got stuck on stays out of its plans. What
/// blocks a proven edge is mostly a body: a bot guarding the bomb at the
/// foot of mp_rocket's stairs held another there for 11 s.
const AVOID_TICKS: u32 = 200;
/// How far below where its path ended a body walking at a point goal has
/// to drop before the follower plans for the point again: more than a stair
/// or a step off a kerb.
const ARRIVED_DROP: f32 = 48.0;
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
}

impl Follower {
    /// The waypoint toward `goal` for a bot standing at `at`. `plans` is the
    /// tick's shared A* budget; a bot that needs a plan when it is spent gets
    /// none this tick. `rand` picks roam destinations.
    pub fn waypoint(
        &mut self,
        g: &NavGraph,
        goal: crate::bots::Goal,
        at: [f32; 3],
        plans: &mut u32,
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
            if *plans == 0 {
                return None;
            }
            *plans -= 1;
            let start = g.nearest(at)?;
            if goal == Goal::Roam && self.dest.is_none() {
                self.dest = g.roam_from(here, rand);
            }
            // Round the edges it got stuck on when there is a way round, else
            // through them again: whatever blocked one may have moved.
            let avoid: Vec<(u32, u32)> = self.avoid.iter().map(|e| (e.0, e.1)).collect();
            // A point out of the graph's reach is closed in on as far as the
            // graph goes, then walked at.
            let point = matches!(goal, Goal::To(_));
            let plan = |d| {
                g.path_avoiding(start, d, &avoid)
                    .or_else(|| g.path(start, d))
                    .or_else(|| point.then(|| g.path_toward(start, d)).flatten())
            };
            let Some(path) = self.dest.and_then(plan) else {
                // Unreachable from here: a roam picks again next time.
                self.reset();
                self.rest = REST_TICKS;
                return None;
            };
            self.path = path;
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
            let w = g.nodes[w as usize];
            let level = (w.z - here.z).abs() < 48.0;
            let past = self.path.get(self.next + 1).is_some_and(|&n| {
                let n = g.nodes[n as usize];
                flat(n) < (n - w).truncate().length()
            });
            if level && (flat(w) < REACH || past) {
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

    fn reset(&mut self) {
        self.dest = None;
        self.path.clear();
        self.arrived = false;
    }
}

impl NavGraph {
    /// A roam destination: of a handful of random nodes, the first at least
    /// [`ROAM_MIN`] from `from`, else the farthest.
    fn roam_from(&self, from: Vec3, rand: &mut dyn FnMut() -> i32) -> Option<u32> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(f32, u32)> = None;
        for _ in 0..8 {
            let n = (rand() as usize % self.nodes.len()) as u32;
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
            Walked::Arrived(_)
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
    Backward,
    Creep,
}

/// What a walk came to.
enum Walked {
    Arrived(Vec3),
    Blocked,
    /// It left the ground and never arrived; a ladder below may still take it.
    Fell,
}

/// [`walk_as`] forward, backward when the forward run fell, and creeping
/// when that fell too. The creep is last because it is slow: a back down
/// off every ledge on a map is most of a build, and only a ladder below
/// needs it.
fn walk(world: &CollisionWorld, from: Vec3, target: glam::Vec2) -> Option<Vec3> {
    for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
        match walk_as(world, from, target, None, gait) {
            Walked::Arrived(p) => return Some(p),
            Walked::Blocked => return None,
            Walked::Fell => {}
        }
    }
    None
}

/// Runs a body from `from` toward `target`, re-aiming every tick, and
/// reports where it came to rest when it got within [`ARRIVE`] on the
/// ground (on the floor at `floor`, when given) without falling past
/// [`MAX_DROP`].
fn walk_as(
    world: &CollisionWorld,
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    let dist = target.distance(from.truncate());
    let on_floor = |z: f32| floor.is_none_or(|f| (z - f).abs() < Z_MERGE);
    if dist < ARRIVE && on_floor(from.z) {
        return Walked::Arrived(from);
    }
    // A running start: a following bot never stops at a node, and the
    // spin-up from a standstill was a third of every walk.
    let mut ps = PlayerState::spawn(from, 0.0);
    let dir = (target - from.truncate()) / dist;
    ps.velocity = (dir * vcod_common::pmove::SPEED_RUN).extend(0.0);
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
    while tick < budget {
        tick += 1;
        let before = ps.origin;
        let to = target - ps.origin.truncate();
        let (yaw, forward) = match gait {
            Gait::Forward => (to.y.atan2(to.x).to_degrees(), 127),
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
        let cmd = UserCmd {
            forward,
            angles: [(pitch * ANGLE2SHORT) as i32, (yaw * ANGLE2SHORT) as i32, 0],
            ..NULL_USERCMD
        };
        sim.tick(world, &mut ps, &cmd);
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
            return Walked::Arrived(ps.origin);
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
        if moved < least && !(climbed && !ps.on_ground && !ps.on_ladder) {
            stalled += 1;
            if stalled == 2 {
                break;
            }
        } else {
            stalled = 0;
        }
    }
    if fell { Walked::Fell } else { Walked::Blocked }
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
/// mp_pavlov kept 540 nodes behind it off the graph).
fn under_ground(world: &CollisionWorld, p: Vec3) -> bool {
    let head = p + Vec3::Z * 72.0;
    let up = world.point_trace(head, head + Vec3::Z * 8192.0, MASK_PLAYERSOLID, false);
    if let Some(Prim::Tri(t)) = up.hit {
        let [a, b, c] = world.tris[t as usize];
        if (b - a).cross(c - a).normalize_or_zero().z > 0.7 {
            return true;
        }
    }
    let down = world.point_trace(up.endpos - Vec3::Z, head, MASK_PLAYERSOLID, false);
    matches!(down.hit, Some(Prim::Tri(t)) if world.is_terrain(t))
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
            server_time: self.time + TICK_MS,
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
        let mut plans = 1;
        let mut rand = || 0;
        let mut at = [0.0, 0.0, 0.0];
        let mut seen = Vec::new();
        for _ in 0..10 {
            let Some(w) = f.waypoint(&g, goal, at, &mut plans, &mut rand) else {
                panic!("no waypoint at {at:?}");
            };
            seen.push(w[0]);
            if w == [170.0, 0.0, 40.0] {
                break;
            }
            at = w;
        }
        assert_eq!(seen, [32.0, 64.0, 96.0, 128.0, 160.0, 170.0]);
        assert_eq!(plans, 0, "one plan for the whole walk");
    }

    #[test]
    fn a_spent_plan_budget_leaves_the_bot_without_a_waypoint() {
        let g = row();
        let mut f = Follower::default();
        let w = f.waypoint(&g, crate::bots::Goal::Roam, [0.0; 3], &mut 0, &mut || 0);
        assert_eq!(w, None);
    }

    #[test]
    fn a_roam_heads_for_a_far_node() {
        let g = row();
        let mut f = Follower::default();
        let mut picks = [1, 5].into_iter();
        let mut rand = || picks.next().unwrap_or(0);
        let w = f.waypoint(&g, crate::bots::Goal::Roam, [0.0; 3], &mut 1, &mut rand);
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        // Nothing is ROAM_MIN away on a 160-unit row: the farthest pick wins.
        assert_eq!(f.dest, Some(5));
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
        let mut plans = 2;
        let at = [32.0, 0.0, 0.0];
        let mut last = None;
        for _ in 0..=STUCK_TICKS + 1 {
            last = f.waypoint(&g, goal, at, &mut plans, &mut || 0);
        }
        assert_eq!(last, None, "still steering at a waypoint it never nears");
        let w = f.waypoint(&g, goal, at, &mut plans, &mut || 0);
        assert_eq!(
            w,
            Some([60.0, 30.0, 0.0]),
            "planned through the blocked edge"
        );
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
        let mut plans = 4;
        // Standing on the path's last node: walked out, head at the point.
        let at = [32.0, 0.0, 100.0];
        assert_eq!(
            f.waypoint(&g, goal, at, &mut plans, &mut || 0),
            Some([40.0, 0.0, 120.0])
        );
        assert_eq!(
            f.waypoint(&g, goal, at, &mut plans, &mut || 0),
            Some([40.0, 0.0, 120.0])
        );
        // Fell to the floor below: a new plan back up, its waypoints nodes.
        let below = [34.0, 0.0, 0.0];
        let w = f.waypoint(&g, goal, below, &mut plans, &mut || 0);
        assert_eq!(plans, 2, "planned again");
        assert!(
            w.is_some_and(|w| g.nodes.contains(&Vec3::from(w))),
            "still walking at the point: {w:?}"
        );
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
        let mut plans = 1;
        let mut at = [0.0, 0.0, 0.0];
        // 2.5 units a tick, the climb rate looking up, until the head's
        // floor is in reach.
        while at[2] <= 152.0 {
            let w = f.waypoint(&g, goal, at, &mut plans, &mut || 0);
            assert_eq!(w, Some([10.0, 0.0, 200.0]), "at z {}", at[2]);
            at[2] += 2.5;
        }
        at[0] = 10.0;
        let w = f.waypoint(&g, goal, at, &mut plans, &mut || 0);
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
