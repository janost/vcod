//! The bots' navigation graph: a grid flood-filled out of the map's spawn
//! points, each edge proven by running pmove along it, and A* over it.
//! Design and build times: `docs/research/bot-navigation.md`.

use glam::Vec3;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::pmove::cmd::{ANGLE2SHORT, EventRing, chop, player_step};
use vcod_common::pmove::PlayerState;

/// Grid pitch in world units: two capsule widths, the narrowest a doorway
/// the lattice reliably threads (bot-navigation.md, "Spacing").
pub const SPACING: f32 = 32.0;
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
/// The longest a walk runs with a ladder in it: 10 s, ~900 units of climb.
const LADDER_TICKS: usize = 200;
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
    /// Nodes by grid column, for lookups only; nothing iterates it.
    columns: HashMap<(i32, i32), Vec<u32>>,
}

fn column_of(p: Vec3) -> (i32, i32) {
    (
        (p.x / SPACING).round() as i32,
        (p.y / SPACING).round() as i32,
    )
}

impl NavGraph {
    /// Floods the grid from `seeds` (feet origins, typically every spawn
    /// point of the map) and keeps an edge only where a standing run, one
    /// usercmd per 50 ms through the shared cmd step, arrives.
    pub fn build(world: &CollisionWorld, seeds: &[[f32; 3]]) -> NavGraph {
        let mut g = NavGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            columns: HashMap::new(),
        };
        let mut queue = Vec::new();
        for seed in seeds {
            let Some(p) = settle(world, Vec3::from(*seed)) else {
                continue;
            };
            // On the lattice when the seed can walk to its own column's
            // centre, so every later walk targets a node's real spot.
            let c = column_of(p);
            let p = walk(world, p, c).unwrap_or(p);
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
                let (cx, cy) = column_of(g.nodes[n as usize]);
                dirs.map(|(dx, dy)| g.step(world, n, (cx + dx, cy + dy)))
            });
            let mut next = Vec::new();
            for (&(n, dirs), ends) in jobs.iter().zip(ends) {
                let (cx, cy) = column_of(g.nodes[n as usize]);
                for (&(dx, dy), to) in dirs.iter().zip(ends) {
                    let Some(to) = to else {
                        continue;
                    };
                    let c = (cx + dx, cy + dy);
                    let m = match g.find(c, to.z) {
                        Some(m) => m,
                        None if g.nodes.len() < MAX_NODES => {
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
        g
    }

    /// Where a run from node `n` toward column `c`'s centre comes to rest,
    /// skipping the walk when the graph already proves the answer: a level
    /// cardinal edge already walked the other way (half a flood's walks), or
    /// a level diagonal across a square whose four sides run both ways.
    fn step(&self, world: &CollisionWorld, n: u32, c: (i32, i32)) -> Option<Vec3> {
        let from = self.nodes[n as usize];
        let (cx, cy) = column_of(from);
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
        walk(world, from, c)
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
        let (cx, cy) = column_of(p);
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
        while let Some(Open { f, node }) = open.pop() {
            if node == to {
                let mut out = vec![to];
                let mut at = to;
                while at != from {
                    at = came[at as usize];
                    out.push(at);
                }
                out.reverse();
                return Some(out);
            }
            // A stale heap entry: the node was reached cheaper since.
            if f > cost[node as usize] + h(node) + 1e-3 {
                continue;
            }
            expanded += 1;
            if expanded > MAX_EXPANSIONS {
                return None;
            }
            let here = self.nodes[node as usize];
            for &next in &self.edges[node as usize] {
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
        None
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

    /// Test-facing: a graph from explicit nodes and directed edges.
    pub fn from_parts(nodes: Vec<Vec3>, edges: Vec<Vec<u32>>) -> NavGraph {
        let mut columns: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, p) in nodes.iter().enumerate() {
            columns.entry(column_of(*p)).or_default().push(i as u32);
        }
        NavGraph {
            nodes,
            edges,
            columns,
        }
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

/// `f` over `items` on every core, results in `items` order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
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
    out.into_iter().map(|r| r.expect("every item mapped")).collect()
}

/// Idle cmds until a body dropped at `p` stands on something; `None` when it
/// falls out of the world or never lands.
fn settle(world: &CollisionWorld, p: Vec3) -> Option<Vec3> {
    let mut ps = PlayerState::spawn(p + Vec3::Z, 0.0);
    let mut sim = Sim::default();
    for _ in 0..40 {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        if ps.on_ground {
            return Some(ps.origin);
        }
    }
    None
}

/// How a walk is driven: facing its way, or backing along it facing the
/// other way, which is how a body gets onto a ladder below a ledge (the grab
/// traces along the view, `ladder_move`).
#[derive(Clone, Copy, PartialEq)]
enum Gait {
    Forward,
    Backward,
}

/// What a walk came to.
enum Walked {
    Arrived(Vec3),
    Blocked,
    /// It left the ground and never arrived; a ladder below may still take it.
    Fell,
}

/// [`walk_as`] forward, and backward when the forward run fell.
fn walk(world: &CollisionWorld, from: Vec3, c: (i32, i32)) -> Option<Vec3> {
    match walk_as(world, from, c, Gait::Forward) {
        Walked::Arrived(p) => Some(p),
        Walked::Blocked => None,
        Walked::Fell => match walk_as(world, from, c, Gait::Backward) {
            Walked::Arrived(p) => Some(p),
            _ => None,
        },
    }
}

/// Runs a body from `from` toward column `c`'s centre, re-aiming every
/// tick, and reports where it came to rest when it got within [`ARRIVE`] on
/// the ground without falling past [`MAX_DROP`].
fn walk_as(world: &CollisionWorld, from: Vec3, c: (i32, i32), gait: Gait) -> Walked {
    let target = glam::Vec2::new(c.0 as f32 * SPACING, c.1 as f32 * SPACING);
    let dist = target.distance(from.truncate());
    if dist < ARRIVE {
        return Walked::Arrived(from);
    }
    // A running start: a following bot never stops at a node, and the
    // spin-up from a standstill was a third of every walk.
    let mut ps = PlayerState::spawn(from, 0.0);
    let dir = (target - from.truncate()) / dist;
    ps.velocity = (dir * vcod_common::pmove::SPEED_RUN).extend(0.0);
    let mut sim = Sim::default();
    // Twice the run time, plus slack for a stair or a slide along a wall.
    let mut budget = (dist / (vcod_common::pmove::SPEED_RUN * 0.05) * 2.0) as usize + 4;
    let mut stalled = 0;
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
        if ps.on_ground && target.distance(ps.origin.truncate()) < ARRIVE {
            return Walked::Arrived(ps.origin);
        }
        // A climb is slower than a run and straight up, so the budget
        // stretches while it goes on, and progress is the height gained.
        let moved = if ps.on_ladder {
            budget = budget.max(tick + 4).min(LADDER_TICKS);
            (ps.origin.z - before.z).abs() * 4.0
        } else {
            (ps.origin - before).length()
        };
        // Pinned against a wall: two ticks without moving two units. A run
        // that only slides along a wall, or overshoots off a ledge and turns
        // back, keeps its budget.
        if moved < 2.0 {
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
#[cfg(test)]
mod dbg_tmp {
    use super::*;
    #[test]
    fn dbg_walk() {
        let fs = vcod_common::testing::game_fs().unwrap();
        let e = fs.resolve_map("mp_ship").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&e).unwrap()).unwrap();
        let w = crate::world::World::from_bsp(&bsp, Some(&fs));
        for b in &w.collision.brushes {
            if b.surface_flags & vcod_common::collision::SURF_LADDER != 0 {
                let mut lo = [0.0f32; 3];
                let mut hi = [0.0f32; 3];
                for (n, d) in &b.planes {
                    for i in 0..3 {
                        if n[i] > 0.999 { hi[i] = *d; }
                        if n[i] < -0.999 { lo[i] = -*d; }
                    }
                }
                eprintln!("LADDER {} {} {} .. {} {} {}", lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]);
            }
        }
        for z in [80.0, 120.0, 200.0, 320.0] { for x in [4200.0, 4260.0] {
            eprintln!("try {x} {z}: {:?}", settle(&w.collision, Vec3::new(x, 232.0, z)));
        } }
        for start in [Vec3::new(4200.0, 232.0, 80.0), Vec3::new(4260.0, 232.0, 80.0)] {
            let Some(from) = settle(&w.collision, start) else { continue };
            eprintln!("settled {from:?}");
            let target = glam::Vec2::new(4240.0, 224.0);
            let mut ps = PlayerState::spawn(from, 0.0);
            let mut sim = Sim::default();
            for t in 0..60 {
                let to = target - ps.origin.truncate();
                let yaw = to.y.atan2(to.x).to_degrees();
                let cmd = UserCmd { forward: 127, angles: [0, (yaw * ANGLE2SHORT) as i32, 0], ..NULL_USERCMD };
                sim.tick(&w.collision, &mut ps, &cmd);
                if t % 4 == 0 { eprintln!("{t} {:?} v {:?} ground {} ladder {}", ps.origin, ps.velocity, ps.on_ground, ps.on_ladder); }
            }
        }
    }
}
