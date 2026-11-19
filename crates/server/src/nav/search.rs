//! A* over the graph, sliced into budgets, and its route noise.

use super::*;

/// A bot's own route noise ([`Search::noisy`]): each [`NOISE_CELL`]-wide
/// square of the map costs it up to this fraction more or less, so a
/// team's plans split over ways of near the same length. Drawn per square,
/// not per edge: a path's many short edges would average a per-edge draw out.
pub const ROUTE_NOISE: f32 = 0.25;

pub const NOISE_CELL: f32 = 384.0;

/// What [`ROUTE_NOISE`] makes of an edge's length under the noise `seed`:
/// a factor drawn by the square its middle lies in.
fn route_noise(seed: u64, a: Vec3, b: Vec3) -> f32 {
    let mid = (a + b) * (0.5 / NOISE_CELL);
    let cell =
        (u64::from(mid.x.floor() as i32 as u32) << 32) | u64::from(mid.y.floor() as i32 as u32);
    let mut z = (seed ^ cell).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let u = (z >> 40) as f32 / (1u64 << 24) as f32;
    1.0 + ROUTE_NOISE * (2.0 * u - 1.0)
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
#[derive(Clone)]
pub struct Search {
    pub(super) from: u32,
    pub(super) to: u32,
    pub(super) avoid: Vec<(u32, u32)>,
    pub(super) crowd: Vec<(u32, u32)>,
    pub(super) crowd_cost: f32,
    pub(super) noise: u64,
    pub(super) blocked: Arc<[u32]>,
    /// Nodes another body stands over ([`Follower::bodies_in_way`]):
    /// passed through only by the plan after this one fails.
    pub(super) shun: Arc<[u32]>,
    pub(super) jump_cost: f32,
    /// Risky drops stay in with edges avoided: the last plan, round the
    /// dead edges only ([`Search::risking`]).
    risks: bool,
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
            crowd: Vec::new(),
            crowd_cost: CROWD_COST,
            noise: 0,
            blocked: Arc::default(),
            shun: Arc::default(),
            jump_cost: 0.0,
            risks: false,
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

    /// The search with `crowd`'s edges (`(low, high)`, sorted) costing
    /// `cost` times more ([`CROWD_COST`], [`FLANK_COST`]).
    pub fn crowded(mut self, crowd: Vec<(u32, u32)>, cost: f32) -> Search {
        self.crowd = crowd;
        self.crowd_cost = cost;
        self
    }

    /// The search with every edge's length under the bot's own
    /// [`ROUTE_NOISE`] drawn from `seed`; 0 leaves the lengths alone. The
    /// heuristic stays the straight line, so a noisy plan is near its
    /// cheapest, not always it.
    pub fn noisy(mut self, seed: u64) -> Search {
        self.noise = seed;
        self
    }

    /// How many nodes the search has reached: all its start reaches, once
    /// it has failed.
    pub(super) fn reached(&self) -> usize {
        self.cost.iter().filter(|c| c.is_finite()).count()
    }

    /// The search with every jump edge `cost` longer than it is.
    pub fn jumps_costing(mut self, cost: f32) -> Search {
        self.jump_cost = cost;
        self
    }

    /// The search through none of `blocked`'s nodes (sorted) but its
    /// destination.
    pub fn blocking(mut self, blocked: Arc<[u32]>) -> Search {
        self.blocked = blocked;
        self
    }

    /// The search through none of `shun` (sorted) but its destination.
    pub fn shunning(mut self, shun: Arc<[u32]>) -> Search {
        self.shun = shun;
        self
    }

    /// The search down risky drops (at [`RISK_COST`]) even with edges
    /// avoided.
    pub fn risking(mut self) -> Search {
        self.risks = true;
        self
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
            if self.expanded > MAX_EXPANSIONS.max(g.nodes.len()) {
                return Plan::Failed;
            }
            let here = g.nodes[node as usize];
            for &next in &g.edges[node as usize] {
                if self.avoid.contains(&(node, next))
                    || (next != self.to
                        && (self.blocked.binary_search(&next).is_ok()
                            || self.shun.binary_search(&next).is_ok()))
                {
                    continue;
                }
                let risk = if g.risky.contains(&(node, next)) {
                    // Round a stuck edge, never down a risky drop: with the
                    // ladder off mp_ship's crow's nest avoided after a
                    // meeting at its head, plans took the drop down its
                    // hatch, and a bot fell past the rungs to its death
                    // (seed 969, bot-navigation.md section 3). The plan
                    // through the stuck edge comes next, and round only the
                    // dead ones it may take the drop: mp_ship's deck pocket
                    // at (3560, -60, 352) has one plain way out, and with
                    // it dead the bots stood there 10 s a time.
                    if !self.risks && (!self.avoid.is_empty() || !self.shun.is_empty()) {
                        continue;
                    }
                    RISK_COST
                } else {
                    0.0
                };
                let there = g.nodes[next as usize];
                let mut len = here.distance(there);
                if self.noise != 0 {
                    len *= route_noise(self.noise, here, there);
                }
                if self
                    .crowd
                    .binary_search(&(node.min(next), node.max(next)))
                    .is_ok()
                {
                    len *= 1.0 + self.crowd_cost;
                }
                if self.jump_cost > 0.0 && g.jumps.contains(&(node, next)) {
                    len += self.jump_cost;
                }
                let c = self.cost[node as usize] + len + risk;
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
    /// [`Self::roam_pool`] of node `start`, which stands at `from`, the one
    /// farthest from the threat among those nearer the bot than the threat
    /// (a way there that runs past the threat is no escape), else the
    /// farthest of all.
    pub(super) fn flee_from(
        &self,
        start: u32,
        from: Vec3,
        threat: Vec3,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<u32> {
        let pool = self.roam_pool(start)?;
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
#[derive(Clone, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::graph::tests::two_ways;

    /// Whether the plan from 0 to 1 goes north.
    fn north(g: &NavGraph, s: Search) -> bool {
        let mut s = s;
        match s.run(g, &mut 10_000) {
            Plan::Found(p) => p[1] == 2,
            _ => panic!("no plan"),
        }
    }

    /// Route noise splits plans over two ways of one length by the seed,
    /// and never takes one over twice as long.
    #[test]
    fn route_noise_splits_plans_over_ways_of_a_length() {
        let even = two_ways(800.0, 800.0);
        let seeds = (1..=64u64).map(|s| s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let norths = seeds
            .clone()
            .filter(|&s| north(&even, Search::new(&even, 0, 1, Vec::new()).noisy(s)))
            .count();
        assert!((8..=56).contains(&norths), "{norths} of 64 north");
        let long = two_ways(800.0, 4000.0);
        assert!(
            seeds
                .clone()
                .all(|s| north(&long, Search::new(&long, 0, 1, Vec::new()).noisy(s)))
        );
        for s in seeds {
            let f = route_noise(s, Vec3::ZERO, Vec3::new(64.0, 0.0, 0.0));
            assert!((1.0 - ROUTE_NOISE..=1.0 + ROUTE_NOISE).contains(&f));
        }
    }

    /// A teammate's way costs a plan [`CROWD_COST`] more, a flanker's
    /// [`FLANK_COST`]: a way round 2.2 times as long is taken by the
    /// flanker only.
    #[test]
    fn a_flanker_goes_round_the_main_groups_way() {
        let g = two_ways(800.0, 4000.0);
        let mut crowd: Vec<(u32, u32)> = (2..9).map(|i| (i, i + 1)).collect();
        crowd.extend([(0, 2), (1, 9)]);
        crowd.sort_unstable();
        assert!(north(&g, Search::new(&g, 0, 1, Vec::new())));
        let plan = |cost| Search::new(&g, 0, 1, Vec::new()).crowded(crowd.clone(), cost);
        assert!(north(&g, plan(CROWD_COST)));
        assert!(!north(&g, plan(FLANK_COST)));
    }
}
