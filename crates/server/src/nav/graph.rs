//! The graph itself: nodes, edges, components, lookups round a point and
//! the paths over it.

use super::*;

/// One directed edge and what the walk that proved it needed. A link that
/// proves an edge without a jump clears `jump` and `crouched` only
/// ([`NavGraph::link_as`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edge {
    pub to: u32,
    /// Proved only with a jump; a bot following one jumps where the walk
    /// did ([`PinJump`], [`leap_cue`]).
    pub jump: bool,
    /// A jump proved from a standstill at its foot, jumping at the lip
    /// ([`NavGraph::link_leaps`]): a bot comes to rest at the foot first.
    pub leap: bool,
    /// A jump proved only with crouch held from the tick after the jump
    /// until the body lands ([`Gait::CrouchJump`]), through a window too
    /// low for a standing body ([`NavGraph::link_pockets`]).
    pub crouched: bool,
    /// A drop deeper than a jump whose walk from 4 units aside of its node
    /// falls, or a walk down beside a fall that hurts whose forward walk
    /// does: a plan takes one only where there is no other way
    /// ([`RISK_COST`], [`NavGraph::mark_risky`]).
    pub risky: bool,
    /// Only a CoD2 or CoD4 mantle climbs it ([`NavGraph::link_mantles`]): a
    /// bot on one runs at the wall and jumps once pmove offers the climb.
    pub mantle: bool,
    /// A jump whose walk lands only aimed beside its top
    /// ([`NavGraph::reprove_jumps`]), and the point it aimed at: a bot on
    /// one steers there ([`NavGraph::aim`]).
    pub aim: Option<glam::Vec2>,
    /// A mantle's landing as its probe proved it, which may stand a cell
    /// off the node the edge ends at: a body run at the node need not be
    /// offered the climb ([`Follower::mantle_land`]).
    pub land: Option<Vec3>,
}

impl Edge {
    pub fn to(to: u32) -> Edge {
        Edge {
            to,
            ..Edge::default()
        }
    }
}

pub struct NavGraph {
    /// Feet origins where pmove came to rest.
    pub nodes: Vec<Vec3>,
    /// Directed: `edges[a]` holds every edge a run from `a` proved, in the
    /// order the build linked them (A*'s tie-break). A drop off a ledge has
    /// no edge back.
    pub edges: Vec<Vec<Edge>>,
    /// Nodes whose middle stands over a fall that kills, on a rail or a
    /// spar's edge: a roam never ends on one ([`NavGraph::mark_perches`]).
    pub perches: HashSet<u32>,
    /// The nodes the flood started from at the spawn points. A ladder's
    /// ends seed a flood of their own, which may never meet the spawns'
    /// floors: mp_shipment's grounds round the yard, out of bounds, are
    /// the larger part of its graph.
    pub(super) seeds: Vec<u32>,
    /// The grid pitch the graph was built on.
    pub spacing: f32,
    /// Nodes by grid column, for lookups only; nothing iterates it.
    pub(super) columns: HashMap<(i32, i32), Vec<u32>>,
    /// Each node's strongly connected component ([`Self::components`]), and
    /// each component's nodes in index order: a roam picks its destination
    /// among the nodes its start reaches both ways.
    pub(super) comp: Vec<u32>,
    pub(super) members: Vec<Vec<u32>>,
    /// Per component, a bitset of the components it reaches (itself
    /// included): an unreachable goal fails before any search. A stock map
    /// has 1 to 13 components.
    pub(super) reach: Vec<Vec<u64>>,
    /// The largest strongly connected component with the [`Self::risky`]
    /// edges left out that holds a spawn's seed ([`Self::seeds`]): a roam
    /// from inside it never ends where only a risky edge leads
    /// ([`Self::roam_from`]).
    pub(super) safe: Vec<bool>,
    /// The world's ladders ([`ladders`]), for the build.
    pub(super) ladder_boxes: Vec<(Vec3, Vec3)>,
    /// The movement rules the walks proved the edges under.
    pub rules: MoveRules,
    /// The fall heights they judged drops by ([`with_falls`]).
    pub falls: FallHeights,
    /// The game's own step they moved by instead, if any ([`with_step`]).
    pub step: Option<Arc<dyn WalkStep>>,
}

/// The spawn points' horizontal bounding box.
pub(super) fn seed_bounds(seeds: &[[f32; 3]]) -> (glam::Vec2, glam::Vec2) {
    seeds.iter().fold(
        (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)),
        |(lo, hi), s| {
            let p = glam::Vec2::new(s[0], s[1]);
            (lo.min(p), hi.max(p))
        },
    )
}

impl NavGraph {
    /// The spawn points in the strongly connected component holding the most
    /// of them: the census bot-navigation.md's tables report. Each spawn is
    /// looked up from where a body dropped at it lands, where a bot spawned
    /// there stands; from the spawn's own origin, often 100 units up, the
    /// nearest node can be one on a crate beside it.
    pub fn spawns_in_one_component(&self, world: &CollisionWorld, spawns: &[[f32; 3]]) -> usize {
        self.spawn_census(world, spawns).0
    }

    /// [`Self::spawns_in_one_component`], and the spawn points whose node
    /// reaches that component: a United Offensive base's spawns mostly
    /// stand behind a drop out of it, which a bot spawned there leaves by
    /// and never comes back up (bot-navigation.md, "Other rules").
    pub fn spawn_census(&self, world: &CollisionWorld, spawns: &[[f32; 3]]) -> (usize, usize) {
        let (most, lost) = self.census_lost(world, spawns);
        (most, spawns.len() - lost.len())
    }

    /// Runs `f` with the walks' bodies moving, and drops judged, as this
    /// graph's were.
    pub fn moving<R>(&self, f: impl FnOnce() -> R) -> R {
        with_rules(self.rules, || {
            with_falls(self.falls, || with_step(self.step.clone(), f))
        })
    }

    /// The plain edges down more than a step and no more than 64 whose
    /// forward walk node to node does not arrive, so a back down or a creep
    /// proved them: (from, to, within [`LADDER_NEAR`] of a ladder)
    /// (`nav_build`'s `BACKED=1`, bot-navigation.md section 3).
    pub fn census_backed(&self, world: &CollisionWorld) -> Vec<(Vec3, Vec3, bool)> {
        let mut out = Vec::new();
        self.moving(|| {
            for (a, bs) in self.edges.iter().enumerate() {
                for &Edge {
                    to: b,
                    jump,
                    mantle,
                    ..
                } in bs
                {
                    if jump || mantle {
                        continue;
                    }
                    let (p, q) = (self.nodes[a], self.nodes[b as usize]);
                    let dz = p.z - q.z;
                    if dz <= vcod_common::pmove::STEPSIZE || dz > 64.0 {
                        continue;
                    }
                    let fw = walk_as(world, p, q.truncate(), Some(q.z), Gait::Forward);
                    if matches!(fw, Walked::Arrived(_, false)) {
                        continue;
                    }
                    out.push((p, q, ladder_near(&self.ladder_boxes, p)));
                }
            }
        });
        out
    }

    /// The census's main-component count, and the spawns that do not reach
    /// that component: each spawn's index with its node, `None` for one
    /// with no node at all (`nav_build`'s `LOST=1`).
    pub fn census_lost(
        &self,
        world: &CollisionWorld,
        spawns: &[[f32; 3]],
    ) -> (usize, Vec<(usize, Option<u32>)>) {
        let nodes: Vec<Option<u32>> = spawns
            .iter()
            .map(|s| {
                let feet = self.moving(|| settle(world, Vec3::from(*s), 0.0));
                self.nearest(feet.map_or(*s, Into::into))
            })
            .collect();
        let mut count = BTreeMap::new();
        for &n in nodes.iter().flatten() {
            *count.entry(self.comp[n as usize]).or_insert(0) += 1;
        }
        // The most spawns, the lower component on a tie.
        let Some((&main, &most)) = count.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        else {
            return (0, (0..spawns.len()).map(|i| (i, None)).collect());
        };
        let head = self.members[main as usize][0];
        let lost = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_none_or(|n| !self.reaches(n, head)))
            .map(|(i, &n)| (i, n))
            .collect();
        (most, lost)
    }

    /// The components of `comp` (one per node) that hold a spawn's seed
    /// ([`Self::seeds`]); empty for a graph built without any.
    fn seeded(&self, comp: &[u32]) -> HashSet<u32> {
        self.seeds.iter().map(|&n| comp[n as usize]).collect()
    }

    pub(super) fn index_components(&mut self) {
        self.comp = self.components();
        let safe = self.components_by(|e| !e.risky);
        let mut sizes = HashMap::new();
        for &c in &safe {
            *sizes.entry(c).or_insert(0usize) += 1;
        }
        let seeded = self.seeded(&safe);
        sizes.retain(|c, _| seeded.is_empty() || seeded.contains(c));
        let largest = sizes
            .into_iter()
            .max_by_key(|&(c, k)| (k, c))
            .map(|(c, _)| c);
        self.safe = safe.iter().map(|&c| Some(c) == largest).collect();
        let count = self.comp.iter().max().map_or(0, |&c| c as usize + 1);
        self.members = vec![Vec::new(); count];
        for (n, &c) in self.comp.iter().enumerate() {
            self.members[c as usize].push(n as u32);
        }
        // The condensation's edges, then a walk of them from each component.
        let mut out = vec![HashSet::new(); count];
        for (a, next) in self.edges.iter().enumerate() {
            let ca = self.comp[a];
            for &Edge { to: b, .. } in next {
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
    pub(super) fn nearest_reached(&self, from: u32, to: u32) -> Option<u32> {
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

    /// The nodes that reach no node of the largest strongly connected
    /// component, in index order.
    pub(super) fn cut_off(&self) -> Vec<u32> {
        let comp = self.components();
        let mut sizes: HashMap<u32, usize> = HashMap::new();
        for &c in &comp {
            *sizes.entry(c).or_insert(0) += 1;
        }
        let Some(main) = sizes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(&c, _)| c)
        else {
            return Vec::new();
        };
        let mut into: Vec<Vec<u32>> = vec![Vec::new(); self.nodes.len()];
        for (a, out) in self.edges.iter().enumerate() {
            for &Edge { to: b, .. } in out {
                into[b as usize].push(a as u32);
            }
        }
        let mut reaches = vec![false; self.nodes.len()];
        let mut stack: Vec<u32> = (0..self.nodes.len() as u32)
            .filter(|&n| comp[n as usize] == main)
            .collect();
        for &n in &stack {
            reaches[n as usize] = true;
        }
        while let Some(b) = stack.pop() {
            for &a in &into[b as usize] {
                if !std::mem::replace(&mut reaches[a as usize], true) {
                    stack.push(a);
                }
            }
        }
        (0..self.nodes.len() as u32)
            .filter(|&n| !reaches[n as usize])
            .collect()
    }

    /// Where a run from node `n` toward column `c`'s centre comes to rest,
    /// skipping the walk when the graph already proves the answer: a level
    /// cardinal edge already walked the other way (half a flood's walks), or
    /// a level diagonal across a square whose four sides run both ways.
    pub(super) fn step(
        &self,
        world: &CollisionWorld,
        n: u32,
        c: (i32, i32),
    ) -> Option<(Vec3, bool)> {
        let from = self.nodes[n as usize];
        let (cx, cy) = self.column(from);
        let level = |m: u32| (self.nodes[m as usize].z - from.z).abs() < LEVEL;
        // Walked without a jump: a jump one way says nothing of the other.
        let has = |a: u32, b: u32| self.linked(a, b) && !self.has(a, b, |k| k.jump);
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

    pub(super) fn column(&self, p: Vec3) -> (i32, i32) {
        (
            (p.x / self.spacing).round() as i32,
            (p.y / self.spacing).round() as i32,
        )
    }

    pub(super) fn center(&self, c: (i32, i32)) -> glam::Vec2 {
        glam::Vec2::new(c.0 as f32, c.1 as f32) * self.spacing
    }

    pub(super) fn link(&mut self, a: u32, b: u32) {
        self.link_as(a, b, false);
    }

    /// Links `a -> b`, a jump edge when `jumped`. A walk that proves an edge
    /// without a jump clears its jump.
    pub(super) fn link_as(&mut self, a: u32, b: u32, jumped: bool) {
        if a == b {
            return;
        }
        match self.edge_mut(a, b) {
            None => self.edges[a as usize].push(Edge {
                jump: jumped,
                ..Edge::to(b)
            }),
            Some(e) if !jumped => {
                e.jump = false;
                e.crouched = false;
            }
            Some(_) => {}
        }
    }

    /// [`Self::link_as`], a crouched jump edge when `crouched`.
    pub(super) fn link_crouched(&mut self, a: u32, b: u32, jumped: bool, crouched: bool) {
        self.link_as(a, b, jumped);
        if let Some(e) = self.edge_mut(a, b).filter(|e| crouched && e.jump) {
            e.crouched = true;
        }
    }

    /// Edge `a -> b`, if the graph has it.
    pub fn edge(&self, a: u32, b: u32) -> Option<&Edge> {
        self.edges.get(a as usize)?.iter().find(|e| e.to == b)
    }

    pub fn edge_mut(&mut self, a: u32, b: u32) -> Option<&mut Edge> {
        self.edges
            .get_mut(a as usize)?
            .iter_mut()
            .find(|e| e.to == b)
    }

    /// Whether the graph has edge `a -> b`.
    pub fn linked(&self, a: u32, b: u32) -> bool {
        self.edge(a, b).is_some()
    }

    /// Whether edge `a -> b` exists and `kind` holds of it.
    pub fn has(&self, a: u32, b: u32, kind: impl FnOnce(&Edge) -> bool) -> bool {
        self.edge(a, b).is_some_and(kind)
    }

    /// The nodes `a`'s edges lead to, in edge order.
    pub fn next(&self, a: u32) -> impl Iterator<Item = u32> + '_ {
        self.edges[a as usize].iter().map(|e| e.to)
    }

    /// Every edge `(a, b)` that `kind` holds of, in edge order.
    pub fn edges_where<'a>(
        &'a self,
        kind: impl Fn(&Edge) -> bool + Copy + 'a,
    ) -> impl Iterator<Item = (u32, u32)> + 'a {
        self.edges.iter().enumerate().flat_map(move |(a, out)| {
            out.iter()
                .filter(move |e| kind(e))
                .map(move |e| (a as u32, e.to))
        })
    }

    pub(super) fn add(&mut self, c: (i32, i32), p: Vec3) -> u32 {
        let n = self.nodes.len() as u32;
        self.nodes.push(p);
        self.edges.push(Vec::new());
        self.columns.entry(c).or_default().push(n);
        n
    }

    /// Column `c`'s node nearest height `z`, within [`Z_MERGE`].
    pub(super) fn find(&self, c: (i32, i32), z: f32) -> Option<u32> {
        let dz = |n: u32| (self.nodes[n as usize].z - z).abs();
        self.columns
            .get(&c)?
            .iter()
            .copied()
            .filter(|&n| dz(n) < Z_MERGE)
            .min_by(|&a, &b| dz(a).total_cmp(&dz(b)))
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

    /// The nodes within `reach` of `at` across and up or down where a
    /// standing body no longer fits in `world` as it stands now: a UO
    /// vehicle parked since the build covers them with its collmap
    /// (bot-navigation.md, "Parked vehicles").
    pub fn blocked_near(&self, world: &CollisionWorld, at: Vec3, reach: f32) -> Vec<u32> {
        let (cx, cy) = self.column(at);
        let span = (reach / self.spacing).ceil() as i32;
        let mut out = Vec::new();
        for dy in -span..=span {
            for dx in -span..=span {
                let Some(col) = self.columns.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &n in col {
                    let p = self.nodes[n as usize];
                    if (p - at).truncate().length() <= reach
                        && (p.z - at.z).abs() <= reach
                        && !stands(world, p + Vec3::Z * 0.125)
                    {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// The nodes less than `half` from `at` along both axes across and
    /// less than `rise` up or down: where a body's box would meet one of
    /// half width `half` minus its own standing at `at`.
    pub fn nodes_in_box(&self, at: Vec3, half: f32, rise: f32) -> Vec<u32> {
        let (cx, cy) = self.column(at);
        let span = (half / self.spacing).ceil() as i32;
        let mut out = Vec::new();
        for dy in -span..=span {
            for dx in -span..=span {
                for &n in self.columns.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let d = self.nodes[n as usize] - at;
                    if d.x.abs() < half && d.y.abs() < half && d.z.abs() < rise {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// The node nearest node `n` (by [`Self::nearest`]'s score) within
    /// [`CLEAR_STEPS`] grid steps across that is in none of `blocked`
    /// (sorted) and passes `ok`, else within [`CLEAR_STEPS_FAR`].
    pub fn clear_near(&self, n: u32, blocked: &[u32], ok: impl Fn(u32) -> bool) -> Option<u32> {
        self.clear_within(n, blocked, &ok, CLEAR_STEPS)
            .or_else(|| self.clear_within(n, blocked, &ok, CLEAR_STEPS_FAR))
    }

    fn clear_within(
        &self,
        n: u32,
        blocked: &[u32],
        ok: &impl Fn(u32) -> bool,
        steps: i32,
    ) -> Option<u32> {
        let at = self.nodes[n as usize];
        let (cx, cy) = self.column(at);
        let mut best: Option<(f32, u32)> = None;
        for dy in -steps..=steps {
            for dx in -steps..=steps {
                let Some(col) = self.columns.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &m in col {
                    let d = self.nodes[m as usize] - at;
                    let score = d.truncate().length() + 4.0 * d.z.abs();
                    if best.is_none_or(|(s, b)| score < s || (score == s && m < b))
                        && blocked.binary_search(&m).is_err()
                        && ok(m)
                    {
                        best = Some((score, m));
                    }
                }
            }
        }
        best.map(|(_, m)| m)
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

    /// The node a point goal at `p` is walked to: [`Self::nearest`],
    /// unless that is one of [`Self::perches`] and a node off them stands
    /// within [`RETARGET`] of `p`, the nearest such then. A chase or a
    /// pickup on mp_ship's crow's nest rails otherwise ends at a run on a
    /// rail two units from its edge, as roams did (section 3).
    pub fn goal_node(&self, p: [f32; 3]) -> Option<u32> {
        let n = self.nearest(p)?;
        if !self.perches.contains(&n) {
            return Some(n);
        }
        let p = Vec3::from(p);
        let (cx, cy) = self.column(p);
        let mut best: Option<(f32, u32)> = None;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let Some(col) = self.columns.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &m in col {
                    if self.perches.contains(&m) || self.edges[m as usize].is_empty() {
                        continue;
                    }
                    let d = self.nodes[m as usize] - p;
                    if d.length() > RETARGET {
                        continue;
                    }
                    let score = d.truncate().length() + 4.0 * d.z.abs();
                    if best.is_none_or(|(s, b)| score < s || (score == s && m < b)) {
                        best = Some((score, m));
                    }
                }
            }
        }
        Some(best.map_or(n, |(_, m)| m))
    }

    /// Where a bot at `from` whose line of fire along `line` a teammate
    /// blocks steps to: a node one or two level edges (no jump, no drop)
    /// from the nearest, [`SIDESTEP_MIN`] to [`SIDESTEP_MAX`] units off and
    /// more across the line than along it, nearest first, the first that
    /// `clear` passes.
    pub fn sidestep(&self, from: Vec3, line: Vec3, clear: impl Fn(Vec3) -> bool) -> Option<Vec3> {
        let start = self.nearest(from.into())?;
        let line = line.truncate().normalize_or_zero();
        let level = |a: u32, b: u32| {
            (self.nodes[a as usize].z - self.nodes[b as usize].z).abs()
                <= vcod_common::pmove::STEPSIZE
                && !self.has(a, b, |k| k.jump)
                && !self.has(a, b, |k| k.risky)
        };
        let mut seen = vec![start];
        let mut ring = vec![start];
        for _ in 0..2 {
            let mut next = Vec::new();
            for &a in &ring {
                for b in self.next(a) {
                    if !seen.contains(&b) && level(a, b) {
                        seen.push(b);
                        next.push(b);
                    }
                }
            }
            ring = next;
        }
        let mut spots: Vec<(f32, Vec3)> = seen
            .iter()
            .map(|&n| self.nodes[n as usize])
            .filter_map(|p| {
                let off = (p - from).truncate();
                let d = off.length();
                let across = off.perp_dot(line).abs();
                ((SIDESTEP_MIN..=SIDESTEP_MAX).contains(&d) && across > off.dot(line).abs())
                    .then_some((d, p))
            })
            .collect();
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        spots.into_iter().map(|(_, p)| p).find(|p| clear(*p))
    }

    /// Whether [`Self::nearest`] to `p` has no edge out: a ledge the flood
    /// found no way off but a fall (mp_ship's bow gutters at y -152 and 281,
    /// z 312, under a roof too steep to climb and over a drop of 376).
    pub fn dead_end(&self, p: [f32; 3]) -> bool {
        self.nearest(p)
            .is_some_and(|n| self.edges[n as usize].is_empty())
    }

    /// Whether [`Self::nearest`] to `p`, on its floor, reaches no node of
    /// the largest component, and no spawn cut off the same way reaches it:
    /// a pit only a fall from the fight leads into, whose roam pool is the
    /// pit itself. mp_foy's hollow at (4620, -2225, -14) is dropped into
    /// from rims 38 to 58 up and left by no walk or jump. A room a cut-off
    /// spawn leads to is a place to be whose way out the graph missed:
    /// mp_italy's allied bunker spawns (6682, -2624, -706) and the charge
    /// room under them reach no node of it under `bas` (bot-navigation.md
    /// section 3).
    pub fn pit(&self, p: [f32; 3], spawns: &[[f32; 3]]) -> bool {
        let Some(n) = self.nearest(p) else {
            return false;
        };
        if (self.nodes[n as usize].z - p[2]).abs() > vcod_common::pmove::STEPSIZE {
            return false;
        }
        let Some(main) = (0..self.members.len()).max_by_key(|&c| self.members[c].len()) else {
            return false;
        };
        let reaches = |c: usize, to: usize| self.reach[c][to / 64] & (1 << (to % 64)) != 0;
        let own = self.comp[n as usize] as usize;
        !reaches(own, main)
            && !spawns.iter().any(|s| {
                self.nearest(*s).is_some_and(|m| {
                    let c = self.comp[m as usize] as usize;
                    !reaches(c, main) && reaches(c, own)
                })
            })
    }

    /// Whether [`Self::nearest`] to `p` stands more than a step off its
    /// height: a body off the graph, whose plan starts at a node it walks
    /// to ([`Self::nearest_walkable`]).
    pub fn off_floor(&self, p: Vec3) -> bool {
        self.nearest(p.into())
            .is_some_and(|n| (self.nodes[n as usize].z - p.z).abs() > vcod_common::pmove::STEPSIZE)
    }

    /// For a body on a ladder, [`Self::nearest`] among the ends of the
    /// climbs around it: nodes an edge climbs more than a jump from or to.
    /// The nearest node to a body on the rungs is often a floor behind its
    /// back, which a ladder move does not reach (mp_ship's ladder at
    /// x 3354, where a plan from the floor at (3427, 546, 128) held a
    /// climber on the rungs for 100 ticks). `nearest` when there is none.
    pub fn nearest_ladder_end(&self, p: Vec3) -> Option<u32> {
        use vcod_common::pmove::JUMP_HEIGHT;
        let (cx, cy) = self.column(p);
        let near: Vec<u32> = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (cx + dx, cy + dy)))
            .filter_map(|c| self.columns.get(&c))
            .flatten()
            .copied()
            .collect();
        let z = |n: u32| self.nodes[n as usize].z;
        let mut ends: HashSet<u32> = HashSet::new();
        for &n in &near {
            for b in self.next(n) {
                if z(b) > z(n) + JUMP_HEIGHT {
                    ends.extend([n, b]);
                }
            }
        }
        let score = |n: u32| {
            let d = self.nodes[n as usize] - p;
            d.truncate().length() + 4.0 * d.z.abs()
        };
        ends.into_iter()
            .filter(|n| near.contains(n))
            .min_by(|&a, &b| score(a).total_cmp(&score(b)).then(a.cmp(&b)))
            .or_else(|| self.nearest(p.into()))
    }

    /// [`Self::nearest`], or of its nearest few by the same score the first
    /// a walk from `p` arrives at on its floor without a jump (a bot jumps
    /// only on a jump edge, never to its first node): a body that fell off its
    /// path can stand where the nearest node is a wall away. mp_ship's lip at
    /// (5424, -187, 344), under the shelf at 396 and over a stair, has the
    /// floor at 352 beyond the shelf's side nearest; the stair node at 320
    /// is a walk off the lip. Where none of those arrives, the nearest few
    /// more than a jump below, flat, get the same walk: a pit off the graph
    /// whose way out is a drop scores the floors over its rim nearest
    /// (mp_ship's boat at (5250, -500, 27), bot-navigation.md section 3).
    pub fn nearest_walkable(&self, world: &CollisionWorld, p: Vec3) -> Option<u32> {
        self.walkable_start(world, p, &|_| true).map(|(n, _)| n)
    }

    /// [`Self::nearest_walkable`], and whether the walk there takes a jump.
    /// Where no walk arrives without one, the first of the nearest few a
    /// bot's walk ([`as_a_bot`]) arrives at with its jump: a dent off the
    /// graph walled higher than a step all round has no other way out, and
    /// the bot runs that first leg as a jump edge ([`Follower::jumping`]).
    /// mp_cassino's roof dent at (4948, -1413, 280), 23 under the roof
    /// round it, held a bot pressing at the node at 280 beyond its wall for
    /// 12 s (bot-objectives.md 10.6). The node may stand as low as the feet:
    /// mp_cassino's pocket at (5882, -2038, -8), 30 under the steps round
    /// it, is ringed by nodes on its own floor beyond the walls
    /// (bot-objectives.md 14.13). Only nodes `keep` passes are scored,
    /// unless it passes none.
    pub fn walkable_start(
        &self,
        world: &CollisionWorld,
        p: Vec3,
        keep: &dyn Fn(u32) -> bool,
    ) -> Option<(u32, bool)> {
        use vcod_common::pmove::JUMP_HEIGHT;
        let (cx, cy) = self.column(p);
        let mut near: Vec<(f32, u32)> = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| (cx + dx, cy + dy)))
            .filter_map(|c| self.columns.get(&c))
            .flatten()
            .map(|&n| {
                let d = self.nodes[n as usize] - p;
                (d.truncate().length() + 4.0 * d.z.abs(), n)
            })
            .collect();
        if near.iter().any(|&(_, n)| keep(n)) {
            near.retain(|&(_, n)| keep(n));
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let walks = |n: u32| {
            let to = self.nodes[n as usize];
            matches!(
                walk_as(world, p, to.truncate(), Some(to.z), Gait::Forward),
                Walked::Arrived(_, false)
            )
        };
        let tried = &near[..near.len().min(WALKABLE_TRIES)];
        let mut below: Vec<(f32, u32)> = near
            .iter()
            .filter(|&&(_, n)| {
                self.nodes[n as usize].z < p.z - JUMP_HEIGHT && !tried.iter().any(|t| t.1 == n)
            })
            .map(|&(_, n)| ((self.nodes[n as usize] - p).truncate().length(), n))
            .collect();
        below.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        // Out of it with a jump, from rest: a body that found no walk out
        // stands. The node may stand no higher than the feet: a pit's way
        // out is over its rim to the floor beyond.
        let jumps = |n: u32| {
            let to = self.nodes[n as usize];
            matches!(
                as_a_bot(|| walk_with(
                    world,
                    p,
                    Some(Vec3::ZERO),
                    to.truncate(),
                    Some(to.z),
                    Gait::Forward
                )),
                Walked::Arrived(_, true)
            )
        };
        tried
            .iter()
            .chain(below.iter().take(WALKABLE_TRIES))
            .find(|&&(_, n)| walks(n))
            .map(|&(_, n)| (n, false))
            .or_else(|| {
                tried
                    .iter()
                    .find(|&&(_, n)| jumps(n))
                    .map(|&(_, n)| (n, true))
            })
            .or(near.first().map(|&(_, n)| (n, false)))
    }

    /// Where a bot stands in a zone `mins`..`maxs` (a bombzone, a goal
    /// trigger): the node 16 units inside the bounds nearest their middle
    /// at the floor, from the component holding the most nodes of those a
    /// spawn's seed is in ([`Self::seeds`]) so a bot anywhere can reach it,
    /// else the nearest such node to the middle.
    pub fn zone_stand(&self, mins: [f32; 3], maxs: [f32; 3]) -> Option<[f32; 3]> {
        let mid = Vec3::new(
            (mins[0] + maxs[0]) * 0.5,
            (mins[1] + maxs[1]) * 0.5,
            mins[2],
        );
        let inside = |n: &Vec3| {
            (0..2).all(|i| n[i] >= mins[i] + 16.0 && n[i] <= maxs[i] - 16.0)
                && n.z <= maxs[2]
                && n.z + 70.0 >= mins[2]
        };
        self.stand_where(mid, inside)
    }

    /// Where a bot stands to take a headquarters radio at `at`: as
    /// [`Self::zone_stand`], the node within `reach` across and `height`
    /// up or down of it nearest it.
    pub fn radio_stand(&self, at: [f32; 3], reach: f32, height: f32) -> Option<[f32; 3]> {
        let at = Vec3::from(at);
        self.stand_where(at, |n: &Vec3| {
            (*n - at).truncate().length() < reach && (n.z - at.z).abs() < height
        })
    }

    /// The node `inside` keeps nearest `mid`, from the component of
    /// [`Self::zone_stand`], else the nearest there to `mid`.
    fn stand_where(&self, mid: Vec3, inside: impl Fn(&Vec3) -> bool) -> Option<[f32; 3]> {
        let comp = self.component_ids();
        let mut sizes: std::collections::BTreeMap<u32, usize> = Default::default();
        for c in comp {
            *sizes.entry(*c).or_default() += 1;
        }
        let seeded = self.seeded(comp);
        sizes.retain(|c, _| seeded.is_empty() || seeded.contains(c));
        // Largest first, lower number on a tie, for a pick that does not
        // depend on map iteration.
        let main = sizes
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(c, _)| *c);
        let best = |want_inside: bool| {
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, n)| Some(comp[*i]) == main && (!want_inside || inside(n)))
                .min_by(|a, b| a.1.distance(mid).total_cmp(&b.1.distance(mid)))
                .map(|(_, n)| (*n).into())
        };
        best(true).or_else(|| best(false))
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
        self.components_by(|_| true)
    }

    /// [`Self::components`] as indexed when the graph was finished, which
    /// nothing changes after: a lookup, where a fresh walk of a stock UO
    /// graph took 9 to 26 ms (map-cycle doc 4.9).
    pub fn component_ids(&self) -> &[u32] {
        &self.comp
    }

    /// [`Self::components`] over only the edges `keep` passes.
    fn components_by(&self, keep: impl Fn(&Edge) -> bool) -> Vec<u32> {
        let n = self.nodes.len();
        let edges: Vec<Vec<u32>> = self
            .edges
            .iter()
            .map(|out| out.iter().filter(|e| keep(e)).map(|e| e.to).collect())
            .collect();
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
                if let Some(&w) = edges[*v as usize].get(*i) {
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
        for (a, out) in edges.iter().enumerate() {
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
            edges: edges
                .into_iter()
                .map(|out| out.into_iter().map(Edge::to).collect())
                .collect(),
            perches: HashSet::new(),
            seeds: Vec::new(),
            spacing: SPACING_MIN,
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            safe: Vec::new(),
            ladder_boxes: Vec::new(),
            rules: MoveRules::V1_1,
            falls: FallHeights::COD1,
            step: None,
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

impl NavGraph {
    /// Whether the edge `a` to `b` is a plain walk on one floor: no jump,
    /// leap, crouched jump or risky drop, and no more than a step between
    /// its ends. What [`Follower::pull`] cuts across.
    pub fn plain_edge(&self, a: u32, b: u32) -> bool {
        !self.has(a, b, |e| {
            e.jump || e.leap || e.crouched || e.risky || e.mantle
        }) && (self.nodes[a as usize].z - self.nodes[b as usize].z).abs()
            <= vcod_common::pmove::STEPSIZE
    }

    /// For a body at `p` on a ladder, the point level with it across the
    /// face and at the middle of the ladder along it: a climb strafes
    /// toward it, since what overhangs a ladder (a spar, a hatch's rim)
    /// mostly pins a body at its edge (section 3).
    pub fn ladder_middle(&self, p: Vec3) -> Option<glam::Vec2> {
        self.ladder_boxes
            .iter()
            .filter(|(lo, hi)| p.z > lo.z - 80.0 && p.z < hi.z + 8.0)
            .filter_map(|&(lo, hi)| {
                let ext = hi - lo;
                let (a, b) = if ext.x < ext.y { (0, 1) } else { (1, 0) };
                let away = (lo[a] - p[a]).max(p[a] - hi[a]).max(0.0);
                let aside = (lo[b] - p[b]).max(p[b] - hi[b]).max(0.0);
                let mut m = p.truncate();
                m[b] = (lo[b] + hi[b]) * 0.5;
                (away < 32.0 && aside < 16.0).then_some((away + aside, m))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|x| x.1)
    }

    /// The point a climb from the ladder foot `foot` runs at: straight into
    /// the nearest ladder face it stands square in front of, as the climb
    /// in [`rung`] did from there. The heading at the head instead drifts
    /// along the face from a foot a few units aside and grabs the rungs at
    /// their edge (section 3). A foot off to the side (a flood walk that
    /// met the ladder) keeps the heading its walk proved.
    pub(super) fn face_ahead(&self, foot: Vec3) -> Option<glam::Vec2> {
        self.ladder_boxes
            .iter()
            .filter(|(lo, hi)| foot.z > lo.z - 48.0 && foot.z < hi.z)
            .filter_map(|&(lo, hi)| {
                // The face's axis is the box's thinner one; `a` across the
                // face, `b` along it.
                let ext = hi - lo;
                let (a, b) = if ext.x < ext.y { (0, 1) } else { (1, 0) };
                let plane = if foot[a] > hi[a] {
                    hi[a]
                } else if foot[a] < lo[a] {
                    lo[a]
                } else {
                    return None;
                };
                let away = (foot[a] - plane).abs();
                let aside = (lo[b] - foot[b]).max(foot[b] - hi[b]);
                let mut p = foot.truncate();
                p[a] = plane;
                (away < 48.0 && aside <= 0.0).then_some((away, p))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|x| x.1)
    }

    /// The nodes a roam or a flight from node `start` picks among: the
    /// largest component the start reaches, its own included. A pick the
    /// start cannot reach would cost a search of everything it can; one in
    /// its own component alone keeps a bot spawned on an island with only
    /// drops off it (`mp_depot`'s platform at z 27) pacing it for good, and
    /// one in any reached component lets a bot roam into a pit it never
    /// leaves.
    pub(super) fn roam_pool(&self, start: u32) -> Option<&[u32]> {
        let own = *self.comp.get(start as usize)? as usize;
        let bits = &self.reach[own];
        let best = (0..self.members.len())
            .filter(|&c| bits[c / 64] & (1 << (c % 64)) != 0)
            .max_by_key(|&c| (self.members[c].len(), c == own))?;
        Some(&self.members[best])
    }

    /// A roam destination: of a handful of random nodes in
    /// [`Self::roam_pool`] of node `start`, which stands at `from`, the
    /// first at least [`ROAM_MIN`] away, else the farthest; never one of
    /// [`Self::perches`], nor, from a start in [`Self::safe`], one outside
    /// it.
    pub(super) fn roam_from(
        &self,
        start: u32,
        from: Vec3,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<u32> {
        let pool = self.roam_pool(start)?;
        let mut best: Option<(f32, u32)> = None;
        for _ in 0..8 {
            let n = pool[rand() as usize % pool.len()];
            if self.perches.contains(&n) || (self.safe[start as usize] && !self.safe[n as usize]) {
                continue;
            }
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;

    /// Each node's edge targets, what [`NavGraph::from_parts`] takes.
    fn targets(g: &NavGraph) -> Vec<Vec<u32>> {
        (0..g.len() as u32).map(|a| g.next(a).collect()).collect()
    }

    /// 0 - 1 - 2 in a row with a shortcut 0 -> 3 -> 2 that is longer, and a
    /// one-way drop 2 -> 4.
    pub(in crate::nav) fn tiny() -> NavGraph {
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

    /// A row across x with a node ahead on the line of fire (+y) and one
    /// down a drop: the step goes across, to the nearest spot `clear` lets.
    #[test]
    fn a_sidestep_goes_across_the_line_on_level_edges() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(-48.0, 0.0, 0.0),
                Vec3::new(48.0, 0.0, 0.0),
                Vec3::new(96.0, 0.0, 0.0),
                Vec3::new(0.0, 48.0, 0.0),
                Vec3::new(-48.0, 8.0, -64.0),
            ],
            vec![
                vec![1, 2, 4],
                vec![0, 5],
                vec![0, 3],
                vec![2],
                vec![0],
                vec![],
            ],
        );
        let line = Vec3::Y;
        let all = |_: Vec3| true;
        let p = g.sidestep(Vec3::ZERO, line, all).unwrap();
        assert_eq!(p.y, 0.0, "stepped along the line: {p}");
        assert_eq!(p.x.abs(), 48.0);
        let east = g.sidestep(Vec3::ZERO, line, |p: Vec3| p.x > 0.0);
        assert_eq!(east, Some(Vec3::new(48.0, 0.0, 0.0)));
        // Two edges out, past the first.
        let far = g.sidestep(Vec3::ZERO, line, |p: Vec3| p.x > 60.0);
        assert_eq!(far, Some(Vec3::new(96.0, 0.0, 0.0)));
        // Never down the drop.
        assert_eq!(g.sidestep(Vec3::ZERO, line, |p: Vec3| p.z < 0.0), None);
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
        assert_eq!(tiny().component_ids(), &c[..]);
        assert!(c[0] == c[1] && c[1] == c[2] && c[2] == c[3]);
        assert_ne!(c[4], c[0], "the node below the drop is its own component");
    }

    /// Six nodes in a two-way row along +x, 32 apart.
    pub(in crate::nav) fn row() -> NavGraph {
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

    /// Node 0 to node 1 4000 units on, by a way `north` units off the line
    /// (nodes 2 to 9) or one `south` off it (10 to 17).
    pub(in crate::nav) fn two_ways(north: f32, south: f32) -> NavGraph {
        let mut nodes = vec![Vec3::ZERO, Vec3::new(4000.0, 0.0, 0.0)];
        let mut edges = vec![vec![], vec![]];
        for y in [north, -south] {
            let first = nodes.len() as u32;
            nodes.extend((0..8).map(|i| Vec3::new(250.0 + 500.0 * i as f32, y, 0.0)));
            edges[0].push(first);
            edges.extend((1..8).map(|i| vec![first + i]));
            edges.push(vec![1]);
        }
        NavGraph::from_parts(nodes, edges)
    }

    /// A two-way chain of nodes 0.125 over `test_world`'s floor: the grid's
    /// stair-step across a diagonal, east then north-east by turns.
    pub(in crate::nav) fn zigzag() -> NavGraph {
        let nodes: Vec<Vec3> = [(0, 0), (32, 0), (64, 32), (96, 32), (128, 64), (160, 64)]
            .iter()
            .map(|&(x, y)| Vec3::new(x as f32, y as f32, 0.125))
            .collect();
        let n = nodes.len() as u32;
        let edges = (0..n)
            .map(|i| {
                [i.wrapping_sub(1), i + 1]
                    .into_iter()
                    .filter(|&j| j < n)
                    .collect()
            })
            .collect();
        NavGraph::from_parts(nodes, edges)
    }

    /// A `side` x `side` two-way grid 32 apart, and one node 1000 units off
    /// it with no edges: an island no plan reaches.
    pub(in crate::nav) fn grid_and_island(side: u32) -> NavGraph {
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

    /// A jump edge stays a jump until a walk proves it without one, and a
    /// jump one way is no proof of a plain walk the other.
    #[test]
    fn a_plain_walk_clears_a_jump_edge() {
        let mut g = row();
        g.link_as(0, 2, true);
        assert!(g.has(0, 2, |k| k.jump));
        g.link_as(0, 2, true);
        assert!(g.has(0, 2, |k| k.jump), "a second jump keeps it a jump");
        g.link_as(0, 2, false);
        assert!(!g.has(0, 2, |k| k.jump));
        g.link_as(2, 4, false);
        g.link_as(2, 4, true);
        assert!(!g.has(2, 4, |k| k.jump), "a plain edge stays plain");
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

    /// From an island whose only ways off are drops, a roam picks in the
    /// largest component below; from there it never picks the pit the
    /// largest one drops into.
    #[test]
    fn a_roam_leaves_an_island_and_keeps_out_of_a_pit() {
        // 0-1 island, drops to 2; 2-5 a row; 5 drops to the pit 6.
        let nodes = (0..7)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let edges = vec![
            vec![1],
            vec![0, 2],
            vec![3],
            vec![2, 4],
            vec![3, 5],
            vec![4, 6],
            vec![],
        ];
        let g = NavGraph::from_parts(nodes, edges);
        let mut picks = (0..8).map(|i| i % 2);
        let d = g.roam_from(0, Vec3::ZERO, &mut || picks.next().unwrap());
        assert!(d.is_some_and(|d| (2..6).contains(&d)), "picked {d:?}");
        for i in 0..4 {
            let d = g.roam_from(5, Vec3::ZERO, &mut || i);
            assert!(d.is_some_and(|d| (2..6).contains(&d)), "picked {d:?}");
        }
    }

    /// A roam never ends over a fall that kills: mp_ship's crow's nest
    /// rails stand a body 2 units from the edge, and bots whose roam ended
    /// on one at a run went on off it (seeds 743, 1319). The rails are
    /// perches; the rim inside them is not, and a pick of a perch is passed
    /// over.
    #[test]
    fn a_roam_never_ends_over_a_fall_that_kills() {
        let nodes = (0..3)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.perches.insert(2);
        let mut picks = [2, 1].into_iter();
        assert_eq!(
            g.roam_from(0, Vec3::ZERO, &mut || picks.next().unwrap_or(0)),
            Some(1)
        );
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let node = |p: [f32; 3]| g.nearest(p).unwrap();
        for rail in [[5023.8, 123.7, 1254.125], [4928.5, 60.8, 1254.125]] {
            assert!(g.perches.contains(&node(rail)), "{rail:?}");
        }
        assert!(!g.perches.contains(&node([4963.9, 35.5, 1219.125])));
    }

    /// A roam from the graph's safe part never ends where only a risky
    /// edge leads: mp_ship's ledge at (3686, -354, 400), narrow over the
    /// hold at 92, is reached only by the risky back down from (3647, -353,
    /// 611), and three bots that roamed to it on seeds 2305-4607 missed it
    /// and died. A start already past such an edge roams as before. Since
    /// the air accelerate's floor (cod11-mantle.md, "The walk's accel
    /// floor") the drops onto (3647, -353, 611) read risky too; its deck
    /// neighbour at (3648, -317, 618) is in the safe part.
    #[test]
    fn a_roam_never_ends_where_only_a_risky_edge_leads() {
        let nodes = (0..3)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.edge_mut(1, 2).unwrap().risky = true;
        g.index_components();
        let mut picks = [2, 1].into_iter();
        assert_eq!(
            g.roam_from(0, Vec3::ZERO, &mut || picks.next().unwrap_or(0)),
            Some(1)
        );
        let mut picks = [0].into_iter();
        assert_eq!(
            g.roam_from(2, Vec3::ZERO, &mut || picks.next().unwrap_or(0)),
            Some(0)
        );
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let node = |p: [f32; 3]| g.nearest(p).unwrap();
        assert!(!g.safe[node([3647.6, -352.0, 400.125]) as usize]);
        assert!(g.safe[node([3647.9, -316.9, 618.5]) as usize]);
    }

    /// mp_ship's ladders at x 3354 (y 496-520) and x 3369 (y -368 to -379),
    /// climbed on their east side: a body on the rungs is nearest a floor
    /// behind its back, at 128 or 320, which no ladder move reaches. A plan
    /// from the rungs starts at the ladder's foot or head instead.
    #[test]
    fn a_plan_from_the_rungs_starts_at_a_ladders_end() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            MoveRules::V1_1,
        );
        let near = |n: Option<u32>, at: [f32; 3]| {
            n.is_some_and(|n| g.nodes[n as usize].distance(Vec3::from(at)) < 1.0)
        };
        let low = Vec3::new(3369.1, 495.1, 154.7);
        assert!(
            near(g.nearest(low.into()), [3423.4, 546.8, 128.7]),
            "behind the climber"
        );
        assert!(
            near(g.nearest_ladder_end(low), [3370.0, 508.0, 56.125]),
            "the foot"
        );
        let high = Vec3::new(3369.2, -379.1, 300.0);
        assert!(
            near(g.nearest(high.into()), [3399.1, -380.5, 320.125]),
            "behind the climber"
        );
        assert!(
            near(g.nearest_ladder_end(high), [3358.6, -384.2, 352.125]),
            "a head"
        );
    }

    /// A plan takes a risky drop only where there is no other way.
    #[test]
    fn a_plan_goes_round_a_risky_drop() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 200.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(0.0, 64.0, 200.0),
                Vec3::new(64.0, 64.0, 0.0),
            ],
            vec![vec![1, 2], vec![], vec![3], vec![1]],
        );
        assert_eq!(g.path(0, 1), Some(vec![0, 1]));
        g.edge_mut(0, 1).unwrap().risky = true;
        assert_eq!(g.path(0, 1), Some(vec![0, 2, 3, 1]), "round it");
        g.edges[2].clear();
        assert_eq!(g.path(0, 1), Some(vec![0, 1]), "the only way");
    }

    /// A drop from the main component into two nodes that reach only each
    /// other: a pit on its floor, not from the rim over it, nor with a
    /// spawn in it or over it that is cut off too.
    #[test]
    fn a_pair_dropped_into_with_no_way_out_is_a_pit() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(96.0, 0.0, -40.0),
                Vec3::new(96.0, 32.0, -40.0),
            ],
            vec![vec![1], vec![0, 2], vec![1, 3], vec![4], vec![3]],
        );
        assert!(g.pit([96.0, 16.0, -40.0], &[[0.0, 0.0, 8.0]]));
        assert!(!g.pit([32.0, 0.0, 0.0], &[]));
        // Over the pit, a storey up: not on its floor.
        assert!(!g.pit([96.0, 16.0, 0.0], &[]));
        // A spawn in it: a room to be in.
        assert!(!g.pit([96.0, 16.0, -40.0], &[[96.0, 32.0, -32.0]]));
        // A spawn on a ledge cut off over it, its one way a drop into it.
        let (mut nodes, mut edges) = (g.nodes.clone(), targets(&g));
        nodes.push(Vec3::new(128.0, 64.0, 0.0));
        edges.push(vec![4]);
        let g = NavGraph::from_parts(nodes, edges);
        assert!(!g.pit([96.0, 16.0, -40.0], &[[128.0, 64.0, 8.0]]));
        assert!(g.pit([96.0, 16.0, -40.0], &[[0.0, 0.0, 8.0]]));
    }

    #[test]
    fn a_node_that_reaches_no_node_of_the_largest_component_is_cut_off() {
        assert_eq!(tiny().cut_off(), vec![4]);
    }

    /// mp_ship's corners at (5375.1, 321.1, 352.1) and (5375.1, -193.1,
    /// 352.1): at rest against the shelf's face, the run at the node
    /// west-south-west (north-west) does not move the body; due west frees
    /// it.
    /// mp_ship's boat at (5250, -500, 27): a dish off the graph that a bot
    /// falls into from the deck at 375, walled off from the deck at 56
    /// beside it, its way out a drop over its rim to the floor at -64.
    /// The deck nodes score nearest; the plan starts under the rim.
    #[test]
    fn a_body_in_a_pit_off_the_graph_plans_from_the_floor_over_its_rim() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let deck = [
            Vec3::new(5280.0, -408.9, 56.125),
            Vec3::new(5312.2, -408.2, 56.125),
            Vec3::new(5344.0, -446.6, 56.125),
        ];
        let floor = Vec3::new(5279.3, -483.9, -63.875);
        let nodes: Vec<Vec3> = deck.iter().copied().chain([floor]).collect();
        let g = NavGraph::from_parts(nodes, vec![vec![]; 4]);
        let body = Vec3::new(5270.0, -450.0, 26.5);
        assert!(g.off_floor(body));
        assert_eq!(g.nearest(body.into()), Some(0));
        assert_eq!(g.nearest_walkable(&world.collision, body), Some(3));
    }

    /// mp_ship's lip at (5424, -187, 344): the 396 shelf's side and the
    /// slope under it stand between a body there and the floor at 352
    /// beyond, the nearest nodes by height; the stair node at 320 is a
    /// walk off the lip.
    #[test]
    fn a_plan_starts_at_a_node_the_body_walks_to() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let nodes = vec![
            Vec3::new(5472.0, -125.98952, 352.125),
            Vec3::new(5440.0015, -125.48199, 352.125),
            Vec3::new(5503.9976, -125.821175, 352.125),
            Vec3::new(5407.997, -125.86806, 352.125),
            Vec3::new(5536.001, -125.32347, 352.125),
            Vec3::new(5413.697, -193.1254, 320.125),
        ];
        let g = NavGraph::from_parts(nodes, vec![vec![]; 6]);
        let lip = Vec3::new(5469.5, -176.1, 344.125);
        assert_eq!(g.nearest(lip.into()), Some(0));
        assert_eq!(g.nearest_walkable(&world.collision, lip), Some(5));
    }

    #[test]
    fn nearest_prefers_the_floor_under_the_feet() {
        let mut g = tiny();
        // A node straight above node 1, one storey up.
        g = NavGraph::from_parts(
            [g.nodes.clone(), vec![Vec3::new(32.0, 0.0, 128.0)]].concat(),
            [targets(&g), vec![vec![]]].concat(),
        );
        assert_eq!(g.nearest([30.0, 2.0, 10.0]), Some(1));
        assert_eq!(g.nearest([30.0, 2.0, 140.0]), Some(5));
        assert_eq!(g.nearest([5000.0, 0.0, 0.0]), None);
    }
}
