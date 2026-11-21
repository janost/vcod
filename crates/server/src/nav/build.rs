//! Building the graph: the flood out of the spawns and the passes that
//! prove, link and drop its edges, ladders among them.

use super::*;

/// Grid pitch bounds in world units. The finest is about a capsule width
/// plus the slack a doorway leaves; a large map coarsens toward the other
/// to keep its node count near [`TARGET_CELLS`] (bot-navigation.md,
/// "Spacing"). At 64 mp_hurtgen's Retrieval bunker, the objective's room,
/// came out as islands its stairs never joined.
pub(super) const SPACING_MIN: f32 = 32.0;

pub(super) const SPACING_MAX: f32 = 48.0;

/// The cells the spawn points' bounding box is cut into.
const TARGET_CELLS: f32 = 20_000.0;

/// How far past the spawn points' bounding box the flood goes. Stock maps
/// put spawns at the edges of play; past this lies the scenery behind the
/// boundary (mp_hurtgen's forest is ~2000 units of it).
const MARGIN: f32 = 512.0;

/// Two nodes in one grid column are one node when their floors are closer
/// than this; a full storey is ~128.
pub(super) const Z_MERGE: f32 = 40.0;

/// A walk counts as arrived this close to its target, horizontally. One 50
/// ms tick at run speed covers 9.5 units, so the walk cannot step over it.
/// A walk bound for a node's floor also ends this close to its height
/// (bot-navigation.md, "The flood": mp_ship's hull beam).
pub(super) const ARRIVE: f32 = 8.0;

/// A walk whose fall, from the highest the body rose to where it lands,
/// is deeper than this is refused: 6 under `bg_fallDamageMinHeight`
/// ([`falls`]; 256 in 1.1d's and UO's `gameCvarTable`, so 250), past
/// which the fall hurts. mp_cassino's allied upper floor leaves by a
/// window sill 240 over the street (bot-navigation.md, "Other rules").
pub(super) fn max_drop() -> f32 {
    falls().min - 6.0
}

/// [`max_drop`] for a walk set off at a body's own pace, the follower's
/// proof again from where a bot stands, read in the air from where the
/// body last stood: a bot moving onto a drop deeper than this, four
/// fifths of it (200 under CoD1's heights), is held to take it from rest.
/// mp_ship's ledge 211 under the
/// deck at (3647, -352.9, 611.1) lands a back down at the 190 a bot came
/// along the deck with, and a bot that did fell past it to the hold
/// (bot-navigation.md section 3).
pub(super) fn paced_drop() -> f32 {
    max_drop() * 4.0 / 5.0
}

/// How long [`coasts_off`] lets a body coast: 190 units/s coasts to rest
/// in about 35 units and half a second.
pub(super) const COAST_TICKS: usize = 16;

/// How high over a drop's landing [`a_miss_hurts`] looks out from.
pub(super) const MISS_EYE: f32 = 48.0;

/// How far either side of the body a run off a drop, or a run on that
/// leaves the ground, whose miss can hurt has to land from too
/// ([`Follower::edge_holds`]).
pub(super) const ASIDE_DROP: f32 = 2.0;

/// An edge whose ends differ by less than this in height is level, and a
/// walk one way stands in for the walk back.
pub(super) const LEVEL: f32 = 1.0;

/// The flood stops here; a stock map fills well under it. United
/// Offensive's largest, mp_rhinevalley, fills 146 000 (bot-navigation.md,
/// "Other rules").
const MAX_NODES: usize = 200_000;

/// The longest a walk runs with a ladder in it: 15 s, ~1400 units of climb
/// at the 95 units/s a full-rate climb makes. mp_ship's hold ladder is 560.
pub(super) const LADDER_TICKS: usize = 300;

/// Degrees up a climbing body looks; past ~9 deg `ladder_move`'s upscale
/// saturates. Shared with the bots, which climb the same way.
pub const LADDER_PITCH: f32 = 45.0;

/// A* gives up past this many expansions, or the graph's node count when
/// that is more, and reports no path.
pub(super) const MAX_EXPANSIONS: usize = 40_000;

/// How far a bot steps aside to clear a teammate off its line of fire.
pub const SIDESTEP_MIN: f32 = 24.0;

pub const SIDESTEP_MAX: f32 = 128.0;

/// [`NavGraph::link_pockets`]: the rounds it floods on from the floors it
/// finds, the headings it runs each cut-off node in, and how far.
const POCKET_ROUNDS: usize = 3;

const POCKET_YAWS: usize = 32;

const POCKET_REACH: [f32; 2] = [48.0, 80.0];

const CARDINALS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

const DIAGONALS: [(i32, i32); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];

impl NavGraph {
    /// Floods the grid from `seeds` (feet origins, typically every spawn
    /// point of the map) and keeps an edge only where a standing run, one
    /// usercmd per 50 ms through the shared cmd step under `rules`,
    /// arrives. No node stands where a body would touch one of `hazards`.
    /// The flood also reaches [`MARGIN`] past each of `cover`
    /// ([`crate::world::World::nav_cover`]), which leaves the pitch alone.
    pub fn build(
        world: &CollisionWorld,
        seeds: &[[f32; 3]],
        cover: &[[f32; 3]],
        hazards: &[BrushHull],
        rules: MoveRules,
    ) -> NavGraph {
        Self::build_with(world, seeds, cover, hazards, rules, None, None)
    }

    /// [`Self::build`], with the mantle edges `mantle` finds
    /// ([`Self::link_mantles`]) and the floors past them flooded too, the
    /// bodies moving by `step` when one is given ([`with_step`]).
    pub fn build_with(
        world: &CollisionWorld,
        seeds: &[[f32; 3]],
        cover: &[[f32; 3]],
        hazards: &[BrushHull],
        rules: MoveRules,
        mantle: Option<&MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
    ) -> NavGraph {
        Self::build_opts(world, seeds, cover, hazards, rules, mantle, step, true)
    }

    /// [`Self::build_with`] for bots that read a leap's lip (`lip`) or
    /// not. A bot whose route never reads the lip
    /// (`BotWorld::PMOVE_PROBES` off, CoD2's and CoD4's) never jumps on a
    /// leap edge and runs off its foot: off mp_killhouse's crate tops at z
    /// 60 it fell to the floor at 28 again and again (bot-objectives.md
    /// 12.14), and paced mp_decoy's pit under its leaps out (11.8).
    /// Without the lip a leap is kept only as a plain jump edge its bot's
    /// own jump lands ([`Self::reprove_jumps`]). The same graph also drops
    /// the edges past the follower's reach
    /// ([`Self::drop_out_of_reach`]) and probes for mantles again off the
    /// floors the last mantles' tops led to.
    pub fn build_opts(
        world: &CollisionWorld,
        seeds: &[[f32; 3]],
        cover: &[[f32; 3]],
        hazards: &[BrushHull],
        rules: MoveRules,
        mantle: Option<&MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
        lip: bool,
    ) -> NavGraph {
        with_rules(rules, || {
            with_step(step, || {
                Self::build_here(world, seeds, cover, hazards, mantle, lip)
            })
        })
    }

    fn build_here(
        world: &CollisionWorld,
        seeds: &[[f32; 3]],
        cover: &[[f32; 3]],
        hazards: &[BrushHull],
        mantle: Option<&MantleProbe>,
        lip: bool,
    ) -> NavGraph {
        // The pitch cuts the spawns' box into about TARGET_CELLS, rounded up
        // to a multiple of 16; the flood stays within MARGIN of the box and
        // of `cover`.
        let (lo, hi) = seed_bounds(seeds);
        let area = (hi - lo).max(glam::Vec2::ZERO).element_product();
        let spacing = ((area / TARGET_CELLS).sqrt() / 16.0).ceil() * 16.0;
        let (clo, chi) = seed_bounds(cover);
        let (lo, hi) = (
            lo.min(clo) - glam::Vec2::splat(MARGIN),
            hi.max(chi) + glam::Vec2::splat(MARGIN),
        );
        let inside = |p: Vec3| {
            let p = p.truncate();
            p.cmpge(lo).all() && p.cmple(hi).all()
        };
        let mut g = NavGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            perches: HashSet::new(),
            seeds: Vec::new(),
            spacing: spacing.clamp(SPACING_MIN, SPACING_MAX),
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            safe: Vec::new(),
            ladder_boxes: ladders(world),
            rules: RULES.get(),
            falls: falls(),
            step: own_step(),
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
            match g.find(c, p.z) {
                Some(n) => g.seeds.push(n),
                None => {
                    let n = g.add(c, p);
                    g.seeds.push(n);
                    queue.push(n);
                }
            }
        }
        g.flood(world, hazards, &inside, queue);
        let ends = g.link_ladders(world, hazards);
        g.flood(world, hazards, &inside, ends);
        if let Some(probe) = mantle {
            // A graph with `lip` off (CoD2's, CoD4's) probes again from
            // the nodes each flood off the tops found, until one finds
            // none: mp_bloc's
            // pool is reached only down off a mantle's top, and its way
            // out is a mantle (bot-objectives.md 12.16).
            let mut probed = 0;
            loop {
                let before = g.nodes.len();
                let tops = g.link_mantles(world, hazards, &inside, probe, probed);
                g.flood(world, hazards, &inside, tops);
                probed = before;
                if lip || g.nodes.len() == probed {
                    break;
                }
            }
        }
        g.link_back(world);
        g.link_leaps(world);
        if !lip {
            g.edges.iter_mut().flatten().for_each(|e| e.leap = false);
        }
        g.reprove_jumps(world);
        if !lip {
            g.drop_out_of_reach();
        }
        g.link_pockets(world, hazards, &inside);
        g.mark_risky(world);
        g.mark_perches(world);
        g.index_components();
        g
    }

    /// Marks [`Edge::risky`]: every walk down off a jump edge beside a fall
    /// that hurts ([`beside_a_fall_that_hurts`]) whose forward walk falls
    /// from 4 units aside: bots that passed mp_ship's roof node
    /// (3514, -283, 623) a few units off the line flew off the ridge past
    /// (3496, -318, 611) to the hold. And every drop deeper than a jump off
    /// a jump edge, walked again in the first gait that
    /// lands it from its node, from 4 units aside each way. mp_ship's ledge
    /// at (3685.5, -354.3, 400.1), narrow over the hold at 92, lands the
    /// back down from (3647, -352.9, 611.1) and misses from 4 units south;
    /// bots that came onto it fell past the ledge whatever they were held
    /// to (bot-navigation.md section 3).
    fn mark_risky(&mut self, world: &CollisionWorld) {
        let drops: Vec<(u32, u32)> = self
            .edges_where(|e| !e.jump && !e.mantle)
            .filter(|&(a, b)| self.nodes[b as usize].z < self.nodes[a as usize].z - LEVEL)
            .collect();
        let risky = par_map(&drops, |&(a, b)| {
            let (p, q) = (self.nodes[a as usize], self.nodes[b as usize]);
            if q.z >= p.z - vcod_common::pmove::JUMP_HEIGHT {
                return beside_a_fall_that_hurts(world, q)
                    && [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y].iter().any(|&d| {
                        let aside = p + d * RISK_ASIDE;
                        let from = settle(world, aside, 0.0).unwrap_or(aside);
                        matches!(
                            walk_as(world, from, q.truncate(), Some(q.z), Gait::Forward),
                            Walked::Fell
                        )
                    });
            }
            // A drop a fall off kills that lands only by a grab of the rungs
            // on the way down, from a node not over them: mp_ship's crow's
            // nest exits back off the rim into the hatch, and bots that came
            // onto the rungs off its line slid off them (bot-navigation.md
            // section 3).
            if q.z < p.z - deadly_fall()
                && self
                    .ladder_boxes
                    .iter()
                    .all(|b| box_flat(b, p) > vcod_common::pmove::HALF_WIDTH)
            {
                return true;
            }
            let lands = |from: Vec3, gait| {
                matches!(
                    walk_as(world, from, q.truncate(), Some(q.z), gait),
                    Walked::Arrived(..)
                )
            };
            // No gait lands it node to node: the edge stands on a walk that
            // ran from beside the node (mp_ship's deck at (3515, -323, 631)
            // over the ledge at 580, which bots ran off and fell past).
            let Some(gait) = [Gait::Forward, Gait::Backward, Gait::Creep]
                .into_iter()
                .find(|&gait| lands(p, gait))
            else {
                return true;
            };
            [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y].iter().any(|&d| {
                let aside = p + d * RISK_ASIDE;
                let from = settle(world, aside, 0.0).unwrap_or(aside);
                matches!(
                    walk_as(world, from, q.truncate(), Some(q.z), gait),
                    Walked::Fell
                )
            })
        });
        self.edges
            .iter_mut()
            .flatten()
            .for_each(|e| e.risky = false);
        for ((a, b), r) in drops.into_iter().zip(risky) {
            if let Some(e) = self.edge_mut(a, b) {
                e.risky = r;
            }
        }
    }

    /// Links every node from `from` on to where a mantle off it lands, by
    /// `probe` facing each of the eight compass ways, as a mantle edge
    /// ([`Edge::mantle`]) unless a walk already proved the edge; a landing
    /// no node stands at gets one, and the new nodes are returned for the
    /// flood to go on from.
    fn link_mantles(
        &mut self,
        world: &CollisionWorld,
        hazards: &[BrushHull],
        inside: &dyn Fn(Vec3) -> bool,
        probe: &MantleProbe,
        from: usize,
    ) -> Vec<u32> {
        let found = par_map(&self.nodes[from..], |&p| {
            (0..8)
                .filter_map(|k| probe(world, p, k as f32 * 45.0))
                .collect::<Vec<Vec3>>()
        });
        let mut new = Vec::new();
        for (a, lands) in found.into_iter().enumerate() {
            let a = (from + a) as u32;
            for q in lands {
                let c = self.column(q);
                let b = match self.find(c, q.z) {
                    Some(b) => b,
                    None if inside(q) && !hazard(hazards, q) && self.nodes.len() < MAX_NODES => {
                        let b = self.add(c, q);
                        new.push(b);
                        b
                    }
                    None => continue,
                };
                if a != b && !self.linked(a, b) {
                    self.edges[a as usize].push(Edge {
                        mantle: true,
                        land: Some(q),
                        ..Edge::to(b)
                    });
                }
            }
        }
        new
    }

    /// Marks [`Self::perches`]: nodes with no floor under their middle
    /// within a fall that kills ([`deadly_fall`]). mp_ship's crow's nest
    /// rails stand a body 2 units from the edge: bots whose roam ended on
    /// one at a run, the next plan a few ticks off, ran on off the rail to
    /// the deck (bot-navigation.md section 3).
    fn mark_perches(&mut self, world: &CollisionWorld) {
        let deadly = deadly_fall();
        let over = par_map(&self.nodes, |&p| {
            let down = p - Vec3::Z * deadly;
            world
                .point_trace(p + Vec3::Z, down, MASK_PLAYERSOLID, false)
                .fraction
                >= 1.0
        });
        self.perches = (0..self.nodes.len() as u32)
            .zip(over)
            .filter_map(|(n, o)| o.then_some(n))
            .collect();
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
                // A rest on a ladder's top is no node: only its exits lead
                // on from there (`link_ladders`).
                dirs.map(|(dx, dy)| {
                    self.step(world, n, (cx + dx, cy + dy))
                        .filter(|&(p, _)| !perched(world, p))
                })
            });
            let mut next = Vec::new();
            let mut recheck = Vec::new();
            for (&(n, dirs), ends) in jobs.iter().zip(ends) {
                let (cx, cy) = self.column(self.nodes[n as usize]);
                for (&(dx, dy), to) in dirs.iter().zip(ends) {
                    let Some((to, jumped)) = to else {
                        continue;
                    };
                    let c = (cx + dx, cy + dy);
                    let from = self.nodes[n as usize];
                    let m = match self.find(c, to.z) {
                        // A rest off the column's node's floor proves no
                        // edge to it: walk to the node itself.
                        Some(m) if (self.nodes[m as usize].z - to.z).abs() >= ARRIVE => {
                            recheck.push((n, m, c, to, jumped, false));
                            continue;
                        }
                        // Nor does one beside a node off the column's centre
                        // at the foot of a drop deeper than a jump: the
                        // landing can be narrow (bot-navigation.md section 3).
                        Some(m)
                            if to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
                                && (self.nodes[m as usize] - to).truncate().length() > 1.0 =>
                        {
                            recheck.push((n, m, c, to, jumped, true));
                            continue;
                        }
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
            let walked = par_map(&recheck, |&(n, m, ..)| {
                let to = self.nodes[m as usize];
                let from = self.nodes[n as usize];
                walk_to(world, &self.ladder_boxes, from, to.truncate(), Some(to.z))
            });
            for (&(n, m, c, to, jumped, beside), w) in recheck.iter().zip(walked) {
                if let Some((_, jumped)) = w {
                    self.link_as(n, m, jumped);
                    continue;
                }
                if beside {
                    continue;
                }
                // Then the rest is a floor of its own.
                let m = match self.find(c, to.z) {
                    Some(m) if (self.nodes[m as usize].z - to.z).abs() < ARRIVE => m,
                    _ if self.nodes.len() < MAX_NODES && inside(to) && !hazard(hazards, to) => {
                        let m = self.add(c, to);
                        next.push(m);
                        m
                    }
                    _ => continue,
                };
                self.link_as(n, m, jumped);
            }
            if !frontier.is_empty() {
                late.push_back(frontier);
            }
            frontier = next;
        }
    }

    /// The flood aims at column centres, so a way out through a gap off
    /// every lattice line is never walked: mp_streets' allied rooms at
    /// (-260, -6900, 8) leave only by a window whose sill a standing body
    /// fits on within 2 units of height and 16 across. From every node
    /// that reaches no node of the largest component, runs in
    /// [`POCKET_YAWS`] headings to [`POCKET_REACH`] away; a rest on a floor
    /// no node holds becomes a node and floods on, and one on another
    /// node's floor is walked again node to node (bot-navigation.md,
    /// "Other rules").
    fn link_pockets(
        &mut self,
        world: &CollisionWorld,
        hazards: &[BrushHull],
        inside: &dyn Fn(Vec3) -> bool,
    ) {
        for _ in 0..POCKET_ROUNDS {
            let cut = self.cut_off();
            if cut.is_empty() {
                return;
            }
            let pocket: HashSet<u32> = cut.iter().copied().collect();
            let jobs: Vec<(u32, glam::Vec2)> = cut
                .iter()
                .flat_map(|&n| {
                    let p = self.nodes[n as usize].truncate();
                    (0..POCKET_YAWS).flat_map(move |i| {
                        let a = (i as f32 * 360.0 / POCKET_YAWS as f32).to_radians();
                        let dir = glam::Vec2::new(a.cos(), a.sin());
                        POCKET_REACH.map(|r| (n, p + dir * r))
                    })
                })
                .collect();
            // A rest proves a way only where the run aimed at the rest itself,
            // as a bot following the edge aims, arrives there too. A run
            // that arrives no other way is tried again crouching from its
            // jump on: some rooms leave only by a window a standing body
            // does not fit through.
            let ends = par_map(&jobs, |&(n, t)| {
                let from = self.nodes[n as usize];
                [Gait::Forward, Gait::CrouchJump]
                    .into_iter()
                    .find_map(|gait| match walk_as(world, from, t, None, gait) {
                        Walked::Arrived(q, _)
                            if !perched(world, q)
                                && (gait == Gait::Forward || stands(world, q)) =>
                        {
                            match walk_as(world, from, q.truncate(), Some(q.z), gait) {
                                Walked::Arrived(_, jumped) => {
                                    Some((q, jumped, gait == Gait::CrouchJump && jumped))
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    })
            });
            let mut fresh = Vec::new();
            let mut recheck = Vec::new();
            for (&(n, _), end) in jobs.iter().zip(ends) {
                let Some((q, jumped, crouched)) = end else {
                    continue;
                };
                let c = self.column(q);
                match self.find(c, q.z) {
                    Some(m) if (self.nodes[m as usize].z - q.z).abs() < ARRIVE => {
                        if !pocket.contains(&m) && !self.linked(n, m) {
                            recheck.push((n, m));
                        }
                    }
                    _ if self.nodes.len() < MAX_NODES && inside(q) && !hazard(hazards, q) => {
                        let m = self.add(c, q);
                        self.link_crouched(n, m, jumped, crouched);
                        fresh.push(m);
                    }
                    _ => {}
                }
            }
            recheck.sort_unstable();
            recheck.dedup();
            let walked = par_map(&recheck, |&(n, m)| {
                let (from, to) = (self.nodes[n as usize], self.nodes[m as usize]);
                walk_to(world, &self.ladder_boxes, from, to.truncate(), Some(to.z))
                    .map(|(_, jumped)| (jumped, false))
                    .or_else(|| {
                        match walk_as(world, from, to.truncate(), Some(to.z), Gait::CrouchJump) {
                            Walked::Arrived(_, true) => Some((true, true)),
                            _ => None,
                        }
                    })
            });
            let mut linked = false;
            for (&(n, m), w) in recheck.iter().zip(walked) {
                if let Some((jumped, crouched)) = w {
                    self.link_crouched(n, m, jumped, crouched);
                    linked = true;
                }
            }
            if fresh.is_empty() && !linked {
                return;
            }
            self.flood(world, hazards, inside, fresh);
        }
    }

    /// The flood walks to a neighbouring column only, so a ledge across a
    /// gap is never tried: mp_depot's Retrieval documents lie on a crate top
    /// 32 units over a step, a 32-unit gap between them. Every node up to two
    /// columns off and between a step and a jump higher, that three edges
    /// don't already reach, is run at from a standstill with a jump at the
    /// lip ([`Gait::Leap`]). A ladder's head is no foot: a body comes to
    /// rest there where the climb tips it over, a few units off the spot
    /// the leap would be proved from (mp_ship's hold ladder, section 2
    /// "Jumps").
    pub(super) fn link_leaps(&mut self, world: &CollisionWorld) {
        use vcod_common::pmove::{JUMP_HEIGHT, STEPSIZE};
        let mut heads: HashSet<u32> = HashSet::new();
        for (a, out) in self.edges.iter().enumerate() {
            let z = self.nodes[a].z + JUMP_HEIGHT;
            heads.extend(
                out.iter()
                    .map(|e| e.to)
                    .filter(|&b| self.nodes[b as usize].z > z),
            );
        }
        let mut jobs = Vec::new();
        for (a, &p) in self.nodes.iter().enumerate() {
            if heads.contains(&(a as u32)) {
                continue;
            }
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
            if ok && !self.linked(a, b) {
                self.edges[a as usize].push(Edge {
                    jump: true,
                    leap: true,
                    ..Edge::to(b)
                });
            }
        }
    }

    /// Walks every jump edge but the leaps again node to node, aimed at
    /// its top, where a bot steers, jumping as a bot does ([`as_a_bot`]).
    /// The flood proved its jumps aimed at a column's centre and
    /// [`Self::link_back`] some 8 units aside, and from the node such a walk
    /// can bob under the top and never land: a bot in mp_depot's pit at
    /// (-1569, 2087, 212) jumped at the step at (-1533, 2112, 239) again and
    /// again (bot-navigation.md, "Jumps"). An edge that lands only aimed
    /// [`SIDESTEP`] to one side, or at its column's centre, where the flood
    /// aimed, keeps that aim ([`Edge::aim`]); one that lands at none is
    /// dropped.
    pub(super) fn reprove_jumps(&mut self, world: &CollisionWorld) {
        let mut jobs: Vec<(u32, u32)> = self.edges_where(|e| e.jump && !e.leap).collect();
        jobs.sort_unstable();
        let walked = par_map(&jobs, |&(a, b)| {
            let (from, to) = (self.nodes[a as usize], self.nodes[b as usize]);
            let across = (to - from).truncate().perp().normalize_or_zero() * SIDESTEP;
            let at = to.truncate();
            let centre = self.center(self.column(to));
            [at, at + across, at - across, centre]
                .into_iter()
                .find_map(|target| {
                    let w = as_a_bot(|| walk_as(world, from, target, Some(to.z), Gait::Forward));
                    matches!(w, Walked::Arrived(..)).then_some((target != at).then_some(target))
                })
        });
        for (&(a, b), aim) in jobs.iter().zip(walked) {
            match aim {
                Some(Some(p)) => {
                    if let Some(e) = self.edge_mut(a, b) {
                        e.aim = Some(p);
                    }
                }
                Some(None) => {}
                None => self.edges[a as usize].retain(|e| e.to != b),
            }
        }
    }

    /// Drops every edge but a mantle whose top lies more than
    /// [`OFF_PATH_STEPS`] grid steps across from its foot: a follower
    /// reads a waypoint that far off as a body knocked off its path and
    /// plans again, so it never sets off on one. mp_crash's roof at
    /// (64, -643, 313) has jumps 158 across to (160, -518, 304), the
    /// way back along its mantles (`NavGraph::link_back`); a bot there
    /// planned the jump, dropped it at its foot and planned it again for
    /// the rest of a `koth` match (bot-objectives.md 12.15).
    fn drop_out_of_reach(&mut self) {
        let reach = OFF_PATH_STEPS * self.spacing;
        for a in 0..self.edges.len() {
            let p = self.nodes[a];
            let mut out = std::mem::take(&mut self.edges[a]);
            out.retain(|e| {
                e.mantle || (self.nodes[e.to as usize] - p).truncate().length() <= reach
            });
            self.edges[a] = out;
        }
    }

    /// Where a bot on edge `a -> b` steers: `b`'s spot, or the point
    /// beside it the edge's jump was proved at ([`Edge::aim`]).
    pub fn aim(&self, a: u32, b: u32) -> glam::Vec2 {
        self.edge(a, b)
            .and_then(|e| e.aim)
            .unwrap_or_else(|| self.nodes[b as usize].truncate())
    }

    /// The nodes `hops` edges or fewer from `a`.
    fn within(&self, a: u32, hops: usize) -> HashSet<u32> {
        let mut seen = HashSet::from([a]);
        let mut layer = vec![a];
        for _ in 0..hops {
            let mut next = Vec::new();
            for n in layer {
                for m in self.next(n) {
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
            for &Edge { to: b, .. } in out {
                let q = self.nodes[b as usize];
                if (q.z - p.z).abs() < Z_MERGE && !self.linked(b, a as u32) {
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
        let rungs = par_map(&faces, |&(lo, hi, n)| {
            rung(world, lo, hi, n).map(|r| (r, ladder_exits(world, r.1, n)))
        });
        let mut ends = Vec::new();
        let safe = |r: &(Vec3, Vec3, bool, bool)| !hazard(hazards, r.0) && !hazard(hazards, r.1);
        for ((foot, head, up, down), exits) in rungs.into_iter().flatten().filter(|(r, _)| safe(r))
        {
            let f = self.add(self.column(foot), foot);
            let h = self.add(self.column(head), head);
            if up {
                self.link(f, h);
            }
            if down {
                self.link(h, f);
            }
            ends.push(f);
            // A head perched on the ladder's top leads only to the floor
            // its exits walked to: the walks across a hatch from the perch
            // arrive or fall by a few units (bot-navigation.md, "Ladders").
            if exits.is_empty() {
                ends.push(h);
            }
            for (exit, back) in exits.into_iter().filter(|&(e, _)| !hazard(hazards, e)) {
                let e = self.add(self.column(exit), exit);
                self.link(h, e);
                if back {
                    self.link(e, h);
                }
                ends.push(e);
            }
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
}

/// The world's ladders as axial boxes: the `SURF_LADDER` brushes of the
/// world model and of every submodel linked where it spawned (mp_italy's
/// allied `bas` bunker is left by the two in its base's `*4`,
/// bot-navigation.md section 2), and a world clip's (CoD2's, which keeps
/// model 0's brushes out of [`CollisionWorld::brushes`]), with touching
/// ones merged, since a tall ladder is often several brushes stacked.
pub(super) fn ladders(world: &CollisionWorld) -> Vec<(Vec3, Vec3)> {
    let mut boxes: Vec<(Vec3, Vec3)> = world
        .brushes
        .iter()
        .filter(|b| b.surface_flags & vcod_common::collision::SURF_LADDER != 0)
        .filter(|b| world.model_in_place(b.model as usize))
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
        .chain(world.world_clip_ladders())
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
    let probe = body(lo, 0.0);
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
    let mut ps = body(foot, yaw);
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
    // The creep from rest too: off a head on a spar beside the rungs' top
    // the running start carries the body out of their reach before it has
    // dropped the 8 under the top they hold from (mp_ship's mast at
    // (3675, -86.5), section 3).
    let down = arrives(head, foot, Gait::Creep)
        || matches!(
            walk_with(
                world,
                head,
                Some(Vec3::ZERO),
                foot.truncate(),
                Some(foot.z),
                Gait::Creep
            ),
            Walked::Arrived(_, false)
        );
    (up || down).then_some((foot, head, up, down))
}

/// Whether a body standing at `p` stands on a ladder brush's top with no
/// other ground under its middle: mp_ship's ladder plates come up through
/// hatches, and a body pinned at the top stands on the plate's 1-unit edge
/// over the hole.
fn perched(world: &CollisionWorld, p: Vec3) -> bool {
    let ps = body(p, 0.0);
    let t = world.box_trace(p + Vec3::Z, p - Vec3::Z, ps.mins(), ps.maxs());
    t.fraction < 1.0
        && t.surface_flags & vcod_common::collision::SURF_LADDER != 0
        && !footed(world, p)
}

/// Whether a standing body fits at `p`: a crouched jump's rest on a
/// window's sill, where it cannot stand, is no node; a walk from a node
/// starts standing.
pub(super) fn stands(world: &CollisionWorld, p: Vec3) -> bool {
    let ps = body(p, 0.0);
    !world.box_trace(p, p, ps.mins(), ps.maxs()).startsolid
}

/// Whether a body standing at `p` has ground right under its middle, not
/// a ladder's: the floor beside a hatch, not the hole's rim.
fn footed(world: &CollisionWorld, p: Vec3) -> bool {
    let down = p - Vec3::Z * vcod_common::pmove::STEPSIZE;
    let t = world.point_trace(p + Vec3::Z, down, MASK_PLAYERSOLID, false);
    t.fraction < 1.0 && t.surface_flags & vcod_common::collision::SURF_LADDER == 0
}

/// Where a [`perched`] ladder head walks off along the face, either way,
/// onto its own floor with ground under its middle ([`footed`]): `(exit, back)`,
/// `back` when the walk from the exit to the head arrives too. Empty for a
/// head that is not perched or has no such floor beside it.
fn ladder_exits(world: &CollisionWorld, head: Vec3, n: Vec3) -> Vec<(Vec3, bool)> {
    if !perched(world, head) {
        return Vec::new();
    }
    let side = glam::Vec2::new(-n.y, n.x);
    [side, -side]
        .into_iter()
        .filter_map(|dir| {
            EXIT_STEPS.iter().find_map(|&d| {
                let to = head.truncate() + dir * d;
                match walk_as(world, head, to, Some(head.z), Gait::Forward) {
                    Walked::Arrived(p, false) if footed(world, p) => Some(p),
                    _ => None,
                }
            })
        })
        .filter(|&p| !under_ground(world, p))
        .map(|p| {
            let back = walk_as(world, p, head.truncate(), Some(head.z), Gait::Forward);
            (p, matches!(back, Walked::Arrived(_, false)))
        })
        .collect()
}

/// How far along the face a perched head's exit is looked for.
const EXIT_STEPS: [f32; 3] = [24.0, 32.0, 48.0];

/// Heights, 16 units apart from the ladder's bottom, a foot's drop is
/// tried from.
const FOOT_TRIES: usize = 6;

/// How far a climb walks on past the top of a ladder before it stops.
const LADDER_STEP_IN: f32 = 24.0;

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// mp_ship's roof south of the mast platform: the walk on from
    /// (3514, -283, 623) to the ridge node (3496, -318, 611), 12 down, falls
    /// from 4 units aside, and bots that passed the node off the line flew
    /// past the ridge to the hold (seeds 379, 457). It is risky; the level
    /// hull floor's walks are not.
    #[test]
    fn a_walk_down_beside_a_deadly_fall_that_misses_from_aside_is_risky() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let roof = g.nearest([3514.0, -283.0, 623.0]).unwrap();
        let ridge = g.nearest([3496.0, -318.0, 611.0]).unwrap();
        assert!(g.linked(roof, ridge));
        assert!(g.has(roof, ridge, |k| k.risky));
        let hull = g.nearest([3839.6, -611.7, 54.125]).unwrap();
        let below = g.nearest([3875.0, -578.4, 27.13]).unwrap();
        assert!(g.linked(hull, below));
        assert!(!g.has(hull, below, |k| k.risky));
    }

    /// mp_ship's crow's nest exits back off the rim into the hatch for the
    /// deck at 692, 527 down, landing only on a grab of the rungs: bots
    /// that came onto them off the line slid off and fell to the deck
    /// (seed 1019). Those drops are risky; the head's own way down the
    /// rungs is not.
    #[test]
    fn a_drop_that_kills_but_for_the_rungs_is_risky() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let node = |p: [f32; 3]| g.nearest(p).unwrap();
        let below = node([4929.0, 68.0, 692.125]);
        for exit in [[4963.9, 35.5, 1219.125], [4963.9, 92.5, 1219.125]] {
            let exit = node(exit);
            assert!(g.linked(exit, below));
            assert!(g.has(exit, below, |k| k.risky));
        }
        let (head, foot) = (
            node([4963.9, 64.0, 1219.125]),
            node([4959.0, 64.0, 692.125]),
        );
        assert!(g.linked(head, foot));
        assert!(!g.has(head, foot, |k| k.risky));
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

    /// mp_ship's ladders, each from its open face: up and down both proved
    /// by the bots' own steering. One plain, one whose head stands right
    /// above its foot in a shaft, one only the creep gets down, and five
    /// more. The climb under the mast's spar
    /// ends on the spar's edge at 1147 beside the plate, and the creep back
    /// off it at a run is 40 units out before it has dropped the 8 under the
    /// plate `PM_CheckLadderMove`'s box needs: up only.
    #[test]
    fn ships_ladders_are_climbed_both_ways() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let entry = fs
            .resolve_map("mp_ship")
            .expect("a stock map every install ships");
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let boxes = ladders(&world.collision);
        for (at, n, foot_z, head_z, want_down) in [
            ([5872.5, 47.0], Vec3::X, 352.125, 480.125, true),
            ([3711.5, 56.0], -Vec3::X, 760.125, 992.125, true),
            ([3792.5, 65.0], Vec3::X, 304.125, 616.125, true),
            // The hold's 560-unit ladder, the mast's under a spar, a crow's
            // nest: climbs past the old 10 s budget, or under a patch.
            ([4332.0, -77.5], -Vec3::Y, 56.125, 615.125, true),
            ([3675.0, -86.5], -Vec3::Y, 896.125, 1147.125, true),
            ([4975.5, 64.0], -Vec3::X, 692.125, 1219.125, true),
            // A brush running on below the floor, a wall crowding one edge.
            ([6487.0, 324.5], Vec3::Y, 80.125, 216.125, true),
            ([3600.5, -119.0], Vec3::X, 760.125, 896.125, true),
        ] {
            let &(lo, hi) = boxes
                .iter()
                .find(|(lo, hi)| (0..2).all(|i| lo[i] <= at[i] && at[i] <= hi[i]))
                .unwrap_or_else(|| panic!("no ladder at {at:?}"));
            let (foot, head, up, down) =
                rung(&world.collision, lo, hi, n).unwrap_or_else(|| panic!("{at:?}: no rung"));
            assert!((foot.z - foot_z).abs() < 1.0, "{at:?}: foot {foot}");
            assert!((head.z - head_z).abs() < 1.0, "{at:?}: head {head}");
            assert!(up && down == want_down, "{at:?}: up {up} down {down}");
            assert!(
                rung(&world.collision, lo, hi, -n).is_none(),
                "{at:?}: the back"
            );
        }
    }

    pub(in crate::nav) fn map_world(map: &str) -> Option<crate::world::World> {
        let fs = vcod_common::testing::game_fs()?;
        let entry = fs.resolve_map(map)?;
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        Some(crate::world::World::from_bsp(&bsp, Some(&fs)))
    }

    /// mp_ship's deck at (3647.9, -316.9, 618.5): the flood's back down
    /// toward the ledge column's centre (3680, -352) landed beside the
    /// node already there at (3685.5, -354.3, 400.1), and no walk from the
    /// one to the other lands; bots that took the edge died in the hold
    /// under it. A drop's rest beside a node off its column's centre is
    /// walked to the node itself.
    #[test]
    fn a_drop_beside_a_node_off_its_columns_centre_is_walked_to_the_node() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let node = |p: [f32; 3]| {
            g.nearest(p)
                .filter(|&n| g.nodes[n as usize].distance(Vec3::from(p)) < 1.0)
                .unwrap_or_else(|| panic!("no node at {p:?}"))
        };
        let ledge = node([3685.5, -354.3, 400.125]);
        let north = node([3647.9, -316.9, 618.5]);
        let west = node([3647.0, -352.9, 611.125]);
        assert!(!g.linked(north, ledge), "from the north");
        assert!(g.linked(west, ledge), "from the west");
        assert!(g.has(west, ledge, |k| k.risky), "missed from 4 units south");
    }

    /// mp_ship's mast hatch: the climb's head stands on the ladder plate's
    /// 1-unit top over the hole, and the hatch's floor lies either side of
    /// the plate along the face. The mast's own top has none.
    #[test]
    fn a_head_perched_on_a_ladder_plate_exits_along_the_face() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let hatch = Vec3::new(3704.8699, 56.0, 992.125);
        assert!(perched(w, hatch));
        let exits = ladder_exits(w, hatch, -Vec3::X);
        assert_eq!(exits.len(), 2, "{exits:?}");
        for (p, back) in exits {
            assert!(back && footed(w, p) && (p.z - hatch.z).abs() < 1.0, "{p}");
            assert!((p.y - hatch.y).abs() > 24.0, "{p}");
        }
        let mast = Vec3::new(3675.0, -101.12528, 1145.126);
        assert!(perched(w, mast) && ladder_exits(w, mast, -Vec3::Y).is_empty());
        // A head on a deck is no perch.
        let deck = Vec3::new(3751.1277, 65.0, 616.125);
        assert!(!perched(w, deck) && ladder_exits(w, deck, Vec3::X).is_empty());
    }

    /// mp_berlin's roof gutter under UO's rules: the slope node a drop off
    /// the roof spawns lands on, at (-674, 4131, 616). The run south pins
    /// under the steep face, the step lifts the body onto it and it slides
    /// back, off the ground most ticks; only a jump from a tick on the
    /// ground takes it onto the ledge 40 up (bot-navigation.md "Jumps").
    /// mp_italy's allied `bas` bunker is left by the ladders in its base's
    /// `*4`, not the world's: a rung off each is found while the base
    /// stands, and none once a breach unlinks it (bot-navigation.md 2).
    #[test]
    fn italys_allied_bunker_ladders_are_its_bases() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_italy").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let inside = |b: &(Vec3, Vec3)| {
            b.0.x >= 6436.0 && b.1.x <= 6876.0 && b.0.y >= -2844.0 && b.1.y <= -2404.0
        };
        let climbs = |world: &CollisionWorld| {
            with_rules(MoveRules::Uo, || {
                ladders(world)
                    .into_iter()
                    .filter(inside)
                    .flat_map(|(lo, hi)| [Vec3::Y, -Vec3::Y].map(|n| rung(world, lo, hi, n)))
                    .flatten()
                    .filter(|(foot, head, up, _)| *up && foot.z < -700.0 && head.z > foot.z + 100.0)
                    .count()
            })
        };
        assert_eq!(climbs(&world.collision), 2);
        world.collision.set_model_linked(4, false);
        assert_eq!(climbs(&world.collision), 0);
    }
}
